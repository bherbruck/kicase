//! Board fixtures used by the geometry golden tests.
//!
//! These describe the boards from section 33 of the specification without
//! needing a running KiCad: each one produces the same [`BoardSource`] the
//! KiCad adapter would produce for the corresponding `.kicad_pcb`.

use kicase_geometry::profile::{Curve2, Loop2};
use kicase_geometry::types::{Circle2, Point2};
use kicase_geometry::units::{mm, Length};
use kicase_model::source::{BoardGraphic, BoardSource, KiCadUuid, LayerRole, MountingHole};

pub fn point(x: f64, y: f64) -> Point2 {
    Point2::from_mm(x, y)
}

pub fn rect_curves(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Curve2> {
    Loop2::rectangle(point(x0, y0), point(x1, y1)).curves().to_vec()
}

/// A wall drawn around a rectangle: centre line and line width, the way a user
/// draws it on the `Enclosure` layer.
pub fn drawn_wall(x0: f64, y0: f64, x1: f64, y1: f64, width: f64) -> (Vec<Curve2>, Vec<Length>) {
    let curves = rect_curves(x0, y0, x1, y1);
    let widths = vec![mm(width); curves.len()];
    (curves, widths)
}

/// 50 x 30 mm rectangular board with four M3 mounting holes.
///
/// The wall is drawn 2 mm wide on a 54 x 34 centre line, giving a 52 x 32
/// cavity and a 56 x 36 exterior.
pub fn rectangular_board() -> BoardSource {
    let (outline, widths) = drawn_wall(-2.0, -2.0, 52.0, 32.0, 2.0);
    BoardSource {
        board_outline: rect_curves(0.0, 0.0, 50.0, 30.0),
        enclosure_outline: outline,
        enclosure_outline_widths: widths,
        mounting_holes: mounting_holes(&[
            ("H1", 3.5, 3.5),
            ("H2", 46.5, 3.5),
            ("H3", 46.5, 26.5),
            ("H4", 3.5, 26.5),
        ]),
        ..BoardSource::default()
    }
}

/// 40 x 24 mm board whose corners are 4 mm `Edge.Cuts` arcs, with a wall drawn
/// around it that follows those curves.
pub fn rounded_board() -> BoardSource {
    let outline = rounded_rect_curves_at(-2.0, -2.0, 44.0, 28.0, 6.0);
    BoardSource {
        board_outline: rounded_rect_curves(40.0, 24.0, 4.0),
        enclosure_outline_widths: vec![mm(2.0); outline.len()],
        enclosure_outline: outline,
        ..BoardSource::default()
    }
}

/// An L-shaped board, to prove nothing assumes a rectangle.
pub fn l_shaped_board() -> BoardSource {
    let pts = [
        point(0.0, 0.0),
        point(60.0, 0.0),
        point(60.0, 20.0),
        point(25.0, 20.0),
        point(25.0, 40.0),
        point(0.0, 40.0),
    ];
    let mut curves = Vec::new();
    for i in 0..pts.len() {
        curves.push(Curve2::line(pts[i], pts[(i + 1) % pts.len()]));
    }

    // The wall follows the L outward by 2 mm, including the inside corner.
    let wall = [
        point(-2.0, -2.0),
        point(62.0, -2.0),
        point(62.0, 22.0),
        point(27.0, 22.0),
        point(27.0, 42.0),
        point(-2.0, 42.0),
    ];
    let mut outline = Vec::new();
    for i in 0..wall.len() {
        outline.push(Curve2::line(wall[i], wall[(i + 1) % wall.len()]));
    }

    BoardSource {
        board_outline: curves,
        enclosure_outline_widths: vec![mm(2.0); outline.len()],
        enclosure_outline: outline,
        ..BoardSource::default()
    }
}

/// Rectangular board with a front datum line and a USB-C sized opening drawn
/// beside it on the cuts layer.
pub fn usb_cutout_board() -> BoardSource {
    let mut source = rectangular_board();
    source.graphics.push(BoardGraphic {
        uuid: KiCadUuid::new("datum-front"),
        role: LayerRole::Datums,
        // Along the front edge (y = 0), running in +X.
        curves: vec![Curve2::line(point(0.0, 0.0), point(50.0, 0.0))],
        closed: false,
        stroke_width: None,
    });
    source.graphics.push(BoardGraphic {
        uuid: KiCadUuid::new("cut-usb"),
        role: LayerRole::Cuts,
        // A USB-C sitting on the board: 9.2 x 3.2 mm, drawn 7.6 to 10.8 mm
        // from the datum line, which is the height of the top of the PCB
        // above the bottom of the case.
        curves: rect_curves(20.4, 7.6, 29.6, 10.8),
        closed: true,
        stroke_width: None,
    });
    source
}

/// Rectangular board with a round LED window drawn on the top layer.
pub fn top_cutout_board() -> BoardSource {
    let mut source = rectangular_board();
    source.graphics.push(BoardGraphic {
        uuid: KiCadUuid::new("cut-led"),
        role: LayerRole::Top,
        curves: Loop2::circle(Circle2::new(point(25.0, 15.0), mm(2.5))).curves().to_vec(),
        closed: true,
        stroke_width: None,
    });
    source
}

/// Rectangular board with a rib drawn on the solids layer.
pub fn rib_board() -> BoardSource {
    let mut source = rectangular_board();
    source.graphics.push(BoardGraphic {
        uuid: KiCadUuid::new("solid-rib"),
        role: LayerRole::Solids,
        curves: rect_curves(10.0, 14.0, 40.0, 16.0),
        closed: true,
        stroke_width: None,
    });
    source
}

/// The rectangular board with a second closed loop drawn inside the wall: a
/// 14 x 10 mm compartment centred on the board, stroked the same 2 mm.
pub fn island_board() -> BoardSource {
    let (mut outline, mut widths) = drawn_wall(-2.0, -2.0, 52.0, 32.0, 2.0);
    let (island, island_widths) = drawn_wall(18.0, 10.0, 32.0, 20.0, 2.0);
    outline.extend(island);
    widths.extend(island_widths);
    BoardSource {
        board_outline: rect_curves(0.0, 0.0, 50.0, 30.0),
        enclosure_outline: outline,
        enclosure_outline_widths: widths,
        ..BoardSource::default()
    }
}

/// The rectangular board with one interior wall drawn inside another.
///
/// Each is a wall in its own right, so the inner one must survive the outer
/// one being hollowed out. That only holds if the outer island is built first,
/// which is the single ordering the builder depends on.
pub fn nested_island_board() -> BoardSource {
    let (mut outline, mut widths) = drawn_wall(-2.0, -2.0, 52.0, 32.0, 2.0);
    for (curves, w) in
        [drawn_wall(12.0, 6.0, 38.0, 24.0, 2.0), drawn_wall(20.0, 11.0, 30.0, 19.0, 2.0)]
    {
        outline.extend(curves);
        widths.extend(w);
    }
    BoardSource {
        board_outline: rect_curves(0.0, 0.0, 50.0, 30.0),
        enclosure_outline: outline,
        enclosure_outline_widths: widths,
        ..BoardSource::default()
    }
}

/// The rectangular board with an interior wall drawn narrower than its own
/// stroke: a 2 x 2 mm centre line at 3 mm covers its whole interior, so it is
/// a 5 x 5 mm solid post rather than a ring.
pub fn post_board() -> BoardSource {
    let (mut outline, mut widths) = drawn_wall(-2.0, -2.0, 52.0, 32.0, 2.0);
    let (post, post_widths) = drawn_wall(24.0, 14.0, 26.0, 16.0, 3.0);
    outline.extend(post);
    widths.extend(post_widths);
    BoardSource {
        board_outline: rect_curves(0.0, 0.0, 50.0, 30.0),
        enclosure_outline: outline,
        enclosure_outline_widths: widths,
        ..BoardSource::default()
    }
}

/// A 40 x 25 mm board whose enclosure wall is drawn on the `Enclosure` layer:
/// a 50 x 35 mm centre line with 5 mm rounded corners, stroked 2.5 mm wide.
pub fn drawn_outline_board() -> BoardSource {
    let outline = rounded_rect_curves_at(-5.0, -5.0, 50.0, 35.0, 5.0);
    BoardSource {
        board_outline: rect_curves(0.0, 0.0, 40.0, 25.0),
        enclosure_outline_widths: vec![mm(2.5); outline.len()],
        enclosure_outline: outline,
        ..BoardSource::default()
    }
}

fn mounting_holes(holes: &[(&str, f64, f64)]) -> Vec<MountingHole> {
    holes
        .iter()
        .map(|(id, x, y)| MountingHole {
            uuid: KiCadUuid::new(format!("hole-{id}")),
            reference: Some((*id).to_string()),
            position: point(*x, *y),
            drill_diameter: mm(3.2),
        })
        .collect()
}

/// A rounded rectangle as lines plus true circular arcs, the way KiCad stores
/// a board outline drawn with rounded corners.
pub fn rounded_rect_curves(w: f64, h: f64, r: f64) -> Vec<Curve2> {
    let k = r - r * std::f64::consts::FRAC_1_SQRT_2;
    vec![
        Curve2::line(point(r, 0.0), point(w - r, 0.0)),
        Curve2::arc(point(w - r, 0.0), point(w - k, k), point(w, r)),
        Curve2::line(point(w, r), point(w, h - r)),
        Curve2::arc(point(w, h - r), point(w - k, h - k), point(w - r, h)),
        Curve2::line(point(w - r, h), point(r, h)),
        Curve2::arc(point(r, h), point(k, h - k), point(0.0, h - r)),
        Curve2::line(point(0.0, h - r), point(0.0, r)),
        Curve2::arc(point(0.0, r), point(k, k), point(r, 0.0)),
    ]
}

/// A rounded rectangle placed at an arbitrary origin.
pub fn rounded_rect_curves_at(x0: f64, y0: f64, w: f64, h: f64, r: f64) -> Vec<Curve2> {
    let k = r - r * std::f64::consts::FRAC_1_SQRT_2;
    let (x1, y1) = (x0 + w, y0 + h);
    vec![
        Curve2::line(point(x0 + r, y0), point(x1 - r, y0)),
        Curve2::arc(point(x1 - r, y0), point(x1 - k, y0 + k), point(x1, y0 + r)),
        Curve2::line(point(x1, y0 + r), point(x1, y1 - r)),
        Curve2::arc(point(x1, y1 - r), point(x1 - k, y1 - k), point(x1 - r, y1)),
        Curve2::line(point(x1 - r, y1), point(x0 + r, y1)),
        Curve2::arc(point(x0 + r, y1), point(x0 + k, y1 - k), point(x0, y1 - r)),
        Curve2::line(point(x0, y1 - r), point(x0, y0 + r)),
        Curve2::arc(point(x0, y0 + r), point(x0 + k, y0 + k), point(x0 + r, y0)),
    ]
}

/// Millimetre helper re-export so tests read cleanly.
pub fn length(value: f64) -> Length {
    mm(value)
}
