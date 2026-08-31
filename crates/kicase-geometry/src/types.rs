//! Neutral geometry primitives.
//!
//! These types are the lingua franca between the KiCad adapter, the semantic
//! enclosure model and the CAD kernel backend. Nothing here knows about KiCad
//! protobufs or OpenCascade.

use crate::units::{mm, Angle, Length};
use serde::{Deserialize, Serialize};
use std::ops::{Add, Mul, Neg, Sub};

/// Default modelling tolerance (10 nm, one hundredth of a KiCad IU).
pub const TOL: Length = mm(1e-5);

/// A 2D vector in the PCB plane.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Vector2 {
    pub x: Length,
    pub y: Length,
}

impl Vector2 {
    pub const ZERO: Vector2 = Vector2 { x: Length::ZERO, y: Length::ZERO };

    #[inline]
    pub const fn new(x: Length, y: Length) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn from_mm(x: f64, y: f64) -> Self {
        Self { x: mm(x), y: mm(y) }
    }

    #[inline]
    pub fn length(self) -> Length {
        mm((self.x.mm() * self.x.mm() + self.y.mm() * self.y.mm()).sqrt())
    }

    #[inline]
    pub fn dot(self, other: Vector2) -> f64 {
        self.x.mm() * other.x.mm() + self.y.mm() * other.y.mm()
    }

    /// 2D cross product (z component of the 3D cross product).
    #[inline]
    pub fn cross(self, other: Vector2) -> f64 {
        self.x.mm() * other.y.mm() - self.y.mm() * other.x.mm()
    }

    /// Unit vector, or `None` for a degenerate (zero-length) vector.
    pub fn normalized(self) -> Option<Vector2> {
        let len = self.length();
        if len.mm() <= TOL.mm() {
            None
        } else {
            Some(Vector2::from_mm(self.x / len, self.y / len))
        }
    }

    /// Rotated 90 degrees counter-clockwise.
    #[inline]
    pub fn perpendicular(self) -> Vector2 {
        Vector2::new(-self.y, self.x)
    }

    #[inline]
    pub fn angle(self) -> Angle {
        Angle::from_radians(self.y.mm().atan2(self.x.mm()))
    }
}

impl Add for Vector2 {
    type Output = Vector2;
    fn add(self, rhs: Vector2) -> Vector2 {
        Vector2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vector2 {
    type Output = Vector2;
    fn sub(self, rhs: Vector2) -> Vector2 {
        Vector2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vector2 {
    type Output = Vector2;
    fn mul(self, rhs: f64) -> Vector2 {
        Vector2::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vector2 {
    type Output = Vector2;
    fn neg(self) -> Vector2 {
        Vector2::new(-self.x, -self.y)
    }
}

/// A point in the PCB plane.
pub type Point2 = Vector2;

/// A 3D vector in enclosure space (X/Y = PCB plane, Z = up).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Vector3 {
    pub x: Length,
    pub y: Length,
    pub z: Length,
}

impl Vector3 {
    pub const ZERO: Vector3 = Vector3 { x: Length::ZERO, y: Length::ZERO, z: Length::ZERO };
    pub const X: Vector3 = Vector3 { x: mm(1.0), y: Length::ZERO, z: Length::ZERO };
    pub const Y: Vector3 = Vector3 { x: Length::ZERO, y: mm(1.0), z: Length::ZERO };
    pub const Z: Vector3 = Vector3 { x: Length::ZERO, y: Length::ZERO, z: mm(1.0) };

    #[inline]
    pub const fn new(x: Length, y: Length, z: Length) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn from_mm(x: f64, y: f64, z: f64) -> Self {
        Self { x: mm(x), y: mm(y), z: mm(z) }
    }

    #[inline]
    pub fn from_2d(p: Vector2, z: Length) -> Self {
        Self { x: p.x, y: p.y, z }
    }

    #[inline]
    pub fn xy(self) -> Vector2 {
        Vector2::new(self.x, self.y)
    }

    #[inline]
    pub fn length(self) -> Length {
        mm((self.x.mm().powi(2) + self.y.mm().powi(2) + self.z.mm().powi(2)).sqrt())
    }

    #[inline]
    pub fn dot(self, other: Vector3) -> f64 {
        self.x.mm() * other.x.mm() + self.y.mm() * other.y.mm() + self.z.mm() * other.z.mm()
    }

    pub fn cross(self, other: Vector3) -> Vector3 {
        Vector3::from_mm(
            self.y.mm() * other.z.mm() - self.z.mm() * other.y.mm(),
            self.z.mm() * other.x.mm() - self.x.mm() * other.z.mm(),
            self.x.mm() * other.y.mm() - self.y.mm() * other.x.mm(),
        )
    }

    pub fn normalized(self) -> Option<Vector3> {
        let len = self.length();
        if len.mm() <= TOL.mm() {
            None
        } else {
            Some(Vector3::from_mm(self.x / len, self.y / len, self.z / len))
        }
    }
}

impl Add for Vector3 {
    type Output = Vector3;
    fn add(self, rhs: Vector3) -> Vector3 {
        Vector3::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vector3 {
    type Output = Vector3;
    fn sub(self, rhs: Vector3) -> Vector3 {
        Vector3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f64> for Vector3 {
    type Output = Vector3;
    fn mul(self, rhs: f64) -> Vector3 {
        Vector3::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Neg for Vector3 {
    type Output = Vector3;
    fn neg(self) -> Vector3 {
        Vector3::new(-self.x, -self.y, -self.z)
    }
}

/// A point in enclosure space.
pub type Point3 = Vector3;

/// A straight segment between two points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineSegment2 {
    pub start: Point2,
    pub end: Point2,
}

impl LineSegment2 {
    pub const fn new(start: Point2, end: Point2) -> Self {
        Self { start, end }
    }

    pub fn direction(&self) -> Option<Vector2> {
        (self.end - self.start).normalized()
    }

    pub fn length(&self) -> Length {
        (self.end - self.start).length()
    }

    pub fn midpoint(&self) -> Point2 {
        Point2::new((self.start.x + self.end.x) / 2.0, (self.start.y + self.end.y) / 2.0)
    }
}

/// A circular arc defined by start / mid / end, matching KiCad's own
/// representation. Keeping the three-point form avoids any ambiguity about
/// sweep direction and is directly consumable by OpenCascade.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arc2 {
    pub start: Point2,
    pub mid: Point2,
    pub end: Point2,
}

impl Arc2 {
    pub const fn new(start: Point2, mid: Point2, end: Point2) -> Self {
        Self { start, mid, end }
    }

    /// Circumcentre of the three defining points, or `None` if they are
    /// collinear (a degenerate arc).
    pub fn center(&self) -> Option<Point2> {
        let (ax, ay) = (self.start.x.mm(), self.start.y.mm());
        let (bx, by) = (self.mid.x.mm(), self.mid.y.mm());
        let (cx, cy) = (self.end.x.mm(), self.end.y.mm());
        let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
        if d.abs() < 1e-12 {
            return None;
        }
        let a2 = ax * ax + ay * ay;
        let b2 = bx * bx + by * by;
        let c2 = cx * cx + cy * cy;
        let ux = (a2 * (by - cy) + b2 * (cy - ay) + c2 * (ay - by)) / d;
        let uy = (a2 * (cx - bx) + b2 * (ax - cx) + c2 * (bx - ax)) / d;
        Some(Point2::from_mm(ux, uy))
    }

    pub fn radius(&self) -> Option<Length> {
        self.center().map(|c| (self.start - c).length())
    }

    /// True when the arc sweeps counter-clockwise from `start` to `end`.
    pub fn is_ccw(&self) -> bool {
        (self.mid - self.start).cross(self.end - self.mid) > 0.0
    }

    /// Samples the arc into `segments` chords. Used for area/orientation
    /// computations and for the OpenSCAD derivative export; the B-rep pipeline
    /// keeps the analytic arc.
    pub fn tessellate(&self, segments: usize) -> Vec<Point2> {
        let segments = segments.max(2);
        let Some(center) = self.center() else {
            return vec![self.start, self.end];
        };
        let start_a = (self.start - center).angle().radians();
        let end_a = (self.end - center).angle().radians();
        let radius = (self.start - center).length();
        let mut sweep = end_a - start_a;
        let tau = std::f64::consts::TAU;
        if self.is_ccw() {
            while sweep <= 0.0 {
                sweep += tau;
            }
        } else {
            while sweep >= 0.0 {
                sweep -= tau;
            }
        }
        (0..=segments)
            .map(|i| {
                let t = start_a + sweep * (i as f64 / segments as f64);
                Point2::new(center.x + radius * t.cos(), center.y + radius * t.sin())
            })
            .collect()
    }
}

/// A full circle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Circle2 {
    pub center: Point2,
    pub radius: Length,
}

impl Circle2 {
    pub const fn new(center: Point2, radius: Length) -> Self {
        Self { center, radius }
    }

    /// Splits the circle into two half arcs, so it can live inside a generic
    /// closed loop of curves.
    pub fn to_arcs(&self) -> [Arc2; 2] {
        let r = self.radius;
        let c = self.center;
        let right = Point2::new(c.x + r, c.y);
        let top = Point2::new(c.x, c.y + r);
        let left = Point2::new(c.x - r, c.y);
        let bottom = Point2::new(c.x, c.y - r);
        [Arc2::new(right, top, left), Arc2::new(left, bottom, right)]
    }
}

/// A closed polygon (implicitly closed: last point connects to first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Polygon2 {
    pub points: Vec<Point2>,
}

impl Polygon2 {
    pub fn new(points: Vec<Point2>) -> Self {
        Self { points }
    }

    /// Signed area; positive when the winding is counter-clockwise.
    pub fn signed_area(&self) -> f64 {
        let n = self.points.len();
        if n < 3 {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            sum += a.x.mm() * b.y.mm() - b.x.mm() * a.y.mm();
        }
        sum / 2.0
    }

    pub fn is_ccw(&self) -> bool {
        self.signed_area() > 0.0
    }

    pub fn make_ccw(&mut self) {
        if !self.is_ccw() {
            self.points.reverse();
        }
    }

    pub fn bounds(&self) -> Option<Bounds2> {
        Bounds2::from_points(self.points.iter().copied())
    }

    /// True when `point` lies inside the polygon, by the even-odd rule.
    ///
    /// Used to ask whether a drawn shape sits over a particular place on the
    /// board — a standoff over a mounting hole, say.
    pub fn contains(&self, point: Point2) -> bool {
        let n = self.points.len();
        if n < 3 {
            return false;
        }
        let (px, py) = (point.x.mm(), point.y.mm());
        let mut inside = false;
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = (self.points[i].x.mm(), self.points[i].y.mm());
            let (xj, yj) = (self.points[j].x.mm(), self.points[j].y.mm());
            if (yi > py) != (yj > py) {
                let x_at = xi + (py - yi) / (yj - yi) * (xj - xi);
                if x_at > px {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }
}

/// An axis-aligned 2D bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds2 {
    pub min: Point2,
    pub max: Point2,
}

impl Bounds2 {
    pub fn from_points(points: impl IntoIterator<Item = Point2>) -> Option<Self> {
        let mut iter = points.into_iter();
        let first = iter.next()?;
        let mut bounds = Bounds2 { min: first, max: first };
        for p in iter {
            bounds.min.x = bounds.min.x.min(p.x);
            bounds.min.y = bounds.min.y.min(p.y);
            bounds.max.x = bounds.max.x.max(p.x);
            bounds.max.y = bounds.max.y.max(p.y);
        }
        Some(bounds)
    }

    pub fn width(&self) -> Length {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> Length {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Point2 {
        Point2::new((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }

    pub fn expanded(&self, by: Length) -> Bounds2 {
        Bounds2 {
            min: Point2::new(self.min.x - by, self.min.y - by),
            max: Point2::new(self.max.x + by, self.max.y + by),
        }
    }

    pub fn union(&self, other: &Bounds2) -> Bounds2 {
        Bounds2 {
            min: Point2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Point2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }
}

/// An axis-aligned 3D bounding box, used by geometry tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds3 {
    pub min: Point3,
    pub max: Point3,
}

impl Bounds3 {
    pub fn size(&self) -> Vector3 {
        self.max - self.min
    }
}

/// A plane in 3D, defined by an origin and two orthonormal in-plane axes.
///
/// For a side datum: `u` runs along the datum line, `v` is world Z, and the
/// normal is the horizontal wall normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane3 {
    pub origin: Point3,
    pub u: Vector3,
    pub v: Vector3,
}

impl Plane3 {
    pub fn new(origin: Point3, u: Vector3, v: Vector3) -> Self {
        Self { origin, u, v }
    }

    /// The XY plane at `z`.
    pub fn xy_at(z: Length) -> Self {
        Self { origin: Point3::new(Length::ZERO, Length::ZERO, z), u: Vector3::X, v: Vector3::Y }
    }

    pub fn normal(&self) -> Vector3 {
        self.u.cross(self.v).normalized().unwrap_or(Vector3::Z)
    }

    /// Maps a point expressed in plane-local `(u, v)` coordinates into world space.
    pub fn to_world(&self, local: Point2) -> Point3 {
        self.origin + self.u * local.x.mm() + self.v * local.y.mm()
    }

    /// Maps a world point into plane-local `(u, v)` coordinates.
    pub fn to_local(&self, world: Point3) -> Point2 {
        let d = world - self.origin;
        Point2::from_mm(d.dot(self.u), d.dot(self.v))
    }
}

/// A rigid transform: rotation (as three basis vectors) followed by translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform3d {
    pub x_axis: Vector3,
    pub y_axis: Vector3,
    pub z_axis: Vector3,
    pub translation: Vector3,
}

impl Transform3d {
    pub const IDENTITY: Transform3d = Transform3d {
        x_axis: Vector3::X,
        y_axis: Vector3::Y,
        z_axis: Vector3::Z,
        translation: Vector3::ZERO,
    };

    pub fn translation(offset: Vector3) -> Self {
        Transform3d { translation: offset, ..Transform3d::IDENTITY }
    }

    /// Transform that maps the XY plane onto `plane` (local X to `plane.u`,
    /// local Y to `plane.v`, local Z to the plane normal).
    pub fn from_plane(plane: &Plane3) -> Self {
        Transform3d {
            x_axis: plane.u,
            y_axis: plane.v,
            z_axis: plane.normal(),
            translation: plane.origin,
        }
    }

    pub fn apply(&self, point: Point3) -> Point3 {
        self.x_axis * point.x.mm()
            + self.y_axis * point.y.mm()
            + self.z_axis * point.z.mm()
            + self.translation
    }

    /// Applies only the rotational part.
    pub fn apply_vector(&self, vector: Vector3) -> Vector3 {
        self.x_axis * vector.x.mm() + self.y_axis * vector.y.mm() + self.z_axis * vector.z.mm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_center_and_radius() {
        // Quarter arc of a unit circle centred at the origin.
        let mid = std::f64::consts::FRAC_1_SQRT_2;
        let arc = Arc2::new(
            Point2::from_mm(1.0, 0.0),
            Point2::from_mm(mid, mid),
            Point2::from_mm(0.0, 1.0),
        );
        let c = arc.center().expect("non-collinear points have a centre");
        assert!(c.x.mm().abs() < 1e-6 && c.y.mm().abs() < 1e-6);
        assert!((arc.radius().unwrap().mm() - 1.0).abs() < 1e-6);
        assert!(arc.is_ccw());
    }

    #[test]
    fn collinear_arc_has_no_center() {
        let arc = Arc2::new(
            Point2::from_mm(0.0, 0.0),
            Point2::from_mm(1.0, 0.0),
            Point2::from_mm(2.0, 0.0),
        );
        assert!(arc.center().is_none());
    }

    #[test]
    fn polygon_orientation() {
        let mut ccw = Polygon2::new(vec![
            Point2::from_mm(0.0, 0.0),
            Point2::from_mm(2.0, 0.0),
            Point2::from_mm(2.0, 1.0),
            Point2::from_mm(0.0, 1.0),
        ]);
        assert!(ccw.is_ccw());
        assert!((ccw.signed_area() - 2.0).abs() < 1e-9);
        ccw.points.reverse();
        assert!(!ccw.is_ccw());
        ccw.make_ccw();
        assert!(ccw.is_ccw());
    }

    #[test]
    fn polygon_containment() {
        let square = Polygon2::new(vec![
            Point2::from_mm(0.0, 0.0),
            Point2::from_mm(10.0, 0.0),
            Point2::from_mm(10.0, 10.0),
            Point2::from_mm(0.0, 10.0),
        ]);
        assert!(square.contains(Point2::from_mm(5.0, 5.0)));
        assert!(!square.contains(Point2::from_mm(15.0, 5.0)));
        assert!(!square.contains(Point2::from_mm(5.0, -1.0)));
        // A concave shape must not report its notch as inside.
        let l_shape = Polygon2::new(vec![
            Point2::from_mm(0.0, 0.0),
            Point2::from_mm(10.0, 0.0),
            Point2::from_mm(10.0, 4.0),
            Point2::from_mm(4.0, 4.0),
            Point2::from_mm(4.0, 10.0),
            Point2::from_mm(0.0, 10.0),
        ]);
        assert!(l_shape.contains(Point2::from_mm(2.0, 8.0)));
        assert!(!l_shape.contains(Point2::from_mm(8.0, 8.0)));
    }

    #[test]
    fn plane_round_trip() {
        // A datum plane: u along +X, v along +Z, normal along -Y.
        let plane = Plane3::new(Point3::from_mm(1.0, 2.0, 3.0), Vector3::X, Vector3::Z);
        let world = plane.to_world(Point2::from_mm(4.0, 5.0));
        assert_eq!(world, Point3::from_mm(5.0, 2.0, 8.0));
        let local = plane.to_local(world);
        assert!((local.x.mm() - 4.0).abs() < 1e-9);
        assert!((local.y.mm() - 5.0).abs() < 1e-9);
    }

    #[test]
    fn arc_tessellation_stays_on_circle() {
        let arc = Arc2::new(
            Point2::from_mm(1.0, 0.0),
            Point2::from_mm(0.0, 1.0),
            Point2::from_mm(-1.0, 0.0),
        );
        for p in arc.tessellate(16) {
            let r = (p.x.mm().powi(2) + p.y.mm().powi(2)).sqrt();
            assert!((r - 1.0).abs() < 1e-9, "point off circle: {r}");
        }
    }
}

/// A triangulated surface, for display only.
///
/// The B-rep stays canonical; this is what a renderer needs. Positions and
/// normals are parallel arrays, and `indices` holds triangle corners.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriangleMesh {
    pub positions: Vec<Point3>,
    pub normals: Vec<Vector3>,
    pub indices: Vec<u32>,
}

impl TriangleMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn bounds(&self) -> Option<Bounds3> {
        let first = *self.positions.first()?;
        let mut bounds = Bounds3 { min: first, max: first };
        for p in &self.positions {
            bounds.min.x = bounds.min.x.min(p.x);
            bounds.min.y = bounds.min.y.min(p.y);
            bounds.min.z = bounds.min.z.min(p.z);
            bounds.max.x = bounds.max.x.max(p.x);
            bounds.max.y = bounds.max.y.max(p.y);
            bounds.max.z = bounds.max.z.max(p.z);
        }
        Some(bounds)
    }
}
