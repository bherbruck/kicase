//! Wires the designer window to the project and the build pipeline.

use crate::pipeline::{self, RebuildOptions, RebuildReport};
use crate::project::Project;
use kicase_geometry::kernel::CadKernel;
use kicase_model::model::OrphanKind;
use kicase_model::EnclosureConfig;
/// The backend the app builds with: pure Rust, so there is no C++
/// toolchain to install and nothing to cross-compile on any platform.
use kicase_truck::TruckKernel as Kernel;
use kicase_ui::{
    ActionReport, DesignerBackend, DesignerData, ExportKind, GraphicRow, HoleInfo, ItemInfo,
};

/// Everything derived from one reading of the board, kept so that a single
/// refresh does the work once instead of three times over.
struct Built {
    /// The settings this was built from, so the window can ask for the picture
    /// again without paying to prove it is still the right one.
    config: EnclosureConfig,
    reading: kicase_kicad::board::BoardReading,
    enclosure: kicase_model::Enclosure,
    scene: kicase_model::Scene,
    exterior: Option<String>,
    problems: Vec<String>,
    parts: Parts,
}

/// What the outline alone settles: change any of it and the whole shell has to
/// be built again.
type ShellKey = (kicase_model::Shell, kicase_model::Lid, kicase_model::ZLayout);

/// The geometry kept in the order it was built.
///
/// The shell is three booleans and a cutout is one, and on drawn arcs a boolean
/// is hundreds of milliseconds. Nothing a user nudges feature by feature — a
/// cutout, a boss, a datum, a mounting hole — moves the shell, so the shell is
/// the thing worth keeping, and each feature after it is worth keeping too so
/// that moving the last of them does not replay the first.
struct Parts {
    key: ShellKey,
    bottom: <Kernel as CadKernel>::Solid,
    lid: <Kernel as CadKernel>::Solid,
    warnings: Vec<kicase_model::Warning>,
    solids: Vec<SolidStage>,
    cuts: Vec<CutStage>,
}

/// The bottom after one added solid, and what that solid had to say.
struct SolidStage {
    feature: kicase_model::AddedSolid,
    bottom: <Kernel as CadKernel>::Solid,
    warnings: Vec<kicase_model::Warning>,
}

/// Both parts after one cutout, and what that cutout had to say.
struct CutStage {
    feature: kicase_model::Cutout,
    bottom: <Kernel as CadKernel>::Solid,
    lid: <Kernel as CadKernel>::Solid,
    warnings: Vec<kicase_model::Warning>,
}

impl Parts {
    /// The two parts as they stand after every feature.
    fn finished(&self) -> (&<Kernel as CadKernel>::Solid, &<Kernel as CadKernel>::Solid) {
        match self.cuts.last() {
            Some(stage) => (&stage.bottom, &stage.lid),
            None => (self.solids.last().map_or(&self.bottom, |stage| &stage.bottom), &self.lid),
        }
    }

    /// Everything the build had to say, in the order it said it.
    ///
    /// Each stage keeps only its own, so a stage that is reused cannot carry a
    /// stale copy of what its neighbours said.
    fn warnings(&self) -> impl Iterator<Item = &kicase_model::Warning> {
        self.warnings
            .iter()
            .chain(self.solids.iter().flat_map(|stage| &stage.warnings))
            .chain(self.cuts.iter().flat_map(|stage| &stage.warnings))
    }
}

pub struct AppBackend {
    project: Project,
    watcher: Option<crate::watcher::BoardWatcher>,
    built: Option<Built>,
    /// Kept so the watcher can be restarted once a waker is available.
    source_for_waker: Option<()>,
}

impl AppBackend {
    pub fn new(project: Project) -> Self {
        // Watch whatever this project reads from, so edits show up by
        // themselves. A watcher that cannot start is not fatal: the window
        // still works, it just will not update on its own.
        let source = match &project.origin {
            crate::project::Origin::Live(_) => Some(crate::watcher::WatchSource::LiveKiCad),
            crate::project::Origin::File(path) => {
                Some(crate::watcher::WatchSource::File(path.clone()))
            },
        };
        // The watcher starts without a waker; the window supplies one as soon
        // as it has a context to wake.
        let watcher = source.map(|source| crate::watcher::BoardWatcher::start(source, None));
        AppBackend { project, watcher, built: None, source_for_waker: None }
    }

    /// Reads and builds, doing only the part of it that has actually moved.
    ///
    /// Reading and resolving cost a tenth of a millisecond together, so they
    /// happen every time and the answer is compared rather than guessed at: the
    /// text of a board changes when a track moves, and a track is not geometry.
    fn ensure_built(&mut self, config: &EnclosureConfig) -> Result<&Built, String> {
        let text = self.project.board_text().map_err(|e| e.to_string())?;
        let reading = self.project.read_board_text(&text).map_err(|e| e.to_string())?;
        let enclosure =
            kicase_model::Enclosure::resolve(config, &reading.source).map_err(|e| e.to_string())?;

        let built = match self.built.take() {
            // Nothing the enclosure is made of moved, so neither did any of it.
            Some(previous) if previous.enclosure == enclosure => {
                Built { config: config.clone(), reading, ..previous }
            },
            previous => {
                let kernel = Kernel::new();
                let parts = build_parts(&kernel, &enclosure, previous.map(|b| b.parts))?;
                let (bottom, lid) = parts.finished();
                let scene = kicase_model::build_scene_of(
                    &kernel,
                    &enclosure,
                    bottom,
                    lid,
                    kicase_model::DISPLAY_TOLERANCE,
                )
                .map_err(|e| e.to_string())?;
                // From the mesh that was just built rather than from a second
                // tessellation of the whole body, which is what a kernel box
                // costs.
                let exterior = scene
                    .part(kicase_model::PartId::Bottom)
                    .and_then(|part| part.mesh.bounds())
                    .map(|bounds| {
                        let size = bounds.size();
                        format!(
                            "{:.2} x {:.2} x {:.2} mm (with lid: {:.2} mm tall)",
                            size.x.mm(),
                            size.y.mm(),
                            size.z.mm(),
                            enclosure.layout.total_height().mm()
                        )
                    });
                let problems = enclosure
                    .warnings
                    .iter()
                    .chain(parts.warnings())
                    .map(|warning| warning.to_string())
                    .collect();
                Built {
                    config: config.clone(),
                    reading,
                    enclosure,
                    scene,
                    exterior,
                    problems,
                    parts,
                }
            },
        };
        self.built = Some(built);
        self.built.as_ref().ok_or_else(|| "nothing built".to_string())
    }

    pub fn config(&self) -> &EnclosureConfig {
        &self.project.config
    }

    fn apply(&mut self, config: &EnclosureConfig) {
        self.project.config = config.clone();
    }
}

impl DesignerBackend for AppBackend {
    fn refresh(&mut self, config: &EnclosureConfig) -> Result<DesignerData, String> {
        self.apply(config);

        let live = self.project.origin.session().map(|s| s.version().to_string());
        let dir = self.project.dir.display().to_string();
        let config_snapshot = self.project.config.clone();

        // One read, one build, shared by everything below.
        let built = match self.ensure_built(config) {
            Ok(built) => built,
            Err(problem) => {
                // The board may simply not be ready to build yet; the window
                // still has to show why.
                return Ok(DesignerData {
                    project_dir: dir,
                    kicad_version: live,
                    problems: vec![problem],
                    ..DesignerData::default()
                });
            },
        };

        let enclosure = &built.enclosure;
        let reading = &built.reading;

        let mut data = DesignerData {
            project_dir: dir,
            kicad_version: live,
            board_summary: format!(
                "{} outline segment(s), {} enclosure graphic(s), {} mounting-hole candidate(s)",
                reading.source.board_outline.len(),
                reading.source.graphics.len(),
                reading.source.mounting_holes.len()
            ),
            dimensions: built.exterior.clone(),
            drawn_wall: enclosure.shell.wall_from_drawing.then(|| enclosure.shell.wall.to_string()),
            ..DesignerData::default()
        };
        data.problems.extend(reading.skipped.iter().cloned());
        // Everything the model and the build had to say. A cut that was too
        // shallow to make is reported here and nowhere else in the window.
        data.problems.extend(built.problems.iter().cloned());

        data.datums = enclosure
            .datums
            .iter()
            .map(|datum| ItemInfo {
                id: datum.id.clone(),
                uuid: datum.uuid.to_string(),
                detail: format!(
                    "z {:.2} mm, normal ({:.2}, {:.2})",
                    datum.z.mm(),
                    datum.normal.x.mm(),
                    datum.normal.y.mm()
                ),
                orphaned: false,
            })
            .collect();
        data.cutouts = enclosure
            .cutouts
            .iter()
            .map(|cutout| ItemInfo {
                id: cutout.id.clone(),
                uuid: cutout.uuid.to_string(),
                detail: match &cutout.placement {
                    kicase_model::CutPlacement::Side { datum, .. } => {
                        format!("side, datum \"{datum}\"")
                    },
                    kicase_model::CutPlacement::Vertical { face } => format!("{face:?} face"),
                },
                orphaned: false,
            })
            .collect();
        data.solids = enclosure
            .solids
            .iter()
            .map(|solid| ItemInfo {
                id: solid.id.clone(),
                uuid: solid.uuid.to_string(),
                detail: format!("z {:.2} mm, {:.2} mm tall", solid.z_start.mm(), solid.height.mm()),
                orphaned: false,
            })
            .collect();
        data.orphans = enclosure
            .orphans
            .iter()
            .map(|orphan| ItemInfo {
                id: orphan.id.clone(),
                uuid: orphan.uuid.clone(),
                detail: match orphan.kind {
                    OrphanKind::Datum => "datum",
                    OrphanKind::Feature => "feature",
                    OrphanKind::MountingHole => "mounting hole",
                }
                .to_string(),
                orphaned: true,
            })
            .collect();

        data.holes = reading
            .source
            .mounting_holes
            .iter()
            .map(|hole| HoleInfo {
                id: hole.reference.clone().unwrap_or_else(|| hole.uuid.to_string()),
                detail: format!(
                    "{} drill at ({:.2}, {:.2})",
                    hole.drill_diameter,
                    hole.position.x.mm(),
                    hole.position.y.mm()
                ),
                orphaned: false,
            })
            .collect();

        data.graphics = reading
            .source
            .graphics
            .iter()
            .map(|graphic| {
                let uuid = graphic.uuid.to_string();
                let bound_to = config_snapshot
                    .datums
                    .iter()
                    .find(|d| d.graphic_uuid == uuid)
                    .map(|d| d.id.clone())
                    .or_else(|| {
                        config_snapshot
                            .features
                            .iter()
                            .find(|f| f.graphic_uuid == uuid)
                            .map(|f| f.id.clone())
                    });
                GraphicRow {
                    layer: match graphic.role {
                        kicase_model::LayerRole::Datums => "Enclosure.Datums",
                        kicase_model::LayerRole::Cuts => "Enclosure.Cuts",
                        kicase_model::LayerRole::Top => "Enclosure.Top",
                        kicase_model::LayerRole::Bottom => "Enclosure.Bottom",
                        kicase_model::LayerRole::Solids => "Enclosure.Solids",
                        kicase_model::LayerRole::Outline => "Enclosure",
                        kicase_model::LayerRole::BoardOutline => "Edge.Cuts",
                    }
                    .to_string(),
                    description: describe_graphic(graphic),
                    closed: graphic.closed,
                    bound_to,
                    uuid,
                }
            })
            .collect();

        Ok(data)
    }

    fn initialize(&mut self, config: &mut EnclosureConfig) -> Result<ActionReport, String> {
        self.apply(config);
        let report = pipeline::init(&mut self.project).map_err(|e| e.to_string())?;
        *config = self.project.config.clone();
        Ok(to_action("Enclosure initialized", report))
    }

    fn rebuild(&mut self, config: &EnclosureConfig) -> Result<ActionReport, String> {
        self.apply(config);
        let options = RebuildOptions::rebuild(&self.project.config);
        let report = pipeline::rebuild(&mut self.project, options).map_err(|e| e.to_string())?;
        Ok(to_action("Rebuilt", report))
    }

    fn export(
        &mut self,
        config: &EnclosureConfig,
        kind: ExportKind,
    ) -> Result<ActionReport, String> {
        self.apply(config);
        let options = match kind {
            ExportKind::Step => {
                RebuildOptions { step: true, stl: false, openscad: false, update_kicad: false }
            },
            ExportKind::Stl => {
                RebuildOptions { step: false, stl: true, openscad: false, update_kicad: false }
            },
            ExportKind::OpenScad => {
                RebuildOptions { step: false, stl: false, openscad: true, update_kicad: false }
            },
        };
        let report = pipeline::rebuild(&mut self.project, options).map_err(|e| e.to_string())?;
        Ok(to_action(kind.label(), report))
    }

    fn save(&mut self, config: &EnclosureConfig) -> Result<(), String> {
        self.apply(config);
        self.project.save_config().map_err(|e| e.to_string())
    }

    fn board_changed(&mut self) -> bool {
        self.watcher.as_ref().is_some_and(|w| w.take_change())
    }

    fn set_repaint_waker(&mut self, waker: std::sync::Arc<dyn Fn() + Send + Sync>) {
        // Restart the watcher so it can wake the window directly. Until now it
        // has only been queuing changes for the next poll.
        let source = match &self.project.origin {
            crate::project::Origin::Live(_) => crate::watcher::WatchSource::LiveKiCad,
            crate::project::Origin::File(path) => crate::watcher::WatchSource::File(path.clone()),
        };
        self.watcher = Some(crate::watcher::BoardWatcher::start(source, Some(waker)));
        self.source_for_waker = Some(());
    }

    fn scene(&mut self, config: &EnclosureConfig) -> Result<kicase_model::Scene, String> {
        self.apply(config);
        // Built during the refresh that always precedes this. Reading the board
        // again to prove it is an IPC round trip to KiCad for an answer already
        // in hand.
        if let Some(built) = self.built.as_ref().filter(|built| built.config == *config) {
            return Ok(built.scene.clone());
        }
        Ok(self.ensure_built(config)?.scene.clone())
    }
}

/// Builds whatever has moved and keeps what has not.
///
/// The chains are walked to the first entry whose feature differs and replayed
/// from there. Order is the whole of it: what one feature leaves behind is what
/// the next one starts from, so a link is only reusable when every link before
/// it is.
fn build_parts(
    kernel: &Kernel,
    enclosure: &kicase_model::Enclosure,
    cached: Option<Parts>,
) -> Result<Parts, String> {
    let key = (enclosure.shell.clone(), enclosure.lid, enclosure.layout);
    let mut parts = match cached {
        Some(parts) if parts.key == key => parts,
        // The drawn outline itself moved: nothing built on it survives.
        _ => {
            let shell =
                kicase_model::builder::build_shell(kernel, enclosure).map_err(|e| e.to_string())?;
            Parts {
                key,
                bottom: shell.bottom,
                lid: shell.lid,
                warnings: shell.warnings,
                solids: Vec::new(),
                cuts: Vec::new(),
            }
        },
    };
    let kept = parts
        .solids
        .iter()
        .zip(&enclosure.solids)
        .take_while(|(stage, feature)| &stage.feature == *feature)
        .count();
    let solids_intact = kept == parts.solids.len() && kept == enclosure.solids.len();
    parts.solids.truncate(kept);
    for solid in &enclosure.solids[kept..] {
        let (bottom, warning) = {
            let base = parts.solids.last().map_or(&parts.bottom, |stage| &stage.bottom);
            kicase_model::builder::apply_solid(kernel, base, solid, enclosure)
                .map_err(|e| e.to_string())?
        };
        parts.solids.push(SolidStage {
            feature: solid.clone(),
            bottom,
            warnings: warning.into_iter().collect(),
        });
    }

    // A different bottom under the cuts means every cut has to be made again.
    if !solids_intact {
        parts.cuts.clear();
    }
    let kept = parts
        .cuts
        .iter()
        .zip(&enclosure.cutouts)
        .take_while(|(stage, feature)| &stage.feature == *feature)
        .count();
    parts.cuts.truncate(kept);
    let margin = kicase_model::builder::cavity_margin(enclosure);
    for cutout in &enclosure.cutouts[kept..] {
        let result = {
            let (base_bottom, base_lid) = match parts.cuts.last() {
                Some(stage) => (&stage.bottom, &stage.lid),
                None => {
                    (parts.solids.last().map_or(&parts.bottom, |stage| &stage.bottom), &parts.lid)
                },
            };
            kicase_model::builder::apply_cutout(
                kernel,
                base_bottom,
                base_lid,
                cutout,
                enclosure,
                margin,
            )
            .map_err(|e| e.to_string())?
        };
        parts.cuts.push(CutStage {
            feature: cutout.clone(),
            bottom: result.bottom,
            lid: result.lid,
            warnings: result.warnings,
        });
    }

    Ok(parts)
}

/// A one-line description of a drawn shape, for the graphics list.
fn describe_graphic(graphic: &kicase_model::BoardGraphic) -> String {
    let mut points = Vec::new();
    for curve in &graphic.curves {
        points.push(curve.start());
        points.push(curve.end());
        if let kicase_geometry::profile::Curve2::Arc(arc) = curve {
            points.push(arc.mid);
        }
    }
    match kicase_geometry::types::Bounds2::from_points(points) {
        Some(bounds) if graphic.closed => format!(
            "{:.2} x {:.2} mm at ({:.2}, {:.2})",
            bounds.width().mm(),
            bounds.height().mm(),
            bounds.min.x.mm(),
            bounds.min.y.mm()
        ),
        Some(bounds) => format!(
            "line from ({:.2}, {:.2}) to ({:.2}, {:.2})",
            bounds.min.x.mm(),
            bounds.min.y.mm(),
            bounds.max.x.mm(),
            bounds.max.y.mm()
        ),
        None => "empty".to_string(),
    }
}

fn to_action(headline: &str, report: RebuildReport) -> ActionReport {
    let mut action =
        ActionReport { headline: headline.to_string(), lines: Vec::new(), ok: report.is_clean() };
    for file in &report.files {
        action.lines.push(format!("wrote {}", file.display()));
    }
    for warning in &report.warnings {
        action.lines.push(format!("warning: {warning}"));
    }
    for orphan in &report.orphans {
        action.lines.push(format!("orphaned: {} ({})", orphan.id, orphan.uuid));
    }
    for note in &report.notes {
        action.lines.push(note.clone());
    }
    action
}
