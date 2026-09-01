//! The 3D models footprints carry, placed on the board and drawn in the
//! viewport.
//!
//! Checking that a connector clears a wall is the point of the viewport, and it
//! cannot be checked against a component that is not drawn. Everything here is
//! decoration: a model becomes a [`TriangleMesh`] and never a kernel solid, so
//! it cannot reach the enclosure geometry, the STEP and STL exports, or the
//! fitment booleans — all of which take solids.
//!
//! # Coordinates
//!
//! Placement is composed entirely in the enclosure frame (millimetres, Y up),
//! which is what [`FootprintInfo::position`] is already in. KiCad's Y flip
//! happens once, in the board reader, and must not happen again here.

use kicase_geometry::types::{Point3, Transform3d, TriangleMesh, Vector3};
use kicase_geometry::units::{mm, Length};
use kicase_kicad::board::{FootprintInfo, ModelRef};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

/// Where KiCad's shipped models live when the environment does not say.
///
/// KiCad resolves `${KICAD9_3DMODEL_DIR}` and friends from its own settings,
/// which KiCase cannot read; on this machine none of them is exported into the
/// environment either. Every version's variable points at the same directory
/// in practice, so they all fall back to one.
#[cfg(target_os = "windows")]
const DEFAULT_MODEL_DIR: &str = r"C:\Program Files\KiCad\share\kicad\3dmodels";
#[cfg(target_os = "macos")]
const DEFAULT_MODEL_DIR: &str = "/Applications/KiCad/KiCad.app/Contents/SharedSupport/3dmodels";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DEFAULT_MODEL_DIR: &str = "/usr/share/kicad/3dmodels";

/// Largest a component may be before it is treated as a unit-system mistake.
///
/// The STEP reader applies no `LENGTH_UNIT`, so a model authored in inches or
/// metres arrives 25.4 or 1000 times too big. That does not look like an error,
/// it looks like a component the size of the room, and it would swallow the
/// enclosure. Nothing a footprint carries is half a metre across.
const IMPLAUSIBLE_SIZE: Length = mm(500.0);

/// What one footprint's model reference resolved to.
enum Resolved {
    /// A file to read.
    File(PathBuf),
    /// Understood, but not loadable, with the reason to show the user.
    Skipped(String),
}

/// One placed component model, ready to be merged into the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentPlacement {
    /// Absolute path of the model file, and the key into the mesh cache.
    pub model: PathBuf,
    /// Model space to world: scale, then rotate, then translate.
    pub transform: Transform3d,
    /// The rotation alone, for normals.
    pub normals: Transform3d,
}

/// Everything one board's footprints ask for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ComponentRequest {
    pub placements: Vec<ComponentPlacement>,
    /// One line per model that could not be used, already deduplicated.
    pub problems: Vec<String>,
}

impl ComponentRequest {
    /// The distinct files that have to be read, in first-seen order.
    pub fn models(&self) -> Vec<PathBuf> {
        let mut seen = Vec::new();
        for placement in &self.placements {
            if !seen.contains(&placement.model) {
                seen.push(placement.model.clone());
            }
        }
        seen
    }
}

/// Works out where every footprint's models sit, and what could not be found.
///
/// `pcb_top` and `pcb_bottom` are the two faces of the drawn board: a
/// front-side model stands on the first, a back-side one hangs from the
/// second. KiCad's own STEP export puts them 0.05 mm further out, the
/// soldermask, which is stackup-dependent and far below the resolution of a
/// clearance check — and sitting the model on the face errs towards the part
/// being lower, which is the safe direction.
pub fn plan(
    footprints: &[FootprintInfo],
    project_dir: &Path,
    pcb_top: Length,
    pcb_bottom: Length,
) -> ComponentRequest {
    let mut request = ComponentRequest::default();
    for footprint in footprints {
        let z = if footprint.on_back { pcb_bottom } else { pcb_top };
        for model in &footprint.models {
            match resolve(&model.raw, project_dir) {
                Resolved::File(path) => {
                    let (transform, normals) = placement(footprint, model, z);
                    request.placements.push(ComponentPlacement { model: path, transform, normals });
                },
                Resolved::Skipped(problem) => {
                    if !request.problems.contains(&problem) {
                        request.problems.push(problem);
                    }
                },
            }
        }
    }
    request
}

/// The model-to-world transform, and the rotation alone for normals.
///
/// The order is the whole of it, and every step of it is confirmed against
/// KiCad's own STEP export — nineteen single-component exports through
/// `kicad-cli pcb export step --component-filter`, read back from the
/// `ITEM_DEFINED_TRANSFORMATION` in each file:
///
/// 1. scale by the model's own scale, about the model origin
/// 2. rotate about X, Y then Z by the *negated* model rotation
/// 3. translate by the model offset, in millimetres, Y unnegated
/// 4. on the back of the board, turn 180 degrees about X
/// 5. rotate about Z by the footprint's angle, counter-clockwise
/// 6. translate to the footprint, on the near face of the board
///
/// The two easy ones to get backwards are both pinned by a case: the flip goes
/// *inside* the footprint rotation (a back-side footprint at 90 degrees sends
/// model X to +Y, and the other order sends it to -Y), and the model rotations
/// compose Z, then Y, then X (rotate 90 90 0 sends model X to +Z, and the
/// reverse order sends it to +Y).
fn placement(footprint: &FootprintInfo, model: &ModelRef, z: Length) -> (Transform3d, Transform3d) {
    let mut rotation = rotate_x(-model.rotate[0])
        .then(&rotate_y(-model.rotate[1]))
        .then(&rotate_z(-model.rotate[2]));
    // Applied after the model's own rotation and before the footprint's, which
    // is why it cannot simply be folded into either.
    let offset = Vector3::new(mm(model.offset[0]), mm(model.offset[1]), mm(model.offset[2]));
    rotation.translation = offset;
    if footprint.on_back {
        rotation = rotation.then(&rotate_x(180.0));
    }
    rotation = rotation.then(&rotate_z(footprint.rotation));
    rotation.translation =
        rotation.translation + Vector3::new(footprint.position.x, footprint.position.y, z);

    // Scale is innermost, in the model's own axes: a model scaled 2,1,1 and
    // rotated 90 about Z still has its *local* X doubled.
    let mut transform = rotation;
    transform.x_axis = transform.x_axis * model.scale[0];
    transform.y_axis = transform.y_axis * model.scale[1];
    transform.z_axis = transform.z_axis * model.scale[2];
    (transform, rotation)
}

fn rotate_x(degrees: f64) -> Transform3d {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Transform3d {
        x_axis: Vector3::X,
        y_axis: Vector3::new(Length::ZERO, mm(cos), mm(sin)),
        z_axis: Vector3::new(Length::ZERO, mm(-sin), mm(cos)),
        translation: Vector3::ZERO,
    }
}

fn rotate_y(degrees: f64) -> Transform3d {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Transform3d {
        x_axis: Vector3::new(mm(cos), Length::ZERO, mm(-sin)),
        y_axis: Vector3::Y,
        z_axis: Vector3::new(mm(sin), Length::ZERO, mm(cos)),
        translation: Vector3::ZERO,
    }
}

fn rotate_z(degrees: f64) -> Transform3d {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Transform3d {
        x_axis: Vector3::new(mm(cos), mm(sin), Length::ZERO),
        y_axis: Vector3::new(mm(-sin), mm(cos), Length::ZERO),
        z_axis: Vector3::Z,
        translation: Vector3::ZERO,
    }
}

/// `outer` applied after `self`.
trait Compose {
    fn then(&self, outer: &Transform3d) -> Transform3d;
}

impl Compose for Transform3d {
    fn then(&self, outer: &Transform3d) -> Transform3d {
        let axis = |v: Vector3| outer.apply_vector(v);
        Transform3d {
            x_axis: axis(self.x_axis),
            y_axis: axis(self.y_axis),
            z_axis: axis(self.z_axis),
            translation: outer.apply(Point3::ZERO + self.translation) - Point3::ZERO,
        }
    }
}

/// Turns a `(model "...")` reference into a file, or into the reason it is not
/// one.
fn resolve(raw: &str, project_dir: &Path) -> Resolved {
    if let Some(name) = raw.strip_prefix("kicad-embed://") {
        // Not a missing file: the model really is inside the board, base64 and
        // zstd, and saying "not found" would send the user looking on disk.
        return Resolved::Skipped(format!(
            "the model {name} is embedded in the board; KiCase cannot read embedded models yet"
        ));
    }

    let expanded = expand(raw, project_dir);
    let path = Path::new(&expanded);
    let path = if path.is_absolute() { path.to_path_buf() } else { project_dir.join(path) };

    // VRML is not STEP and cannot be read. KiCad ships a .step beside almost
    // every .wrl it ships — 6837 of 6843 here — and the two are the same part,
    // so taking the sibling is a courtesy the user cannot lose by.
    let vrml = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wrl") || e.eq_ignore_ascii_case("vrml"));
    if vrml {
        for extension in ["step", "stp", "STEP"] {
            let sibling = path.with_extension(extension);
            if sibling.is_file() {
                return Resolved::File(sibling);
            }
        }
        return Resolved::Skipped(format!(
            "{} is VRML, which KiCase cannot read, and there is no STEP file beside it",
            path.display()
        ));
    }

    if path.is_file() {
        Resolved::File(path)
    } else {
        Resolved::Skipped(format!("the model {} was not found", path.display()))
    }
}

/// Substitutes the KiCad path variables a board may use.
fn expand(raw: &str, project_dir: &Path) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}').map(|at| start + at) else { break };
        let name = &rest[start + 2..end];
        let value = match name {
            "KIPRJMOD" => project_dir.display().to_string(),
            _ if name.ends_with("3DMODEL_DIR") || name == "KISYS3DMOD" => {
                std::env::var(name).unwrap_or_else(|_| DEFAULT_MODEL_DIR.to_string())
            },
            // Anything else is a variable only KiCad knows; leave it alone so
            // the "not found" message shows what the board actually said.
            _ => rest[start..=end].to_string(),
        };
        out.push_str(&value);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Loaded model meshes, keyed by the file they came from.
///
/// The mesh is cached rather than the STEP table: a 1 MB pin header expands to
/// tens of megabytes of B-rep and tessellates to 2044 triangles, and the mesh
/// is the only part that gets drawn. Failures are cached too, or every refresh
/// pays the read again and repeats the same warning.
#[derive(Default)]
pub struct ModelCache {
    entries: HashMap<PathBuf, Entry>,
}

enum Entry {
    Loaded(Arc<TriangleMesh>),
    Failed(String),
    Pending,
}

impl ModelCache {
    pub fn mesh(&self, path: &Path) -> Option<&Arc<TriangleMesh>> {
        match self.entries.get(path) {
            Some(Entry::Loaded(mesh)) => Some(mesh),
            _ => None,
        }
    }

    /// True when this path has never been asked for.
    fn is_new(&self, path: &Path) -> bool {
        !self.entries.contains_key(path)
    }

    fn mark_pending(&mut self, path: PathBuf) {
        self.entries.insert(path, Entry::Pending);
    }

    fn insert(&mut self, path: PathBuf, result: Result<TriangleMesh, String>) {
        let entry = match result {
            Ok(mesh) => Entry::Loaded(Arc::new(mesh)),
            Err(problem) => Entry::Failed(problem),
        };
        self.entries.insert(path, entry);
    }

    /// Everything that failed, as lines for the problems list.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .entries
            .iter()
            .filter_map(|(path, entry)| match entry {
                Entry::Failed(why) => Some(format!("{}: {why}", path.display())),
                _ => None,
            })
            .collect();
        problems.sort();
        problems
    }

    /// How many of `wanted` have finished loading.
    pub fn ready(&self, wanted: &[PathBuf]) -> usize {
        wanted
            .iter()
            .filter(|path| !matches!(self.entries.get(*path), None | Some(Entry::Pending)))
            .count()
    }
}

/// A background thread that reads and tessellates model files.
///
/// Reading one connector model costs 100-400 ms, nearly all of it parsing STEP
/// text, and a busy board has dozens of distinct ones. That is many frames, so
/// it happens off the UI thread and components appear as they arrive — which is
/// honest about what is happening in a way a spinner is not, and leaves the
/// enclosure usable meanwhile.
pub struct ModelLoader {
    requests: Sender<PathBuf>,
    results: Receiver<(PathBuf, Result<TriangleMesh, String>)>,
    /// Shared with the thread rather than handed to it at start, because the
    /// window only has a context to wake several frames in — and restarting the
    /// loader to deliver one would strand every request already in flight.
    waker: Arc<std::sync::Mutex<Option<crate::watcher::Waker>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for ModelLoader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl ModelLoader {
    pub fn start() -> Self {
        let (requests, inbox) = std::sync::mpsc::channel::<PathBuf>();
        let (outbox, results) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let waker: Arc<std::sync::Mutex<Option<crate::watcher::Waker>>> = Arc::default();
        let thread_waker = waker.clone();

        std::thread::spawn(move || {
            for path in inbox {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                let result = std::fs::read(&path)
                    .map_err(|err| err.to_string())
                    .and_then(|bytes| load(&path, &bytes));
                if outbox.send((path, result)).is_err() {
                    return;
                }
                let waker = thread_waker.lock().ok().and_then(|waker| waker.clone());
                if let Some(waker) = waker {
                    waker();
                }
            }
        });

        ModelLoader { requests, results, waker, stop }
    }

    /// Hands the loader a way to wake the window, once there is one.
    pub fn set_waker(&self, waker: crate::watcher::Waker) {
        if let Ok(mut held) = self.waker.lock() {
            *held = Some(waker);
        }
    }

    /// Asks for every model not already known. Cached paths cost nothing.
    pub fn request(&self, cache: &mut ModelCache, wanted: &[PathBuf]) {
        for path in wanted {
            if cache.is_new(path) {
                cache.mark_pending(path.clone());
                let _ = self.requests.send(path.clone());
            }
        }
    }

    /// Moves everything the thread has finished into the cache.
    pub fn drain(&self, cache: &mut ModelCache) -> bool {
        let mut arrived = false;
        while let Ok((path, result)) = self.results.try_recv() {
            cache.insert(path, result);
            arrived = true;
        }
        arrived
    }
}

/// Reads one model, and refuses one that is implausibly large.
fn load(path: &Path, bytes: &[u8]) -> Result<TriangleMesh, String> {
    let mesh = kicase_truck::load_step_mesh(bytes, kicase_truck::COMPONENT_MESH_TOLERANCE)
        .map_err(|err| err.to_string())?;
    // The STEP reader ignores the file's own LENGTH_UNIT, so a model authored
    // in inches or metres comes in 25.4 or 1000 times too big. Drawn, it would
    // swallow the enclosure; refused, it is one line in the problems list.
    if let Some(bounds) = mesh.bounds() {
        let size = bounds.size();
        let largest = size.x.max(size.y).max(size.z);
        if largest > IMPLAUSIBLE_SIZE {
            return Err(format!(
                "the model is {:.0} mm across, which is not a component — it was probably \
                 authored in different units",
                largest.mm()
            ));
        }
    }
    tracing::debug!("loaded {} ({} triangles)", path.display(), mesh.triangle_count());
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kicase_geometry::types::Point2;

    fn footprint(x: f64, y: f64, rotation: f64, on_back: bool, model: ModelRef) -> FootprintInfo {
        FootprintInfo {
            uuid: kicase_model::KiCadUuid::new("u"),
            reference: Some("X1".into()),
            // Already in the enclosure frame: KiCad's y = 100 is y = -100 here.
            position: Point2::new(mm(x), mm(-y)),
            rotation,
            on_back,
            models: vec![model],
        }
    }

    fn model(offset: [f64; 3], rotate: [f64; 3], scale: [f64; 3]) -> ModelRef {
        ModelRef { raw: "m.step".into(), offset, scale, rotate }
    }

    /// The oracle is KiCad itself. Each case below was exported alone with
    /// `kicad-cli pcb export step --component-filter <ref>` from a probe board,
    /// and the expected location and axes read out of the
    /// `ITEM_DEFINED_TRANSFORMATION` of the resulting file. They are cheap, and
    /// they are the only thing that catches a future sign flip.
    #[test]
    fn placement_matches_kicads_own_step_export() {
        let z = mm(1.615);
        // ref, (x, y, rotation, back), offset, rotate, expected location,
        // expected world direction of model +Z, of model +X.
        #[allow(clippy::type_complexity)]
        let cases: [(
            &str,
            (f64, f64, f64, bool),
            [f64; 3],
            [f64; 3],
            [f64; 3],
            [f64; 3],
            [f64; 3],
        ); 13] = [
            (
                "A",
                (100.0, 100.0, 0.0, false),
                [0.0; 3],
                [0.0; 3],
                [100.0, -100.0, 1.615],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "D",
                (140.0, 100.0, 90.0, false),
                [0.0; 3],
                [0.0; 3],
                [140.0, -100.0, 1.615],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
            ),
            (
                "E",
                (160.0, 100.0, 0.0, true),
                [0.0; 3],
                [0.0; 3],
                [160.0, -100.0, 1.615],
                [0.0, 0.0, -1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "F",
                (180.0, 100.0, 90.0, true),
                [0.0; 3],
                [0.0; 3],
                [180.0, -100.0, 1.615],
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
            ),
            (
                "G",
                (100.0, 140.0, 0.0, false),
                [1.0, 2.0, 3.0],
                [0.0; 3],
                [101.0, -138.0, 4.615],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "H",
                (120.0, 140.0, 90.0, false),
                [1.0, 2.0, 3.0],
                [0.0; 3],
                [118.0, -139.0, 4.615],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
            ),
            (
                "I",
                (140.0, 140.0, 0.0, true),
                [1.0, 2.0, 3.0],
                [0.0; 3],
                [141.0, -142.0, -1.385],
                [0.0, 0.0, -1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "J",
                (160.0, 140.0, 0.0, false),
                [0.0; 3],
                [90.0, 0.0, 0.0],
                [160.0, -140.0, 1.615],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "K",
                (180.0, 140.0, 0.0, false),
                [0.0; 3],
                [0.0, 90.0, 0.0],
                [180.0, -140.0, 1.615],
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
            ),
            (
                "L",
                (100.0, 180.0, 0.0, false),
                [0.0; 3],
                [0.0, 0.0, 90.0],
                [100.0, -180.0, 1.615],
                [0.0, 0.0, 1.0],
                [0.0, -1.0, 0.0],
            ),
            (
                "M",
                (120.0, 180.0, 90.0, false),
                [0.0; 3],
                [0.0, 0.0, 90.0],
                [120.0, -180.0, 1.615],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                "N",
                (140.0, 180.0, 0.0, false),
                [0.0; 3],
                [90.0, 90.0, 0.0],
                [140.0, -180.0, 1.615],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ),
            (
                "O",
                (160.0, 180.0, 0.0, true),
                [0.0; 3],
                [0.0, 0.0, 90.0],
                [160.0, -180.0, 1.615],
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
            ),
        ];

        for (name, (x, y, rotation, back), offset, rotate, location, model_z, model_x) in cases {
            let fp = footprint(x, y, rotation, back, model(offset, rotate, [1.0; 3]));
            let (transform, _) = placement(&fp, &fp.models[0], z);
            let got = [
                transform.translation.x.mm(),
                transform.translation.y.mm(),
                transform.translation.z.mm(),
            ];
            for axis in 0..3 {
                assert!(
                    (got[axis] - location[axis]).abs() < 1e-9,
                    "case {name}: location {got:?} wanted {location:?}"
                );
            }
            let z_axis = transform.z_axis;
            let x_axis = transform.x_axis;
            let got_z = [z_axis.x.mm(), z_axis.y.mm(), z_axis.z.mm()];
            let got_x = [x_axis.x.mm(), x_axis.y.mm(), x_axis.z.mm()];
            for axis in 0..3 {
                assert!(
                    (got_z[axis] - model_z[axis]).abs() < 1e-9,
                    "case {name}: model Z points {got_z:?}, wanted {model_z:?}"
                );
                assert!(
                    (got_x[axis] - model_x[axis]).abs() < 1e-9,
                    "case {name}: model X points {got_x:?}, wanted {model_x:?}"
                );
            }
        }
    }

    /// Case Q: scale 2,1,1 with the model turned 90 degrees about Z doubles the
    /// model's *local* X, not the world's.
    #[test]
    fn scale_is_innermost_and_leaves_the_normals_unscaled() {
        let fp =
            footprint(0.0, 0.0, 0.0, false, model([0.0; 3], [0.0, 0.0, 90.0], [2.0, 1.0, 1.0]));
        let (transform, normals) = placement(&fp, &fp.models[0], Length::ZERO);
        // Model +X becomes world -Y, twice as long.
        assert!((transform.x_axis.y.mm() + 2.0).abs() < 1e-9);
        assert!((normals.x_axis.y.mm() + 1.0).abs() < 1e-9);
    }

    #[test]
    fn resolves_kicad_variables_and_says_why_a_model_was_skipped() {
        let dir = std::env::temp_dir();
        let embedded = resolve("kicad-embed://part.step", &dir);
        assert!(matches!(&embedded, Resolved::Skipped(why) if why.contains("embedded")));

        let missing = resolve("${KICAD9_3DMODEL_DIR}/nope.3dshapes/nope.step", &dir);
        let Resolved::Skipped(why) = missing else { panic!("a missing model is not a file") };
        assert!(why.contains("3dmodels"), "the expanded path should be shown: {why}");

        // An unknown variable is left as written, so the message shows what the
        // board actually said.
        let odd = resolve("${SOMETHING_ELSE}/x.step", &dir);
        assert!(matches!(&odd, Resolved::Skipped(why) if why.contains("${SOMETHING_ELSE}")));
    }
}
