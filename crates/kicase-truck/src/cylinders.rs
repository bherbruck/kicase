//! Drawn corners, handed to the engine as the cylinders they are.
//!
//! This is a workaround for monstertruck 0.4, and it lives here because it is
//! about that engine and nothing else. `builder::extrude` sweeps a circular arc
//! by translation, and the only surface it can name for the result is a
//! rational B-spline: `ToSameGeometry for ExtrusionSurface` turns an extruded
//! `Line` into an analytic `Plane` but an extruded arc into a
//! `NurbsSurface`. Everything downstream then pays for that. Tessellating a
//! NURBS patch needs a subdivision search where a cylinder's is closed form,
//! and worse, every sample of every intersection curve calls
//! `search_nearest_parameter` on it with no hint, which runs a 51 x 51 grid of
//! rational evaluations. That one chain is most of the cost of a boolean on an
//! enclosure with drawn corners.
//!
//! The surface is the only thing wrong: the face's own edges are already exact
//! circles and lines. So the face keeps its topology and is simply told what
//! surface it was always on.

use monstertruck_modeling::cgmath::InnerSpace;
use monstertruck_modeling::{
    Curve, Face, Line, ParametricCurve, ParametricSurface, ParametricSurface3D, Point3 as MtPoint3,
    Processor, RevolutionSurface, Solid, Surface, Vector3 as MtVec3,
};

/// How far, as a fraction of the radius, a face's own boundary may sit from the
/// circle fitted through it before the fit is disbelieved.
///
/// The arc is already an exact circle, so this only has to be above the noise
/// of fitting one through three of its points. Anything it rejects keeps the
/// surface truck gave it and is merely slow.
const FIT_TOLERANCE: f64 = 1.0e-9;

/// Re-surfaces every arc-swept face of a freshly extruded shell.
///
/// `sweep` is the vector the profile was swept along; the profile is planar and
/// perpendicular to it, so that vector is also the axis every corner turns
/// about. Anything this cannot read with certainty is left exactly as truck
/// built it, so the worst case is the speed we had before.
pub(crate) fn make_arc_faces_analytic(solid: &Solid, sweep: MtVec3) {
    let Some(normal) = unit(sweep) else { return };
    for shell in solid.boundaries() {
        for face in shell.face_iter() {
            if matches!(face.surface(), Surface::Plane(_)) {
                continue;
            }
            if let Some(surface) = cylinder_through(face, normal) {
                face.set_surface(surface);
            }
        }
    }
}

/// The cylinder a face's own boundary lies on, or `None` when its boundary does
/// not describe one.
fn cylinder_through(face: &Face, normal: MtVec3) -> Option<Surface> {
    let edges: Vec<_> = face.boundary_iters().into_iter().flatten().collect();

    // The two arcs are the swept ends of one drawn curve and the two straight
    // edges are the sweep itself, which is where the face's extent along the
    // axis comes from.
    let arc = edges.iter().find(|edge| !matches!(edge.curve(), Curve::Line(_)))?.curve();
    let rail = edges.iter().find(|edge| matches!(edge.curve(), Curve::Line(_)))?.curve();
    let Curve::Line(rail) = rail else { return None };

    let (t0, t1) = arc.try_range_tuple()?;
    let (start, mid, end) = (arc.subs(t0), arc.subs((t0 + t1) / 2.0), arc.subs(t1));
    let centre = circumcentre(start, mid, end)?;
    let radius = (start - centre).magnitude();
    let slack = radius * FIT_TOLERANCE;
    if ((mid - centre).magnitude() - radius).abs() > slack
        || ((end - centre).magnitude() - radius).abs() > slack
    {
        return None;
    }

    // Signed so that the parameter runs forward along the arc as drawn, which
    // is what keeps the face's own edges inside the surface's domain.
    let turning = (start - centre).cross(mid - centre);
    let axis = if turning.dot(normal) < 0.0 { -normal } else { normal };
    let sweep = angle_about(axis, start - centre, end - centre);

    // The surface's parameter wraps at 0 == 2 pi, and a boundary point landing
    // on that seam leaves the boolean unable to close its intersection loop.
    // Starting the revolution half the leftover angle back from the arc puts
    // the seam as far from both of its ends as it can be.
    let offset = (std::f64::consts::TAU - sweep) / 2.0;
    if offset <= f64::EPSILON {
        return None;
    }
    let radial = rotate(start - centre, axis, -offset);

    // The generatrix spans the same stretch of the axis the face does.
    let along = |p: MtPoint3| centre + (p - centre).dot(normal) * normal + radial;
    let (foot, head) = (along(rail.0), along(rail.1));
    if (head - foot).magnitude2() <= f64::EPSILON {
        return None;
    }

    // Which way the generatrix runs decides which side of the surface is
    // outward, and that has to match the side the face was already showing.
    let candidate = |from: MtPoint3, to: MtPoint3| {
        RevolutionSurface::by_revolution(Curve::Line(Line(from, to)), centre, axis)
    };
    let facing = candidate(foot, head);
    let outward = facing.normal(0.5, offset + sweep / 2.0);
    let outward = if face.orientation() { outward } else { -outward };
    let surface = if outward.dot(face_normal(face)) < 0.0 { candidate(head, foot) } else { facing };
    Some(Surface::RevolutionSurface(Processor::new(surface)))
}

/// The outward normal the face already shows, at the middle of its surface.
fn face_normal(face: &Face) -> MtVec3 {
    let surface = face.oriented_surface();
    let (u, v) = surface.try_range_tuple();
    let mid = |range: Option<(f64, f64)>| range.map_or(0.5, |(a, b)| (a + b) / 2.0);
    surface.normal(mid(u), mid(v))
}

/// Centre of the circle through three points, or `None` when they are
/// collinear.
fn circumcentre(a: MtPoint3, b: MtPoint3, c: MtPoint3) -> Option<MtPoint3> {
    let (ab, ac) = (b - a, c - a);
    let plane = ab.cross(ac);
    let scale = 2.0 * plane.magnitude2();
    if scale <= f64::EPSILON {
        return None;
    }
    Some(a + (ab.magnitude2() * ac - ac.magnitude2() * ab).cross(plane) / scale)
}

/// Angle from `from` to `to` about `axis`, in `0..2 * PI`.
fn angle_about(axis: MtVec3, from: MtVec3, to: MtVec3) -> f64 {
    let (from, to) = (from.normalize(), to.normalize());
    let angle = from.dot(to).clamp(-1.0, 1.0).acos();
    if from.cross(to).dot(axis) < 0.0 {
        std::f64::consts::TAU - angle
    } else {
        angle
    }
}

/// Rodrigues' rotation of `v` about a unit `axis`.
fn rotate(v: MtVec3, axis: MtVec3, angle: f64) -> MtVec3 {
    let (sin, cos) = angle.sin_cos();
    v * cos + axis.cross(v) * sin + axis * (axis.dot(v) * (1.0 - cos))
}

fn unit(v: MtVec3) -> Option<MtVec3> {
    (v.magnitude2() > f64::EPSILON).then(|| v.normalize())
}

#[cfg(test)]
mod tests {
    use crate::TruckKernel;
    use kicase_geometry::kernel::CadKernel;
    use kicase_geometry::profile::{Loop2, Profile2d};
    use kicase_geometry::types::{Plane3, Point2};
    use kicase_geometry::units::{mm, Length};
    use monstertruck_modeling::Surface;

    /// Every drawn corner has to reach the engine as the cylinder it is. A
    /// corner that arrives as a rational B-spline still builds the right part,
    /// so nothing else in the suite notices — it just costs several times as
    /// much in every boolean it takes part in.
    #[test]
    fn an_extruded_arc_is_a_cylinder_and_not_a_nurbs_patch() {
        let kernel = TruckKernel::new();
        let outline = Loop2::rounded_rectangle(
            Point2::from_mm(0.0, 0.0),
            Point2::from_mm(40.0, 24.0),
            mm(6.0),
        );
        let profile = kernel
            .make_profile(&Profile2d::simple(outline), &Plane3::xy_at(Length::ZERO))
            .expect("a rounded rectangle is a profile");
        let solid = kernel.extrude(&profile, mm(13.0)).expect("extrudes");

        let mut cylinders = 0;
        for body in solid.bodies() {
            for shell in body.boundaries() {
                for face in shell.face_iter() {
                    match face.surface() {
                        Surface::Plane(_) => {},
                        Surface::RevolutionSurface(_) => cylinders += 1,
                        other => panic!("a swept arc came through as {other:?}"),
                    }
                }
            }
        }
        assert_eq!(cylinders, 4, "one cylinder per drawn corner");

        // And the cylinders bound the same material the arcs did: a surface
        // facing the wrong way would still be analytic and quite wrong.
        let area = 40.0 * 24.0 - (4.0 - std::f64::consts::PI) * 36.0;
        let expected = area * 13.0;
        let volume = kernel.volume(&solid).expect("measurable");
        assert!(
            (volume - expected).abs() / expected < 1e-3,
            "volume was {volume}, expected {expected}"
        );
    }
}
