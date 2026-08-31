//! Conversions between neutral geometry and OpenCascade topology.

use glam::{dvec3, DVec3};
use kicase_geometry::error::{GeometryError, Result};
use kicase_geometry::profile::{Curve2, Loop2};
use kicase_geometry::types::{Plane3, Point2, Point3};
use opencascade::primitives::{Edge, Wire};

/// Neutral 3D point to an OpenCascade-friendly vector (millimetres).
pub(crate) fn to_dvec3(p: Point3) -> DVec3 {
    dvec3(p.x.mm(), p.y.mm(), p.z.mm())
}

pub(crate) fn from_dvec3(v: DVec3) -> Point3 {
    Point3::from_mm(v.x, v.y, v.z)
}

/// Maps a profile-local 2D point onto `plane` and into kernel space.
fn place(plane: &Plane3, p: Point2) -> DVec3 {
    to_dvec3(plane.to_world(p))
}

/// Builds a kernel wire from a neutral closed loop placed on `plane`.
///
/// Straight segments stay straight and arcs stay analytic circular arcs; no
/// polygonization happens here.
pub(crate) fn loop_to_wire(loop2: &Loop2, plane: &Plane3) -> Result<Wire> {
    let mut edges = Vec::with_capacity(loop2.curves().len());

    for (index, curve) in loop2.curves().iter().enumerate() {
        if curve.is_degenerate() {
            return Err(GeometryError::DegenerateCurve { curve_index: index });
        }
        let edge = match curve {
            Curve2::Line(line) => Edge::segment(place(plane, line.start), place(plane, line.end)),
            Curve2::Arc(arc) => {
                if arc.center().is_none() {
                    // Three collinear points: a straight edge is the honest
                    // interpretation, and OCC would reject the arc.
                    Edge::segment(place(plane, arc.start), place(plane, arc.end))
                } else {
                    Edge::arc(place(plane, arc.start), place(plane, arc.mid), place(plane, arc.end))
                }
            },
        };
        edges.push(edge);
    }

    if edges.is_empty() {
        return Err(GeometryError::EmptyContour);
    }

    Ok(Wire::from_edges(edges.iter()))
}
