//! [`CadKernel`] implemented on truck — pure Rust, no C++ toolchain.

use crate::convert::{from_point, from_vector, loop_to_wire, to_point};
use kicase_geometry::error::{GeometryError, Result};
use kicase_geometry::kernel::{CadKernel, NamedSolid};
use kicase_geometry::profile::Profile2d;
use kicase_geometry::types::{Bounds3, Plane3, Transform3d, TriangleMesh};
use kicase_geometry::units::{mm, Length};
use monstertruck_meshing::prelude::*;
use monstertruck_modeling::{
    builder, Face, Matrix4, Point3 as MtPoint3, Shell, Solid, Vector3 as MtVec3,
};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// Tolerance handed to the boolean engine, in millimetres.
///
/// Enclosure walls are millimetres thick and drawings are snapped to microns,
/// so a micron is fine enough to keep every real feature and coarse enough
/// that coincident faces produced by our own strokes still classify.
///
/// It is the coarsest tolerance ever used, never the only one: see
/// [`tolerance_for`].
const BOOLEAN_TOLERANCE: f64 = 2.0e-3;

/// How much finer than the cut itself the engine's tolerance has to be for the
/// cut to survive.
///
/// Measured on this engine: a clean pair of boxes resolves a feature about
/// 1.26 times its tolerance, and a whole enclosure about ten times. Ten is the
/// ratio that holds everywhere.
const RESOLUTION_RATIO: f64 = 10.0;

/// The finest tolerance a boolean is ever asked for.
///
/// This is the one geometric limit in KiCase that is a constant rather than
/// something drawn, so it is chosen rather than inherited. The tolerance is
/// also the chord tolerance both operands are tessellated at, so a shallow cut
/// into an arc-swept shell pays for a very fine mesh of the whole shell:
/// measured, 5e-6 costs about two minutes on the worst arc geometry here where
/// 1e-4 costs about four seconds. 5e-6 is the number, because it puts the
/// shallowest cut this backend will make at 5e-5 mm — twenty times below the
/// micron a KiCad drawing is snapped to, so every feature a user can express
/// gets cut, and a two-minute build only ever happens for a feature nobody
/// could have drawn deliberately.
const MIN_TOLERANCE: f64 = 5.0e-6;

/// Probe sizes tried in turn when measuring how deeply two bodies overlap.
///
/// Coarsest first: the rung that first finds a point inside both bodies is a
/// bracket on the depth from above, which is all the tolerance needs. It runs
/// two orders past what can be cut, because below the last rung "they overlap"
/// and "they merely touch" are the same observation — and a cut that is too
/// shallow to make has to be reported, not dropped in silence.
const DEPTH_LADDER: [f64; 12] =
    [2e-2, 5e-3, 1e-3, 5e-4, 2e-4, 1e-4, 5e-5, 2e-5, 1e-5, 5e-6, 2e-6, 1e-6];

/// How far a boolean's tool is displaced before the engine sees it.
///
/// A ten-thousandth of a micron: four orders below the engine's own tolerance,
/// so it cannot change a classification, and four orders below the micron a
/// KiCad drawing is snapped to, so it cannot change the part. Its only job is
/// to break an exact coincidence — see [`boolean`].
const NUDGE: f64 = 1.0e-7;

/// Tessellation tolerance used when asking whether two bodies share material.
///
/// It only has to be fine enough that a real overlap shows up as crossing
/// triangles, and every weld and every cutter in an enclosure overlaps by whole
/// millimetres.
const OVERLAP_TOLERANCE: Length = mm(0.05);
/// How far under a face a point has to sit to count as inside the body.
///
/// Well under the thinnest wall KiCase will print, and well over the sag of a
/// tessellated arc at [`OVERLAP_TOLERANCE`].
const INTERIOR_STEP: f64 = 0.01;

/// Samples taken along each edge curve when boxing a body.
const CURVE_SAMPLES: usize = 8;

/// A closed contour living on a known plane in 3D.
pub struct TruckProfile {
    face: Face,
    plane: Plane3,
}

impl TruckProfile {
    pub fn plane(&self) -> &Plane3 {
        &self.plane
    }
}

/// One or more disjoint bodies.
///
/// truck's `Solid` is a single connected body, but KiCase routinely holds
/// several at once — a lid and a base, or the islands a cut leaves behind — so
/// the kernel type is a list and the booleans keep it disjoint.
pub struct TruckSolid {
    bodies: Vec<Solid>,
}

impl TruckSolid {
    fn new(bodies: Vec<Solid>) -> Self {
        Self { bodies }
    }

    pub fn bodies(&self) -> &[Solid] {
        &self.bodies
    }
}

/// The truck B-rep backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct TruckKernel;

impl TruckKernel {
    pub fn new() -> Self {
        TruckKernel
    }
}

impl CadKernel for TruckKernel {
    type Profile = TruckProfile;
    type Solid = TruckSolid;

    fn make_profile(&self, profile: &Profile2d, plane: &Plane3) -> Result<Self::Profile> {
        let mut wires = vec![loop_to_wire(&profile.outer, plane)?];
        for hole in &profile.holes {
            // `Profile2d` already winds holes opposite the outer loop, which is
            // the orientation a face needs to bound material rather than void.
            wires.push(loop_to_wire(hole, plane)?);
        }
        let face: Face = builder::try_attach_plane(wires)
            .map_err(|e| GeometryError::kernel("make_profile", e.to_string()))?;
        Ok(TruckProfile { face, plane: *plane })
    }

    fn extrude(&self, profile: &Self::Profile, distance: Length) -> Result<Self::Solid> {
        if distance.mm().abs() <= f64::EPSILON {
            return Err(GeometryError::NonPositive { name: "extrusion distance", value: distance });
        }
        let n = profile.plane.normal();
        let direction = MtVec3::new(n.x.mm(), n.y.mm(), n.z.mm()) * distance.mm();
        // truck sweeps the face along the vector; a face whose normal opposes
        // the sweep produces an inside-out solid, so orient the seed face.
        let seeded = if face_normal(&profile.face).dot(direction) < 0.0 {
            profile.face.inverse()
        } else {
            profile.face.clone()
        };
        let solid: Solid = builder::extrude(&seeded, direction);
        crate::cylinders::make_arc_faces_analytic(&solid, direction);
        Ok(TruckSolid::new(vec![solid]))
    }

    fn union(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let mut bodies: Vec<Solid> = a.bodies.clone();
        for addition in &b.bodies {
            merge_into(&mut bodies, addition.clone())?;
        }
        Ok(TruckSolid::new(bodies))
    }

    fn subtract(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let mut bodies = a.bodies.clone();
        for tool in &b.bodies {
            let mut next = Vec::with_capacity(bodies.len());
            for body in bodies {
                // A tool that reaches nothing leaves the body alone. Asking the
                // engine instead is both slower and wrong: it answers with an
                // empty solid, which is how a pocket in the shell used to
                // delete the whole lid.
                let Some(depth) = penetration(tool, &body) else {
                    next.push(body);
                    continue;
                };
                // The engine resolves nothing finer than the tolerance it is
                // given, so a shallow cut buys a finer one; a deep cut gets the
                // standard tolerance and costs exactly what it always did. When
                // even the finest is not enough the cut cannot be made, and
                // saying so is the whole point — dropping it in silence leaves
                // the user with a part they never asked for.
                let Some(tolerance) = tolerance_for(depth) else {
                    return Err(GeometryError::kernel(
                        "subtract",
                        format!(
                            "the cut is only {depth:.2e} mm deep, \
                             below what this kernel can resolve"
                        ),
                    ));
                };
                let cut =
                    boolean_at("subtract", monstertruck_solid::difference, &body, tool, tolerance)?;
                // Empty is the honest answer only when the tool covered the
                // body outright.
                if cut.is_empty() && !swallows(tool, &body) {
                    return Err(GeometryError::kernel("subtract", "the cut left nothing behind"));
                }
                next.extend(separate(cut).into_iter().filter(|body| !body.is_empty()));
            }
            bodies = next;
        }
        Ok(TruckSolid::new(bodies))
    }

    fn intersect(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let mut bodies = Vec::new();
        for left in &a.bodies {
            for right in &b.bodies {
                // Two parts designed to meet flat on the rim share a plane and
                // no volume. The engine calls that a broken shell rather than
                // the empty answer it is, so it never gets asked.
                //
                // The bar here is higher than for a cut, and deliberately. A lid
                // and a shell share whole surfaces by construction — the same
                // outer footprint, the rim they meet on — and two meshes of one
                // curved surface wander apart by their own chord error, which
                // reads as interpenetration on any rounded outline. Only an
                // overlap deeper than that error is evidence of one. Nothing is
                // lost by it: this asks whether two printed parts collide, and
                // an overlap thinner than the mesh that found it is not a
                // collision anyone can print, let alone measure.
                if penetration(left, right).is_none() {
                    continue;
                }
                let common = boolean("intersect", monstertruck_solid::and, left, right)?;
                bodies.extend(separate(common).into_iter().filter(|body| !body.is_empty()));
            }
        }
        Ok(TruckSolid::new(bodies))
    }

    fn transform(&self, solid: &Self::Solid, transform: Transform3d) -> Result<Self::Solid> {
        let m = to_matrix(&transform);
        let bodies = solid.bodies.iter().map(|body| builder::transformed(body, m)).collect();
        Ok(TruckSolid::new(bodies))
    }

    fn bounds(&self, solid: &Self::Solid) -> Result<Bounds3> {
        let mut min = MtPoint3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut max = MtPoint3::new(f64::MIN, f64::MIN, f64::MIN);
        for body in &solid.bodies {
            let mesh = raw_polygon(body, Length::from_mm(0.05));
            for p in mesh.positions() {
                min = MtPoint3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
                max = MtPoint3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
            }
        }
        if min.x > max.x {
            return Err(GeometryError::kernel("bounds", "shape is empty"));
        }
        Ok(Bounds3 { min: from_point(min), max: from_point(max) })
    }

    fn volume(&self, solid: &Self::Solid) -> Result<f64> {
        let mut total = 0.0;
        for body in &solid.bodies {
            total += raw_polygon(body, Length::from_mm(0.01)).volume().abs();
        }
        Ok(total)
    }

    fn mesh(&self, solid: &Self::Solid, tolerance: Length) -> Result<TriangleMesh> {
        GeometryError::require_positive("mesh tolerance", tolerance)?;
        let mut out = TriangleMesh::default();
        for body in &solid.bodies {
            let mut mesh = polygon(body, tolerance)?;
            mesh.add_smooth_normals(std::f64::consts::FRAC_PI_6, false);
            mesh.triangulate();
            append_triangles(&mut out, &mesh);
        }
        Ok(out)
    }

    fn solid_count(&self, solid: &Self::Solid) -> Result<usize> {
        Ok(solid.bodies.len())
    }

    fn export_step(&self, solid: &Self::Solid, path: &Path) -> Result<()> {
        let named = [NamedSolid { name: "body", solid }];
        self.export_step_assembly(&named, path)
    }

    fn export_step_assembly(
        &self,
        solids: &[NamedSolid<'_, Self::Solid>],
        path: &Path,
    ) -> Result<()> {
        ensure_parent_dir(path)?;
        let bodies: Vec<_> = solids
            .iter()
            .flat_map(|named| named.solid.bodies.iter())
            .map(|b| b.compress())
            .collect();
        if bodies.is_empty() {
            return Err(GeometryError::kernel("export_step_assembly", "no solids to write"));
        }
        let models = monstertruck_io::step::save::StepModels::from_iter(bodies.iter());
        let text = monstertruck_io::step::save::CompleteStepDisplay::new(
            models,
            monstertruck_io::step::save::StepHeaderDescriptor {
                organization_system: "KiCase".to_owned(),
                ..Default::default()
            },
        )
        .to_string();
        std::fs::write(path, text)
            .map_err(|source| GeometryError::Io { path: path.display().to_string(), source })
    }

    fn export_stl(&self, solid: &Self::Solid, path: &Path, tolerance: Length) -> Result<()> {
        ensure_parent_dir(path)?;
        GeometryError::require_positive("stl tolerance", tolerance)?;
        let mesh = self.mesh(solid, tolerance)?;
        write_binary_stl(&mesh, path)
    }
}

/// Adds `body` to `bodies`, fusing it with every body it touches.
///
/// Fusing transitively matters: a new body can bridge two that were disjoint,
/// and leaving them separate would report the wrong body count.
fn merge_into(bodies: &mut Vec<Solid>, body: Solid) -> Result<()> {
    let mut merged = body;
    let mut index = 0;
    while index < bodies.len() {
        // Bodies that only touch stay separate, which is what a union of
        // disjoint solids means and the only thing the engine can do with them.
        if penetration(&bodies[index], &merged).is_none() {
            index += 1;
            continue;
        }
        let existing = bodies.remove(index);
        merged = boolean("union", monstertruck_solid::or, &existing, &merged)?;
        index = 0;
    }
    bodies.extend(separate(merged));
    Ok(())
}

/// One of monstertruck's boolean entry points.
type Boolean =
    fn(&Solid, &Solid, f64) -> std::result::Result<Solid, monstertruck_solid::ShapeOpsError>;

/// Runs one boolean, containing the two ways monstertruck 0.4 rejects topology
/// that is sound.
///
/// It reports failure by panicking as often as by returning an error, so the
/// call is caught the way the OpenCascade backend catches its own: a bad
/// feature must cost the user that feature, not the whole plugin.
///
/// The tool goes in displaced, for a defect we cannot reach from here.
/// Intersection curves are found by mesh interference, and an arc-swept face is
/// tessellated at dyadic fractions of its sweep; when a mating plane lands
/// exactly on one of those vertex rows no triangle straddles it, the
/// intersection loop never closes, and a valid cut is rejected. Every height in
/// an enclosure is a round number, so those coincidences are the rule rather
/// than the exception here. The straight call is kept as the fallback, since
/// the displacement is a guess at which grid the engine is on.
fn boolean(operation: &'static str, op: Boolean, target: &Solid, tool: &Solid) -> Result<Solid> {
    boolean_at(operation, op, target, tool, BOOLEAN_TOLERANCE)
}

/// The same, at a tolerance chosen for this pair of operands.
fn boolean_at(
    operation: &'static str,
    op: Boolean,
    target: &Solid,
    tool: &Solid,
    tolerance: f64,
) -> Result<Solid> {
    // A different offset per axis, so a coincidence on any axis-aligned plane
    // is broken rather than merely moved to another.
    let off = MtVec3::new(NUDGE, NUDGE * 2.0, NUDGE * 3.0);
    let displaced = builder::transformed(tool, Matrix4::from_translation(off));
    match attempt(op, target, &displaced, tolerance) {
        Ok(solid) => Ok(solid),
        Err(reason) => attempt(op, target, tool, tolerance)
            .map_err(|_| GeometryError::kernel(operation, reason)),
    }
}

fn attempt(
    op: Boolean,
    target: &Solid,
    tool: &Solid,
    tolerance: f64,
) -> std::result::Result<Solid, String> {
    match catch_unwind(AssertUnwindSafe(|| op(target, tool, tolerance))) {
        Ok(Ok(solid)) => Ok(solid),
        Ok(Err(error)) => Err(format!("{error:?}")),
        Err(_) => Err("truck panicked on this topology".to_owned()),
    }
}

/// Splits a boolean result that came back as several boundary shells.
///
/// monstertruck packs every connected component of a result into one `Solid`,
/// which its own booleans then cannot consume and which would report a severed
/// part as a single body. A shell that encloses material becomes a body of its
/// own; a shell enclosing void is an internal cavity and stays where it is.
fn separate(solid: Solid) -> Vec<Solid> {
    if solid.boundaries().len() < 2 {
        return vec![solid];
    }
    let shells = solid.into_boundaries();
    if shells.iter().any(|shell| shell_volume(shell) <= 0.0) {
        return vec![Solid::new_unchecked(shells)];
    }
    shells.into_iter().map(|shell| Solid::new_unchecked(vec![shell])).collect()
}

fn shell_volume(shell: &Shell) -> f64 {
    shell.triangulation(OVERLAP_TOLERANCE.mm()).to_polygon().volume()
}

/// How deeply two bodies share volume, or `None` when they only rest against
/// one another.
///
/// Two questions ride on this, and they are not the same question. The first is
/// whether the engine may be asked at all: monstertruck has no answer for
/// operands that do not interpenetrate — `and` and `or` report a broken shell,
/// and `difference` returns an empty solid instead of the untouched target,
/// which then panics the next boolean it is handed to — and a lid sitting on a
/// rim is a shape the enclosure is built around. The second is how much
/// material there is to cut, which is what sets the tolerance the engine needs.
/// One bit cannot answer both: rejecting a flush touch wants a coarse probe and
/// resolving a shallow cut wants a fine one. So this measures instead.
///
/// The question is settled on the meshes, seeded from where the two surfaces
/// cross: around any such point, two bodies that interpenetrate have a point
/// inside both, and two that merely touch do not. How far out that point has to
/// be looked for is the depth.
fn penetration(a: &Solid, b: &Solid) -> Option<f64> {
    penetration_by(a, b, &DEPTH_LADDER)
}

/// [`penetration`], asking at the reaches given rather than the cut ladder.
fn penetration_by(a: &Solid, b: &Solid, ladder: &[f64]) -> Option<f64> {
    if !overlaps(a, b) {
        return None;
    }
    // Welded, not raw. `inside` casts a ray and counts crossings, which needs a
    // closed mesh; truck's triangulation is not closed until its duplicated
    // vertices are put together. Measuring a volume or a box tolerates the raw
    // form, but a containment test reads pure noise from it.
    let (Ok(left), Ok(right)) = (polygon(a, OVERLAP_TOLERANCE), polygon(b, OVERLAP_TOLERANCE))
    else {
        return None;
    };
    let contact = left.extract_interference(&right);
    if contact.is_empty() {
        // Surfaces that never cross leave one case: a body buried whole in the
        // other, which one interior point settles, and which is as deep an
        // overlap as there is.
        let whole = buried(&left, &right) || buried(&right, &left);
        return whole.then_some(f64::INFINITY);
    }
    let points: Vec<MtPoint3> = contact.iter().map(|(from, to)| from.midpoint(*to)).collect();
    ladder.iter().copied().find(|reach| {
        points
            .iter()
            .any(|point| corners(*point, *reach).any(|p| left.inside(p) && right.inside(p)))
    })
}

/// The eight corners of a cube of side `2 * reach` around `point`.
///
/// Corners only, with no zero offset among them: a contact point lies on a face
/// of both bodies, and a probe point that kept any one of its coordinates would
/// lie on that face too — where `inside` casts a ray from the surface itself and
/// answers by coin toss.
fn corners(point: MtPoint3, reach: f64) -> impl Iterator<Item = MtPoint3> {
    const SIGNS: [f64; 2] = [-1.0, 1.0];
    SIGNS.iter().flat_map(move |x| {
        SIGNS.iter().flat_map(move |y| {
            SIGNS.iter().map(move |z| point + MtVec3::new(x * reach, y * reach, z * reach))
        })
    })
}

/// The tolerance one boolean needs to resolve a cut of the given depth, or
/// `None` when no tolerance this backend will use is fine enough.
fn tolerance_for(depth: f64) -> Option<f64> {
    let tolerance = (depth / RESOLUTION_RATIO).min(BOOLEAN_TOLERANCE);
    (tolerance >= MIN_TOLERANCE).then_some(tolerance)
}

/// Whether `inner` lies wholly within `outer`.
fn buried(inner: &PolygonMesh, outer: &PolygonMesh) -> bool {
    // A point taken from just under a face rather than on it, so a shared
    // surface cannot make the ray cast a coin toss.
    interior_points(inner).take(8).any(|point| outer.inside(point))
}

/// Points just inside the body, one under each face of its mesh.
fn interior_points(mesh: &PolygonMesh) -> impl Iterator<Item = MtPoint3> + '_ {
    let positions = mesh.positions();
    // truck orients a solid's boundary outward, but `extrude` can seed a face
    // the other way round, so take the direction to step in from the mesh.
    let depth = if mesh.volume() < 0.0 { -INTERIOR_STEP } else { INTERIOR_STEP };
    mesh.face_iter().filter_map(move |face| {
        let corner = |index: usize| Some(positions[face.get(index)?.pos]);
        let (a, b, c) = (corner(0)?, corner(1)?, corner(2)?);
        let normal = (b - a).cross(c - a).normalize();
        let centre = a + (b - a) / 3.0 + (c - a) / 3.0;
        normal.magnitude2().is_finite().then(|| centre - normal * depth)
    })
}

/// Whether `outer`'s box encloses `inner`'s, the only way `inner` can be
/// wholly buried in `outer`.
fn swallows(outer: &Solid, inner: &Solid) -> bool {
    let (Some((omin, omax)), Some((imin, imax))) = (bbox(outer), bbox(inner)) else {
        return false;
    };
    let slack = BOOLEAN_TOLERANCE;
    omin.x <= imin.x + slack
        && omin.y <= imin.y + slack
        && omin.z <= imin.z + slack
        && imax.x <= omax.x + slack
        && imax.y <= omax.y + slack
        && imax.z <= omax.z + slack
}

/// Cheap rejection test: bodies whose boxes miss cannot interact, and asking
/// the boolean engine about them is both slow and a source of spurious errors.
fn overlaps(a: &Solid, b: &Solid) -> bool {
    // A body with no geometry left touches nothing.
    let (Some((amin, amax)), Some((bmin, bmax))) = (bbox(a), bbox(b)) else {
        return false;
    };
    let slack = BOOLEAN_TOLERANCE;
    amin.x <= bmax.x + slack
        && bmin.x <= amax.x + slack
        && amin.y <= bmax.y + slack
        && bmin.y <= amax.y + slack
        && amin.z <= bmax.z + slack
        && bmin.z <= amax.z + slack
}

/// Axis-aligned box around one body, or `None` if it has no bounded geometry.
fn bbox(body: &Solid) -> Option<(MtPoint3, MtPoint3)> {
    let compressed = body.compress();
    let mut min = MtPoint3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut max = MtPoint3::new(f64::MIN, f64::MIN, f64::MIN);
    let mut push = |p: MtPoint3| {
        min = MtPoint3::new(min.x.min(p.x), min.y.min(p.y), min.z.min(p.z));
        max = MtPoint3::new(max.x.max(p.x), max.y.max(p.y), max.z.max(p.z));
    };
    for shell in &compressed.boundaries {
        for vertex in &shell.vertices {
            push(*vertex);
        }
        // Vertices alone miss the bulge of an arc, so walk each curve too.
        for edge in &shell.edges {
            let Some((t0, t1)) = edge.curve.try_range_tuple() else { continue };
            for step in 1..CURVE_SAMPLES {
                let t = t0 + (t1 - t0) * step as f64 / CURVE_SAMPLES as f64;
                push(edge.curve.subs(t));
            }
        }
    }
    (min.x <= max.x).then_some((min, max))
}

/// Outward normal of a planar face at its parametric centre.
fn face_normal(face: &Face) -> MtVec3 {
    let surface = face.oriented_surface();
    let (u_range, v_range) = surface.try_range_tuple();
    surface.normal(mid(u_range), mid(v_range))
}

/// Midpoint of a parameter range, or an arbitrary interior value for the
/// unbounded directions of an infinite plane — every point has the same normal.
fn mid(range: Option<(f64, f64)>) -> f64 {
    range.map_or(0.5, |(a, b)| (a + b) / 2.0)
}

/// Tessellates one body, welded into a mesh whose vertices are shared.
///
/// Only display needs this: smooth normals are averaged across the faces
/// meeting at a vertex, so the vertices have to be the same ones.
fn polygon(body: &Solid, tolerance: Length) -> Result<PolygonMesh> {
    let mut mesh = raw_polygon(body, tolerance);
    mesh.put_together_same_attrs(TOLERANCE);
    mesh.remove_degenerate_faces();
    Ok(mesh)
}

/// Tessellates one body as truck produced it.
///
/// Measurement takes this path rather than [`polygon`]. A volume is a sum over
/// triangles and a box is a min and a max over positions, so neither notices a
/// duplicated vertex, while welding them is quadratic in the faces sharing a
/// normal and costs more than everything else in the measurement together.
fn raw_polygon(body: &Solid, tolerance: Length) -> PolygonMesh {
    body.triangulation(tolerance.mm().max(0.001)).to_polygon()
}

/// Appends a truck mesh to our neutral one, flattening truck's separate
/// position and normal index streams into a single interleaved buffer.
pub(crate) fn append_triangles(out: &mut TriangleMesh, mesh: &PolygonMesh) {
    let positions = mesh.positions();
    let normals = mesh.normals();
    let mut seen: std::collections::HashMap<(usize, Option<usize>), u32> =
        std::collections::HashMap::new();

    for face in mesh.faces().tri_faces() {
        for vertex in face {
            let key = (vertex.pos, vertex.nor);
            let index = *seen.entry(key).or_insert_with(|| {
                let index = out.positions.len() as u32;
                out.positions.push(from_point(positions[vertex.pos]));
                let normal = vertex.nor.and_then(|n| normals.get(n)).unwrap_or(MtVec3::unit_z());
                out.normals.push(from_vector(normal));
                index
            });
            out.indices.push(index);
        }
    }
}

fn to_matrix(t: &Transform3d) -> Matrix4 {
    Matrix4::from_cols(
        MtVec3::new(t.x_axis.x.mm(), t.x_axis.y.mm(), t.x_axis.z.mm()).extend(0.0),
        MtVec3::new(t.y_axis.x.mm(), t.y_axis.y.mm(), t.y_axis.z.mm()).extend(0.0),
        MtVec3::new(t.z_axis.x.mm(), t.z_axis.y.mm(), t.z_axis.z.mm()).extend(0.0),
        to_point(kicase_geometry::types::Point3::ZERO + t.translation).to_homogeneous(),
    )
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| GeometryError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Writes the mesh as binary STL. The format is small enough to emit directly,
/// and doing so keeps the STL identical whichever kernel produced the mesh.
fn write_binary_stl(mesh: &TriangleMesh, path: &Path) -> Result<()> {
    let mut bytes = Vec::with_capacity(84 + mesh.indices.len() / 3 * 50);
    bytes.extend_from_slice(&[0u8; 80]);
    bytes.extend_from_slice(&((mesh.indices.len() / 3) as u32).to_le_bytes());
    let (triangles, _) = mesh.indices.as_chunks::<3>();
    for triangle in triangles {
        let p = triangle.map(|i| mesh.positions[i as usize]);
        let u = p[1] - p[0];
        let v = p[2] - p[0];
        let normal = u.cross(v).normalized().unwrap_or(kicase_geometry::types::Vector3::Z);
        for value in [normal.x, normal.y, normal.z] {
            bytes.extend_from_slice(&(value.mm() as f32).to_le_bytes());
        }
        for point in &p {
            for value in [point.x, point.y, point.z] {
                bytes.extend_from_slice(&(value.mm() as f32).to_le_bytes());
            }
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    std::fs::write(path, bytes)
        .map_err(|source| GeometryError::Io { path: path.display().to_string(), source })
}
