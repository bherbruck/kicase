//! Closed contour assembly.
//!
//! KiCad stores board outlines as an unordered soup of independent lines, arcs,
//! circles and rectangles. This module chains them into closed loops and
//! reports precisely where a contour fails to close.

use crate::error::{GeometryError, Result};
use crate::types::{Arc2, Bounds2, Circle2, LineSegment2, Point2, Polygon2, Vector2, TOL};
use crate::units::{mm, Length};
use serde::{Deserialize, Serialize};

/// Tolerance used when deciding whether two endpoints are the same point.
///
/// Real drawings do not close to the nanometre. Arc endpoints that KiCad
/// computed — from a fillet, or a rounded rectangle drawn by hand — routinely
/// land tens of microns apart, and no one can see or fix that. 50 um is far
/// below anything anyone draws on purpose and far above the noise, so a
/// contour that looks closed is treated as closed.
pub const JOIN_TOL: Length = mm(0.05);

/// Segments per arc when checking that an offset still stands off its source.
///
/// Fine enough that the sag of a tessellated corner stays far under the half-
/// offset threshold [`Loop2::stands_off`] measures against.
const STANDOFF_SAMPLES: usize = 32;

/// One element of a contour.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Curve2 {
    Line(LineSegment2),
    Arc(Arc2),
}

impl Curve2 {
    pub fn line(start: Point2, end: Point2) -> Self {
        Curve2::Line(LineSegment2::new(start, end))
    }

    pub fn arc(start: Point2, mid: Point2, end: Point2) -> Self {
        Curve2::Arc(Arc2::new(start, mid, end))
    }

    pub fn start(&self) -> Point2 {
        match self {
            Curve2::Line(l) => l.start,
            Curve2::Arc(a) => a.start,
        }
    }

    pub fn end(&self) -> Point2 {
        match self {
            Curve2::Line(l) => l.end,
            Curve2::Arc(a) => a.end,
        }
    }

    pub fn reversed(&self) -> Curve2 {
        match self {
            Curve2::Line(l) => Curve2::Line(LineSegment2::new(l.end, l.start)),
            Curve2::Arc(a) => Curve2::Arc(Arc2::new(a.end, a.mid, a.start)),
        }
    }

    /// True when start and end coincide and the curve carries no area.
    pub fn is_degenerate(&self) -> bool {
        match self {
            Curve2::Line(l) => l.length().mm() <= TOL.mm(),
            Curve2::Arc(a) => a.center().is_none() && (a.end - a.start).length().mm() <= TOL.mm(),
        }
    }

    /// Points approximating this curve, excluding the end point (so segments
    /// can be concatenated without duplicates).
    pub fn tessellate(&self, arc_segments: usize) -> Vec<Point2> {
        match self {
            Curve2::Line(l) => vec![l.start],
            Curve2::Arc(a) => {
                let mut pts = a.tessellate(arc_segments);
                pts.pop();
                pts
            },
        }
    }
}

impl Curve2 {
    /// The same curve with its start moved to `point`.
    ///
    /// Used to snap a chain together: curves that merely land within tolerance
    /// leave microscopic gaps, and a CAD kernel handed a wire with gaps in it
    /// produces self-intersecting nonsense rather than an error.
    pub fn with_start(&self, point: Point2) -> Curve2 {
        match self {
            Curve2::Line(line) => Curve2::line(point, line.end),
            Curve2::Arc(arc) => Curve2::arc(point, arc.mid, arc.end),
        }
    }

    /// The same curve with its end moved to `point`.
    pub fn with_end(&self, point: Point2) -> Curve2 {
        match self {
            Curve2::Line(line) => Curve2::line(line.start, point),
            Curve2::Arc(arc) => Curve2::arc(arc.start, arc.mid, point),
        }
    }

    /// Unit tangent leaving the start of the curve.
    pub fn tangent_at_start(&self) -> Option<Vector2> {
        match self {
            Curve2::Line(line) => line.direction(),
            Curve2::Arc(arc) => {
                let centre = arc.center()?;
                let radial = (arc.start - centre).normalized()?;
                let t = radial.perpendicular();
                Some(if arc.is_ccw() { t } else { -t })
            },
        }
    }

    /// Unit tangent arriving at the end of the curve.
    pub fn tangent_at_end(&self) -> Option<Vector2> {
        match self {
            Curve2::Line(line) => line.direction(),
            Curve2::Arc(arc) => {
                let centre = arc.center()?;
                let radial = (arc.end - centre).normalized()?;
                let t = radial.perpendicular();
                Some(if arc.is_ccw() { t } else { -t })
            },
        }
    }

    /// The closed outline of this curve drawn `width` thick, centred on it.
    ///
    /// This is what a KiCad stroke actually covers: a line becomes a rectangle,
    /// an arc becomes an annular sector. Building the wall from per-curve
    /// strokes is what lets each drawn segment carry its own thickness.
    pub fn stroke_loop(&self, width: Length) -> Option<Loop2> {
        if !width.is_positive() {
            return None;
        }
        let half = width / 2.0;

        match self {
            Curve2::Line(line) => {
                let dir = line.direction()?;
                let n = dir.perpendicular();
                let out = Point2::new(n.x * half.mm(), n.y * half.mm());
                let a = line.start + out;
                let b = line.end + out;
                let c = line.end - out;
                let d = line.start - out;
                Loop2::from_ordered(vec![
                    Curve2::line(a, b),
                    Curve2::line(b, c),
                    Curve2::line(c, d),
                    Curve2::line(d, a),
                ])
                .ok()
            },
            Curve2::Arc(arc) => {
                let centre = arc.center()?;
                let radius = arc.radius()?;
                if radius <= half {
                    // The stroke would swallow its own centre; a sector is not
                    // a meaningful shape here.
                    return None;
                }
                let scale = |p: Point2, r: Length| -> Point2 {
                    let d = p - centre;
                    let len = d.length();
                    if len.mm() <= TOL.mm() {
                        p
                    } else {
                        Point2::new(centre.x + d.x * (r / len), centre.y + d.y * (r / len))
                    }
                };
                let (outer_r, inner_r) = (radius + half, radius - half);
                let outer = Arc2::new(
                    scale(arc.start, outer_r),
                    scale(arc.mid, outer_r),
                    scale(arc.end, outer_r),
                );
                let inner = Arc2::new(
                    scale(arc.end, inner_r),
                    scale(arc.mid, inner_r),
                    scale(arc.start, inner_r),
                );
                Loop2::from_ordered(vec![
                    Curve2::Arc(outer),
                    Curve2::line(outer.end, inner.start),
                    Curve2::Arc(inner),
                    Curve2::line(inner.end, outer.start),
                ])
                .ok()
            },
        }
    }
}

/// Moves one curve along its own normal. Positive is outward for a loop that
/// runs counter-clockwise.
fn offset_curve(curve: &Curve2, distance: Length) -> Option<Curve2> {
    match curve {
        Curve2::Line(line) => {
            let direction = line.direction()?;
            // Interior is to the left when running counter-clockwise, so
            // outward is to the right.
            let outward = -direction.perpendicular();
            let shift = outward * distance.mm();
            Some(Curve2::line(line.start + shift, line.end + shift))
        },
        Curve2::Arc(arc) => {
            let centre = arc.center()?;
            let radius = arc.radius()?;
            // Running counter-clockwise, a left-turning arc curves away from
            // its centre, so growing the loop grows its radius. A right-turning
            // arc is a concave corner and does the opposite.
            let new_radius = if arc.is_ccw() { radius + distance } else { radius - distance };
            if !new_radius.is_positive() {
                return None;
            }
            let scale = |p: Point2| -> Point2 {
                let d = p - centre;
                let len = d.length();
                if len.mm() <= TOL.mm() {
                    p
                } else {
                    Point2::new(
                        centre.x + d.x * (new_radius / len),
                        centre.y + d.y * (new_radius / len),
                    )
                }
            };
            Some(Curve2::arc(scale(arc.start), scale(arc.mid), scale(arc.end)))
        },
    }
}

/// Where two lines would cross if extended, if they are not parallel.
fn line_intersection(a: &LineSegment2, b: &LineSegment2) -> Option<Point2> {
    let da = a.direction()?;
    let db = b.direction()?;
    let denominator = da.cross(db);
    if denominator.abs() < 1e-9 {
        return None;
    }
    let offset = b.start - a.start;
    let t = offset.cross(db) / denominator;
    Some(a.start + da * t)
}

/// Fills the wedge left between two stroked segments that meet at a corner.
///
/// Two strokes meeting at an angle leave a notch on the outside of the turn:
/// each one stops square at the shared point. This builds the mitre that closes
/// it, so a drawn corner comes out sharp rather than nicked.
pub fn miter_join(
    incoming: &Curve2,
    outgoing: &Curve2,
    width_in: Length,
    width_out: Length,
) -> Option<Loop2> {
    if !width_in.is_positive() || !width_out.is_positive() {
        return None;
    }
    let vertex = incoming.end();
    let t_in = incoming.tangent_at_end()?;
    let t_out = outgoing.tangent_at_start()?;

    // Straight through, or tangent — a line meeting an arc smoothly, which is
    // what every rounded corner is made of. There is no notch to fill, and
    // trying produces a sliver with zero-length edges.
    let turn = t_in.cross(t_out);
    if turn.abs() <= 1e-4 {
        return None;
    }

    // The notch is on the outside of the turn.
    let side = if turn > 0.0 { -1.0 } else { 1.0 };
    let n_in = t_in.perpendicular() * side;
    let n_out = t_out.perpendicular() * side;

    let a = vertex + n_in * (width_in.mm() / 2.0);
    let b = vertex + n_out * (width_out.mm() / 2.0);

    // Where the two outer edges would meet if extended.
    let denominator = t_in.cross(t_out);
    let diff = b - a;
    let t = diff.cross(t_out) / denominator;
    if !t.is_finite() {
        return None;
    }
    let apex = a + t_in * t;

    // A mitre that runs away is a near-tangent junction in disguise; clamp it
    // rather than emitting a spike.
    let reach = (apex - vertex).length();
    let widest = width_in.max(width_out);
    if reach > widest * 8.0 {
        return None;
    }

    // Drop any edge that came out degenerate, and give up if that leaves less
    // than a triangle.
    let corners = [vertex, a, apex, b];
    let mut curves = Vec::with_capacity(4);
    for index in 0..corners.len() {
        let from = corners[index];
        let to = corners[(index + 1) % corners.len()];
        if (to - from).length() > TOL {
            curves.push(Curve2::line(from, to));
        }
    }
    if curves.len() < 3 {
        return None;
    }
    Loop2::from_ordered(curves).ok()
}

/// A closed chain of curves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Loop2 {
    curves: Vec<Curve2>,
}

impl Loop2 {
    /// Builds a loop from curves that are already in order and closed.
    pub fn from_ordered(curves: Vec<Curve2>) -> Result<Self> {
        if curves.is_empty() {
            return Err(GeometryError::EmptyContour);
        }
        let loop2 = Loop2 { curves };
        if !loop2.is_closed(JOIN_TOL) {
            let last = loop2.curves.last().expect("non-empty").end();
            return Err(GeometryError::OpenContour {
                at: last,
                curve_index: loop2.curves.len() - 1,
                nearest_gap: Some((loop2.curves[0].start() - last).length()),
            });
        }
        Ok(loop2)
    }

    /// A rectangle, as four line segments (counter-clockwise).
    pub fn rectangle(min: Point2, max: Point2) -> Self {
        let p = [min, Point2::new(max.x, min.y), max, Point2::new(min.x, max.y)];
        Loop2 {
            curves: vec![
                Curve2::line(p[0], p[1]),
                Curve2::line(p[1], p[2]),
                Curve2::line(p[2], p[3]),
                Curve2::line(p[3], p[0]),
            ],
        }
    }

    /// A rectangle with rounded corners, as KiCad's `gr_rect` draws one when it
    /// carries a `radius`.
    ///
    /// The radius is clamped to half the shorter side, which is what KiCad
    /// itself does; a zero or negative radius gives a plain rectangle.
    pub fn rounded_rectangle(min: Point2, max: Point2, radius: Length) -> Self {
        let width = max.x - min.x;
        let height = max.y - min.y;
        let limit = width.min(height) / 2.0;
        let r = radius.min(limit);
        if !r.is_positive() {
            return Loop2::rectangle(min, max);
        }

        // Each corner is a quarter arc; its midpoint sits at the 45 degree
        // point, r - r/sqrt(2) in from the corner along both axes.
        let k = r - r * std::f64::consts::FRAC_1_SQRT_2;
        let (x0, y0, x1, y1) = (min.x, min.y, max.x, max.y);
        let p = |x: Length, y: Length| Point2::new(x, y);

        Loop2 {
            curves: vec![
                Curve2::line(p(x0 + r, y0), p(x1 - r, y0)),
                Curve2::arc(p(x1 - r, y0), p(x1 - k, y0 + k), p(x1, y0 + r)),
                Curve2::line(p(x1, y0 + r), p(x1, y1 - r)),
                Curve2::arc(p(x1, y1 - r), p(x1 - k, y1 - k), p(x1 - r, y1)),
                Curve2::line(p(x1 - r, y1), p(x0 + r, y1)),
                Curve2::arc(p(x0 + r, y1), p(x0 + k, y1 - k), p(x0, y1 - r)),
                Curve2::line(p(x0, y1 - r), p(x0, y0 + r)),
                Curve2::arc(p(x0, y0 + r), p(x0 + k, y0 + k), p(x0 + r, y0)),
            ],
        }
    }

    /// A circle, as two half arcs.
    pub fn circle(circle: Circle2) -> Self {
        let [a, b] = circle.to_arcs();
        Loop2 { curves: vec![Curve2::Arc(a), Curve2::Arc(b)] }
    }

    /// A closed polygon.
    pub fn polygon(points: &[Point2]) -> Result<Self> {
        if points.len() < 3 {
            return Err(GeometryError::EmptyContour);
        }
        let mut curves = Vec::with_capacity(points.len());
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            if (b - a).length().mm() > TOL.mm() {
                curves.push(Curve2::line(a, b));
            }
        }
        Loop2::from_ordered(curves)
    }

    pub fn curves(&self) -> &[Curve2] {
        &self.curves
    }

    pub fn is_closed(&self, tol: Length) -> bool {
        let Some(first) = self.curves.first() else {
            return false;
        };
        let last = self.curves.last().expect("non-empty");
        points_equal(last.end(), first.start(), tol)
            && self.curves.windows(2).all(|w| points_equal(w[0].end(), w[1].start(), tol))
    }

    /// Polyline approximation of the loop, used for area, orientation, bounds
    /// and the OpenSCAD derivative export.
    pub fn to_polygon(&self, arc_segments: usize) -> Polygon2 {
        let mut points = Vec::new();
        for curve in &self.curves {
            points.extend(curve.tessellate(arc_segments));
        }
        Polygon2::new(points)
    }

    pub fn signed_area(&self) -> f64 {
        self.to_polygon(64).signed_area()
    }

    pub fn is_ccw(&self) -> bool {
        self.signed_area() > 0.0
    }

    /// Reverses the loop so that it winds counter-clockwise (positive area).
    pub fn make_ccw(&mut self) {
        if !self.is_ccw() {
            self.reverse();
        }
    }

    pub fn reverse(&mut self) {
        self.curves.reverse();
        for curve in &mut self.curves {
            *curve = curve.reversed();
        }
    }

    pub fn bounds(&self) -> Bounds2 {
        self.to_polygon(64).bounds().expect("a closed loop always has at least one point")
    }

    /// The same loop moved `distance` outward, or inward when negative.
    ///
    /// Offsetting is done here rather than in a CAD kernel so that every
    /// backend gets the same answer and none has to provide it. Each curve
    /// moves along its own normal — a line slides, an arc changes radius — and
    /// the joints are then closed up: tangent joints already meet, and corners
    /// are mitred to the point where the two offset curves would cross.
    pub fn offset(&self, distance: Length) -> Option<Loop2> {
        self.offset_each(&vec![distance; self.curves.len()])
    }

    /// Offsets each curve by its own distance, positive outward.
    ///
    /// This is how a drawn outline becomes a wall: offset the centre-line by
    /// half the line width one way for the outside face and the other way for
    /// the inside, per segment, so a segment drawn thicker is thicker only
    /// along that stretch. Where two widths meet the offsets no longer line up
    /// and a straight connector carries the step across — which is what a step
    /// in wall thickness actually looks like.
    ///
    /// `distances` is indexed by curve. Missing entries offset by zero.
    pub fn offset_each(&self, distances: &[Length]) -> Option<Loop2> {
        if !self.is_ccw() {
            // Work counter-clockwise so "outward" has one meaning. Reversing
            // reverses the curve order too, so the distances follow.
            let mut flipped = self.clone();
            flipped.reverse();
            let flipped_distances: Vec<Length> = distances.iter().rev().copied().collect();
            let mut result = flipped.offset_each(&flipped_distances)?;
            result.reverse();
            return Some(result);
        }
        if distances.iter().all(|d| *d == Length::ZERO) {
            return Some(self.clone());
        }

        let moved: Vec<Curve2> = self
            .curves
            .iter()
            .enumerate()
            .map(|(index, curve)| {
                offset_curve(curve, distances.get(index).copied().unwrap_or(Length::ZERO))
            })
            .collect::<Option<_>>()?;

        // Close the joints. A tangent joint of equal width needs nothing; a
        // corner needs the two curves brought to where they would meet; a
        // width step needs a connector between them.
        let mut joined: Vec<Curve2> = Vec::with_capacity(moved.len() * 2);
        let mut pending: Vec<(usize, Curve2)> = Vec::new();
        let mut moved = moved;
        for index in 0..moved.len() {
            let next_index = (index + 1) % moved.len();
            let end = moved[index].end();
            let start = moved[next_index].start();
            if (end - start).length() <= JOIN_TOL {
                moved[next_index] = moved[next_index].with_start(end);
                continue;
            }
            match (&moved[index], &moved[next_index]) {
                (Curve2::Line(a), Curve2::Line(b)) => match line_intersection(a, b) {
                    Some(meeting) => {
                        moved[index] = moved[index].with_end(meeting);
                        moved[next_index] = moved[next_index].with_start(meeting);
                    },
                    // Parallel: a straight-through joint whose width changed.
                    None => pending.push((index, Curve2::line(end, start))),
                },
                // An arc is offset tangentially, so any gap left here is the
                // width step itself. Bridge it rather than distorting either
                // neighbour to close it.
                _ => pending.push((index, Curve2::line(end, start))),
            }
        }

        for (index, curve) in moved.into_iter().enumerate() {
            joined.push(curve);
            if let Some((_, connector)) = pending.iter().find(|(at, _)| *at == index) {
                joined.push(*connector);
            }
        }

        let kept: Vec<Curve2> = joined.into_iter().filter(|c| !c.is_degenerate()).collect();
        if kept.len() < 2 {
            return None;
        }
        Loop2::from_ordered(kept).ok()
    }

    /// True when this loop still stands off from `original` the way an offset
    /// of it by `distance` should.
    ///
    /// [`Loop2::offset_each`] does not notice an offset that runs past the far
    /// side of a shape. It trims the curves that crossed against each other and
    /// hands back a smaller loop travelling backwards, and nothing else sees
    /// through that: the loop is well formed, it closes, and it keeps the sign
    /// of its area. What it does not keep is its distance. Every point of a real
    /// offset stands the offset distance off the shape it came from, and a loop
    /// that folded through the middle has points far nearer than that.
    ///
    /// `distance` is the smallest offset that was asked for, and half of it is
    /// the threshold — far below anything a mitre or a tessellated arc can
    /// account for, and far above the fold this is looking for.
    pub fn stands_off(&self, original: &Loop2, distance: Length) -> bool {
        if !distance.is_positive() {
            return true;
        }
        let threshold = distance.mm() / 2.0;
        let source = original.to_polygon(STANDOFF_SAMPLES).points;
        if source.len() < 2 {
            return false;
        }
        let offset = self.to_polygon(STANDOFF_SAMPLES).points;
        // Midpoints as well as corners: on a loop that folded, the nearest
        // approach can be along an edge rather than at either end of it.
        offset
            .iter()
            .enumerate()
            .flat_map(|(index, point)| {
                let next = offset[(index + 1) % offset.len()];
                [*point, LineSegment2::new(*point, next).midpoint()]
            })
            .all(|point| distance_to_polygon(point, &source) >= threshold)
    }

    /// True when the loop contains at least one arc.
    ///
    /// A loop with arcs was drawn with its corners already rounded, so nothing
    /// downstream should try to round them again.
    pub fn has_arcs(&self) -> bool {
        self.curves.iter().any(|curve| matches!(curve, Curve2::Arc(_)))
    }
}

/// A closed region: one outer loop plus zero or more hole loops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile2d {
    pub outer: Loop2,
    pub holes: Vec<Loop2>,
}

impl Profile2d {
    pub fn new(mut outer: Loop2, mut holes: Vec<Loop2>) -> Self {
        outer.make_ccw();
        for hole in &mut holes {
            // Holes wind opposite to the outer boundary.
            hole.make_ccw();
            hole.reverse();
        }
        Profile2d { outer, holes }
    }

    pub fn simple(outer: Loop2) -> Self {
        Profile2d::new(outer, Vec::new())
    }

    pub fn bounds(&self) -> Bounds2 {
        self.outer.bounds()
    }

    /// True when any loop in the region contains an arc.
    pub fn has_arcs(&self) -> bool {
        self.outer.has_arcs() || self.holes.iter().any(|hole| hole.has_arcs())
    }

    /// The whole region moved outward, or inward when negative.
    ///
    /// Holes move the opposite way, so that growing the region shrinks the
    /// holes in it.
    pub fn offset(&self, distance: Length) -> Option<Profile2d> {
        let outer = self.outer.offset(distance)?;
        let holes =
            self.holes.iter().map(|hole| hole.offset(distance)).collect::<Option<Vec<_>>>()?;
        Some(Profile2d::new(outer, holes))
    }

    /// Area of the region, holes subtracted.
    pub fn area(&self) -> f64 {
        let outer = self.outer.signed_area().abs();
        let holes: f64 = self.holes.iter().map(|h| h.signed_area().abs()).sum();
        outer - holes
    }
}

/// Shortest distance from a point to the boundary of a closed polygon.
fn distance_to_polygon(point: Point2, polygon: &[Point2]) -> f64 {
    (0..polygon.len())
        .map(|index| {
            let (a, b) = (polygon[index], polygon[(index + 1) % polygon.len()]);
            let edge = b - a;
            let length = edge.length().mm();
            if length <= TOL.mm() {
                return (point - a).length().mm();
            }
            let t = ((point - a).dot(edge) / (length * length)).clamp(0.0, 1.0);
            let nearest = Point2::new(a.x + edge.x * t, a.y + edge.y * t);
            (point - nearest).length().mm()
        })
        .fold(f64::INFINITY, f64::min)
}

#[inline]
fn points_equal(a: Point2, b: Point2, tol: Length) -> bool {
    (a - b).length().mm() <= tol.mm()
}

/// Chains an unordered set of curves into closed loops.
///
/// Curves may be given in any order and any direction, exactly as KiCad hands
/// them over. Degenerate curves are dropped. A chain that runs out of
/// continuations produces [`GeometryError::OpenContour`] naming the free end,
/// so callers can select the offending graphic in KiCad.
pub fn assemble_loops(curves: &[Curve2], tol: Length) -> Result<Vec<Loop2>> {
    let mut remaining: Vec<(usize, Curve2)> =
        curves.iter().copied().enumerate().filter(|(_, c)| !c.is_degenerate()).collect();

    if remaining.is_empty() {
        return Err(GeometryError::EmptyContour);
    }

    let mut loops = Vec::new();

    while !remaining.is_empty() {
        let (start_index, first) = remaining.remove(0);
        let mut chain = vec![first];
        let start_point = first.start();
        let mut current_end = first.end();
        let mut last_index = start_index;

        while !points_equal(current_end, start_point, tol) {
            let next = remaining.iter().position(|(_, c)| {
                points_equal(c.start(), current_end, tol) || points_equal(c.end(), current_end, tol)
            });

            match next {
                Some(pos) => {
                    let (index, curve) = remaining.remove(pos);
                    let oriented = if points_equal(curve.start(), current_end, tol) {
                        curve
                    } else {
                        curve.reversed()
                    };
                    // Snap it onto the end of the chain. Without this the wire
                    // has microscopic gaps at every joint, which the kernel
                    // turns into invalid geometry further down.
                    let oriented = oriented.with_start(current_end);
                    current_end = oriented.end();
                    last_index = index;
                    chain.push(oriented);
                },
                None => {
                    // Say how far away the nearest loose end is: a contour that
                    // misses by microns is a different problem from one with a
                    // segment missing, and the number tells them apart.
                    let nearest = remaining
                        .iter()
                        .flat_map(|(_, c)| [c.start(), c.end()])
                        .map(|p| (p - current_end).length())
                        .fold(None::<Length>, |best, d| {
                            Some(match best {
                                Some(b) if b < d => b,
                                _ => d,
                            })
                        });
                    return Err(GeometryError::OpenContour {
                        at: current_end,
                        curve_index: last_index,
                        nearest_gap: nearest,
                    });
                },
            }
        }

        // Close the ring exactly, so the last curve lands on the first's start.
        if let Some(last) = chain.last_mut() {
            *last = last.with_end(start_point);
        }
        loops.push(Loop2 { curves: chain });
    }

    Ok(loops)
}

/// Assembles curves into exactly one region: the largest loop becomes the outer
/// boundary and any remaining loops become holes.
pub fn assemble_profile(curves: &[Curve2], tol: Length) -> Result<Profile2d> {
    let loops = assemble_loops(curves, tol)?;
    let mut sorted: Vec<(f64, Loop2)> =
        loops.into_iter().map(|l| (l.signed_area().abs(), l)).collect();
    sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut iter = sorted.into_iter().map(|(_, l)| l);
    let outer = iter.next().ok_or(GeometryError::EmptyContour)?;
    let holes: Vec<Loop2> = iter.collect();
    Ok(Profile2d::new(outer, holes))
}

/// Like [`assemble_profile`], but rejects multiple disjoint outer contours.
///
/// The board outline must be a single connected region; two separate outer
/// contours mean the user drew two boards.
pub fn assemble_single_region(curves: &[Curve2], tol: Length) -> Result<Profile2d> {
    let loops = assemble_loops(curves, tol)?;
    let profile = {
        let mut sorted: Vec<(f64, Loop2)> =
            loops.into_iter().map(|l| (l.signed_area().abs(), l)).collect();
        sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut iter = sorted.into_iter();
        let (_, outer) = iter.next().ok_or(GeometryError::EmptyContour)?;
        let outer_bounds = outer.bounds();
        let mut holes = Vec::new();
        for (_, candidate) in iter {
            let b = candidate.bounds();
            let contained = b.min.x >= outer_bounds.min.x
                && b.min.y >= outer_bounds.min.y
                && b.max.x <= outer_bounds.max.x
                && b.max.y <= outer_bounds.max.y;
            if !contained {
                return Err(GeometryError::DisconnectedContours { count: 2 });
            }
            holes.push(candidate);
        }
        Profile2d::new(outer, holes)
    };
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::from_mm(x, y)
    }

    #[test]
    fn assembles_shuffled_rectangle() {
        // Deliberately shuffled and with mixed directions, as KiCad delivers.
        let curves = vec![
            Curve2::line(p(50.0, 30.0), p(50.0, 0.0)),
            Curve2::line(p(0.0, 0.0), p(50.0, 0.0)),
            Curve2::line(p(0.0, 30.0), p(0.0, 0.0)),
            Curve2::line(p(50.0, 30.0), p(0.0, 30.0)),
        ];
        let profile = assemble_single_region(&curves, JOIN_TOL).expect("rectangle closes");
        assert!(profile.holes.is_empty());
        assert!((profile.area() - 1500.0).abs() < 1e-6);
        let bounds = profile.bounds();
        assert!((bounds.width().mm() - 50.0).abs() < 1e-9);
        assert!((bounds.height().mm() - 30.0).abs() < 1e-9);
        assert!(profile.outer.is_ccw());
    }

    #[test]
    fn chained_curves_are_snapped_exactly_together() {
        // Endpoints 15 um apart, as KiCad leaves them after a fillet: close
        // enough to chain, far enough to wreck a wire if left alone.
        let curves = vec![
            Curve2::line(p(0.0, 0.0), p(10.0, 0.0)),
            Curve2::line(p(10.000_015, 0.000_015), p(10.0, 10.0)),
            Curve2::line(p(10.0, 10.0), p(0.0, 10.0)),
            Curve2::line(p(0.0, 10.0), p(0.000_02, 0.0)),
        ];
        let loops = assemble_loops(&curves, JOIN_TOL).expect("chains");
        assert_eq!(loops.len(), 1);

        // Every joint is now exact, not merely close.
        let chained = loops[0].curves();
        for index in 0..chained.len() {
            let end = chained[index].end();
            let next = chained[(index + 1) % chained.len()].start();
            assert_eq!(end, next, "joint {index} was left with a gap");
        }
    }

    #[test]
    fn open_contour_reports_free_end() {
        let curves = vec![
            Curve2::line(p(0.0, 0.0), p(10.0, 0.0)),
            Curve2::line(p(10.0, 0.0), p(10.0, 10.0)),
            Curve2::line(p(10.0, 10.0), p(0.0, 10.0)),
            // Missing the closing segment back to the origin.
        ];
        let err = assemble_loops(&curves, JOIN_TOL).expect_err("open contour must fail");
        match err {
            GeometryError::OpenContour { at, .. } => {
                assert!((at.x.mm() - 0.0).abs() < 1e-9 && (at.y.mm() - 10.0).abs() < 1e-9);
            },
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn rounded_rectangle_with_arcs_closes() {
        // 20 x 10 rectangle with 2 mm rounded corners, drawn as lines + arcs.
        // `k` is where the arc midpoint sits: r - r/sqrt(2) in from the corner.
        let r = 2.0;
        let k = r * std::f64::consts::FRAC_1_SQRT_2;
        let (w, h) = (20.0, 10.0);
        let curves = vec![
            Curve2::line(p(r, 0.0), p(w - r, 0.0)),
            Curve2::arc(p(w - r, 0.0), p(w - r + k, r - k), p(w, r)),
            Curve2::line(p(w, r), p(w, h - r)),
            Curve2::arc(p(w, h - r), p(w - r + k, h - r + k), p(w - r, h)),
            Curve2::line(p(w - r, h), p(r, h)),
            Curve2::arc(p(r, h), p(r - k, h - r + k), p(0.0, h - r)),
            Curve2::line(p(0.0, h - r), p(0.0, r)),
            Curve2::arc(p(0.0, r), p(r - k, r - k), p(r, 0.0)),
        ];
        let profile = assemble_single_region(&curves, JOIN_TOL).expect("rounded rect closes");
        // Area of the rounded rectangle: w*h - (4 - pi) * r^2
        let expected = w * h - (4.0 - std::f64::consts::PI) * r * r;
        assert!((profile.area() - expected).abs() < 0.01, "area was {}", profile.area());
    }

    #[test]
    fn board_with_hole_becomes_profile_with_hole() {
        let mut curves = vec![
            Curve2::line(p(0.0, 0.0), p(20.0, 0.0)),
            Curve2::line(p(20.0, 0.0), p(20.0, 20.0)),
            Curve2::line(p(20.0, 20.0), p(0.0, 20.0)),
            Curve2::line(p(0.0, 20.0), p(0.0, 0.0)),
        ];
        curves.extend(Loop2::circle(Circle2::new(p(10.0, 10.0), mm(2.0))).curves().iter().copied());
        let profile = assemble_single_region(&curves, JOIN_TOL).expect("region assembles");
        assert_eq!(profile.holes.len(), 1);
        let expected = 400.0 - std::f64::consts::PI * 4.0;
        assert!((profile.area() - expected).abs() < 0.05, "area was {}", profile.area());
    }

    #[test]
    fn two_separate_boards_are_rejected() {
        let curves = vec![
            Curve2::line(p(0.0, 0.0), p(10.0, 0.0)),
            Curve2::line(p(10.0, 0.0), p(10.0, 10.0)),
            Curve2::line(p(10.0, 10.0), p(0.0, 10.0)),
            Curve2::line(p(0.0, 10.0), p(0.0, 0.0)),
            Curve2::line(p(50.0, 0.0), p(60.0, 0.0)),
            Curve2::line(p(60.0, 0.0), p(60.0, 10.0)),
            Curve2::line(p(60.0, 10.0), p(50.0, 10.0)),
            Curve2::line(p(50.0, 10.0), p(50.0, 0.0)),
        ];
        let err = assemble_single_region(&curves, JOIN_TOL).expect_err("two outlines must fail");
        assert!(matches!(err, GeometryError::DisconnectedContours { .. }));
    }

    #[test]
    fn a_rounded_rectangle_loses_the_corners_but_keeps_its_size() {
        let r = mm(2.0);
        let loop2 = Loop2::rounded_rectangle(p(0.0, 0.0), p(20.0, 10.0), r);
        assert!(loop2.is_closed(JOIN_TOL));
        assert!(loop2.has_arcs());

        let bounds = loop2.bounds();
        assert!((bounds.width().mm() - 20.0).abs() < 1e-6);
        assert!((bounds.height().mm() - 10.0).abs() < 1e-6);

        // Area is the rectangle less the four corner offcuts.
        let expected = 20.0 * 10.0 - (4.0 - std::f64::consts::PI) * 4.0;
        assert!((loop2.signed_area().abs() - expected).abs() < 0.01);
    }

    #[test]
    fn a_rounded_rectangle_clamps_the_radius_to_the_shorter_side() {
        // A 10 mm radius cannot fit a 10 mm tall rectangle; KiCad clamps to 5.
        let loop2 = Loop2::rounded_rectangle(p(0.0, 0.0), p(20.0, 10.0), mm(10.0));
        assert!(loop2.is_closed(JOIN_TOL));
        let bounds = loop2.bounds();
        assert!((bounds.height().mm() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_radius_is_just_a_rectangle() {
        let loop2 = Loop2::rounded_rectangle(p(0.0, 0.0), p(20.0, 10.0), Length::ZERO);
        assert_eq!(loop2.curves().len(), 4);
        assert!(!loop2.has_arcs());
    }

    #[test]
    fn offsetting_a_rectangle_grows_and_shrinks_it() {
        let rect = Loop2::rectangle(p(0.0, 0.0), p(20.0, 10.0));

        let bigger = rect.offset(mm(2.0)).expect("grows");
        let bounds = bigger.bounds();
        assert!((bounds.width().mm() - 24.0).abs() < 1e-6, "width {}", bounds.width());
        assert!((bounds.height().mm() - 14.0).abs() < 1e-6);
        assert!((bounds.min.x.mm() + 2.0).abs() < 1e-6, "left edge {}", bounds.min.x);

        let smaller = rect.offset(mm(-2.0)).expect("shrinks");
        let bounds = smaller.bounds();
        assert!((bounds.width().mm() - 16.0).abs() < 1e-6, "width {}", bounds.width());
        assert!((bounds.height().mm() - 6.0).abs() < 1e-6);
    }

    #[test]
    fn offsetting_a_rounded_rectangle_keeps_it_closed_and_curved() {
        let r = 4.0;
        let k = r * std::f64::consts::FRAC_1_SQRT_2;
        // A 20 x 12 rounded rectangle: lines meeting arcs tangentially.
        let curves = vec![
            Curve2::line(p(r, 0.0), p(20.0 - r, 0.0)),
            Curve2::arc(p(20.0 - r, 0.0), p(20.0 - r + k, r - k), p(20.0, r)),
            Curve2::line(p(20.0, r), p(20.0, 12.0 - r)),
            Curve2::arc(p(20.0, 12.0 - r), p(20.0 - r + k, 12.0 - r + k), p(20.0 - r, 12.0)),
            Curve2::line(p(20.0 - r, 12.0), p(r, 12.0)),
            Curve2::arc(p(r, 12.0), p(r - k, 12.0 - r + k), p(0.0, 12.0 - r)),
            Curve2::line(p(0.0, 12.0 - r), p(0.0, r)),
            Curve2::arc(p(0.0, r), p(r - k, r - k), p(r, 0.0)),
        ];
        let loop2 = Loop2::from_ordered(curves).expect("closed");

        let grown = loop2.offset(mm(1.5)).expect("grows");
        assert!(grown.is_closed(JOIN_TOL), "the offset loop must still close");
        assert!(grown.has_arcs(), "the corners must stay curved");

        let bounds = grown.bounds();
        assert!((bounds.width().mm() - 23.0).abs() < 0.01, "width {}", bounds.width());
        assert!((bounds.height().mm() - 15.0).abs() < 0.01, "height {}", bounds.height());

        // Area grows by the perimeter times the distance, plus the corners.
        let before = loop2.signed_area().abs();
        let after = grown.signed_area().abs();
        assert!(after > before, "growing must add area");
    }

    #[test]
    fn shrinking_past_its_own_radius_is_refused() {
        // A circle of radius 2 cannot be shrunk by 5.
        let circle = Loop2::circle(Circle2::new(p(0.0, 0.0), mm(2.0)));
        assert!(circle.offset(mm(-5.0)).is_none());
    }

    /// A rectangle shrunk by more than half its own width comes back inside
    /// out rather than empty, and neither the loop's validity nor the sign of
    /// its area says so.
    #[test]
    fn an_offset_that_runs_past_the_far_side_stops_standing_off_the_shape() {
        let square = Loop2::rectangle(p(0.0, 0.0), p(2.0, 2.0));

        let real = square.offset(mm(-0.5)).expect("half a millimetre fits");
        assert!(real.stands_off(&square, mm(0.5)));

        let overshot = square.offset(mm(-1.5)).expect("the offset is not refused");
        assert!((overshot.signed_area().signum() - square.signed_area().signum()).abs() < 1e-9);
        assert!(
            !overshot.stands_off(&square, mm(1.5)),
            "an inverted offset must not read as the shape"
        );
    }

    /// A drawn outline with a bump on one side: offset inward, the foot of the
    /// bump leaves a gap that has to be bridged, so the result carries a curve
    /// the original has no counterpart for. That is an ordinary offset, and
    /// reading it as a fold refuses a board that builds perfectly well.
    #[test]
    fn an_offset_that_had_to_bridge_a_joint_still_stands_off() {
        let outline = Loop2::from_ordered(vec![
            Curve2::line(p(95.0, -83.5), p(95.0, -106.5)),
            Curve2::arc(p(95.0, -106.5), p(97.489592, -112.510408), p(103.5, -115.0)),
            Curve2::line(p(103.5, -115.0), p(146.5, -115.0)),
            Curve2::arc(p(146.5, -115.0), p(152.510408, -112.510408), p(155.0, -106.5)),
            Curve2::line(p(155.0, -106.5), p(155.0, -75.010408)),
            Curve2::arc(p(155.0, -75.010408), p(146.505204, -66.510409), p(138.0, -75.0)),
            Curve2::line(p(138.0, -75.0), p(103.5, -75.0)),
            Curve2::arc(p(103.5, -75.0), p(97.489592, -77.489592), p(95.0, -83.5)),
        ])
        .expect("the outline closes");

        let inward = outline.offset(mm(-0.75)).expect("half of a 1.5 mm wall fits");
        assert!(
            inward.curves().len() > outline.curves().len(),
            "the fixture needs a joint the offset had to bridge"
        );
        assert!(inward.stands_off(&outline, mm(0.75)));
    }

    #[test]
    fn offsetting_a_circle_changes_its_radius() {
        let circle = Loop2::circle(Circle2::new(p(0.0, 0.0), mm(5.0)));
        let grown = circle.offset(mm(1.0)).expect("grows");
        let bounds = grown.bounds();
        assert!((bounds.width().mm() - 12.0).abs() < 0.01, "diameter {}", bounds.width());
    }

    #[test]
    fn a_line_stroke_is_a_rectangle_centred_on_the_line() {
        let stroke = Curve2::line(p(0.0, 0.0), p(10.0, 0.0))
            .stroke_loop(mm(3.0))
            .expect("a line strokes to a rectangle");
        assert!(stroke.is_closed(JOIN_TOL));

        let bounds = stroke.bounds();
        assert!((bounds.width().mm() - 10.0).abs() < 1e-9);
        // 3 mm wide means 1.5 mm either side of the line.
        assert!((bounds.height().mm() - 3.0).abs() < 1e-9);
        assert!((bounds.min.y.mm() + 1.5).abs() < 1e-9);
        assert!((bounds.max.y.mm() - 1.5).abs() < 1e-9);
        assert!((stroke.signed_area().abs() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn an_arc_stroke_is_an_annular_sector() {
        // A quarter arc of radius 10, stroked 2 mm wide.
        let k = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
        let arc = Curve2::arc(p(10.0, 0.0), p(k, k), p(0.0, 10.0));
        let stroke = arc.stroke_loop(mm(2.0)).expect("an arc strokes to a sector");
        assert!(stroke.is_closed(JOIN_TOL));

        // Area of a quarter annulus between r = 9 and r = 11.
        let expected = std::f64::consts::PI * (11.0f64.powi(2) - 9.0f64.powi(2)) / 4.0;
        assert!(
            (stroke.signed_area().abs() - expected).abs() < 0.05,
            "area was {}, expected {expected}",
            stroke.signed_area().abs()
        );
    }

    #[test]
    fn a_stroke_wider_than_its_arc_is_refused() {
        let k = 2.0 * std::f64::consts::FRAC_1_SQRT_2;
        let arc = Curve2::arc(p(2.0, 0.0), p(k, k), p(0.0, 2.0));
        assert!(arc.stroke_loop(mm(6.0)).is_none());
    }

    #[test]
    fn a_miter_fills_the_notch_at_a_right_angle() {
        // Two 2 mm strokes meeting at 90 degrees leave a 1 x 1 notch outside.
        let incoming = Curve2::line(p(0.0, 0.0), p(10.0, 0.0));
        let outgoing = Curve2::line(p(10.0, 0.0), p(10.0, 10.0));
        let join = miter_join(&incoming, &outgoing, mm(2.0), mm(2.0)).expect("a corner to fill");
        assert!(join.is_closed(JOIN_TOL));
        assert!(
            (join.signed_area().abs() - 1.0).abs() < 1e-9,
            "area was {}",
            join.signed_area().abs()
        );
    }

    #[test]
    fn a_tangential_join_has_no_notch() {
        // A line running into an arc that starts tangent to it: every rounded
        // corner is made of these, and there is no wedge to fill.
        let r = 8.5;
        let k = r * std::f64::consts::FRAC_1_SQRT_2;
        let line = Curve2::line(p(0.0, 0.0), p(10.0, 0.0));
        let arc = Curve2::arc(p(10.0, 0.0), p(10.0 + k, r - k), p(10.0 + r, r));
        assert!(
            miter_join(&line, &arc, mm(1.5), mm(1.5)).is_none(),
            "a tangential junction must not produce a sliver"
        );
    }

    #[test]
    fn a_straight_join_has_no_notch() {
        let incoming = Curve2::line(p(0.0, 0.0), p(10.0, 0.0));
        let outgoing = Curve2::line(p(10.0, 0.0), p(20.0, 0.0));
        assert!(miter_join(&incoming, &outgoing, mm(2.0), mm(2.0)).is_none());
    }

    #[test]
    fn hole_loops_wind_opposite_to_outer() {
        let profile = Profile2d::new(
            Loop2::rectangle(p(0.0, 0.0), p(10.0, 10.0)),
            vec![Loop2::rectangle(p(2.0, 2.0), p(4.0, 4.0))],
        );
        assert!(profile.outer.is_ccw());
        assert!(!profile.holes[0].is_ccw());
    }
}
