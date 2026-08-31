//! Conversions between neutral geometry and truck topology.

use kicase_geometry::error::{GeometryError, Result};
use kicase_geometry::profile::{Curve2, Loop2};
use kicase_geometry::types::{Plane3, Point2, Point3};
use monstertruck_modeling::builder::CircularArcConstraint;
use monstertruck_modeling::{builder, Point3 as MtPoint3, Vector3 as MtVector3, Wire};

pub(crate) fn to_point(p: Point3) -> MtPoint3 {
    MtPoint3::new(p.x.mm(), p.y.mm(), p.z.mm())
}

pub(crate) fn from_point(p: MtPoint3) -> Point3 {
    Point3::from_mm(p.x, p.y, p.z)
}

pub(crate) fn from_vector(v: MtVector3) -> kicase_geometry::types::Vector3 {
    kicase_geometry::types::Vector3::from_mm(v.x, v.y, v.z)
}

/// Maps a profile-local 2D point onto `plane` and into kernel space.
fn place(plane: &Plane3, p: Point2) -> MtPoint3 {
    to_point(plane.to_world(p))
}

/// Builds a closed truck wire from a neutral loop placed on `plane`.
///
/// Vertices are created once per corner and shared by the two edges that meet
/// there, so the wire is topologically closed rather than merely coincident.
/// Straight segments stay straight and arcs stay analytic circular arcs.
pub(crate) fn loop_to_wire(loop2: &Loop2, plane: &Plane3) -> Result<Wire> {
    let curves = loop2.curves();
    if curves.is_empty() {
        return Err(GeometryError::EmptyContour);
    }
    for (index, curve) in curves.iter().enumerate() {
        if curve.is_degenerate() {
            return Err(GeometryError::DegenerateCurve { curve_index: index });
        }
    }

    let vertices: Vec<_> =
        curves.iter().map(|curve| builder::vertex(place(plane, curve.start()))).collect();

    let mut wire = Wire::new();
    for (index, curve) in curves.iter().enumerate() {
        let from = &vertices[index];
        let to = &vertices[(index + 1) % vertices.len()];
        let edge = match curve {
            Curve2::Line(_) => builder::line(from, to),
            Curve2::Arc(arc) if arc.center().is_some() => {
                let through = CircularArcConstraint::ThroughPoint(place(plane, arc.mid));
                builder::try_circle_arc(from, to, through).map_err(|e| {
                    GeometryError::kernel("circle_arc", format!("curve {index}: {e:?}"))
                })?
            },
            // Three collinear points: a straight edge is the honest reading.
            Curve2::Arc(_) => builder::line(from, to),
        };
        wire.push_back(edge);
    }
    Ok(wire)
}
