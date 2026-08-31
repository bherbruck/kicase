//! The orientation cube.
//!
//! A small cube in the corner of the viewport that turns with the camera, so
//! you can always tell which way round the model is. Clicking a face turns the
//! view to look straight down that axis; clicking a corner gives the isometric
//! between its three faces.
//!
//! It is drawn with egui's 2D painter rather than in GL: a cube is six quads,
//! and this way hit-testing and labels come for free.

use crate::camera::Camera;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};

/// Width of the cube widget, in points.
pub const SIZE: f32 = 96.0;

/// One clickable face.
struct Face {
    normal: [f32; 3],
    label: &'static str,
    /// Corner offsets from the cube centre, in cube-local space.
    corners: [[f32; 3]; 4],
}

/// The six faces, labelled the way a person thinks about a case sitting on a
/// desk rather than by axis letter.
fn faces() -> [Face; 6] {
    [
        Face {
            normal: [0.0, 0.0, 1.0],
            label: "TOP",
            corners: [[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0]],
        },
        Face {
            normal: [0.0, 0.0, -1.0],
            label: "BOT",
            corners: [[-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, -1.0], [1.0, -1.0, -1.0]],
        },
        Face {
            normal: [0.0, -1.0, 0.0],
            label: "FRONT",
            corners: [[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0]],
        },
        Face {
            normal: [0.0, 1.0, 0.0],
            label: "BACK",
            corners: [[-1.0, 1.0, -1.0], [-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, -1.0]],
        },
        Face {
            normal: [-1.0, 0.0, 0.0],
            label: "LEFT",
            corners: [[-1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [-1.0, 1.0, -1.0]],
        },
        Face {
            normal: [1.0, 0.0, 0.0],
            label: "RIGHT",
            corners: [[1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0], [1.0, -1.0, 1.0]],
        },
    ]
}

/// The eight corners, each giving the isometric view between three faces.
fn corner_directions() -> [[f32; 3]; 8] {
    let mut out = [[0.0f32; 3]; 8];
    let mut index = 0;
    for x in [-1.0f32, 1.0] {
        for y in [-1.0f32, 1.0] {
            for z in [-1.0f32, 1.0] {
                out[index] = [x, y, z];
                index += 1;
            }
        }
    }
    out
}

/// Draws the cube in the top-right of `within` and applies any click.
///
/// Returns true when the view was changed, so the caller can repaint.
pub fn show(ui: &mut Ui, within: Rect, camera: &mut Camera) -> bool {
    let margin = 12.0;
    let rect = Rect::from_min_size(
        Pos2::new(within.right() - SIZE - margin, within.top() + margin),
        Vec2::splat(SIZE),
    );
    if !ui.is_rect_visible(rect) {
        return false;
    }

    let response = ui.interact(rect, ui.id().with("view-cube"), Sense::click());
    let pointer = response.hover_pos();
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    let radius = SIZE * 0.30;

    let (right, up) = camera.basis();
    let forward = camera.forward();
    // Orthographic projection onto the screen: Y grows downward in egui.
    let project = |p: [f32; 3]| -> Pos2 {
        let x = p[0] * right[0] + p[1] * right[1] + p[2] * right[2];
        let y = p[0] * up[0] + p[1] * up[1] + p[2] * up[2];
        Pos2::new(centre.x + x * radius, centre.y - y * radius)
    };
    let depth = |p: [f32; 3]| p[0] * forward[0] + p[1] * forward[1] + p[2] * forward[2];

    // Corners first: they sit on top and are the smaller target.
    let mut clicked: Option<[f32; 3]> = None;
    let mut hovered_corner: Option<usize> = None;
    let corners = corner_directions();
    for (index, corner) in corners.iter().enumerate() {
        if depth(*corner) > 0.0 {
            continue; // Facing away.
        }
        let at = project(*corner);
        if let Some(pointer) = pointer {
            if pointer.distance(at) <= 9.0 {
                hovered_corner = Some(index);
                if response.clicked() {
                    clicked = Some(*corner);
                }
            }
        }
    }

    // Faces, painted back to front so the near ones win.
    let mut visible: Vec<(f32, usize)> = faces()
        .iter()
        .enumerate()
        .map(|(index, face)| (depth(face.normal), index))
        .filter(|(d, _)| *d < -0.05)
        .collect();
    visible.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let faces = faces();
    for (_, index) in &visible {
        let face = &faces[*index];
        let points: Vec<Pos2> = face.corners.iter().map(|c| project(*c)).collect();

        let hovered =
            hovered_corner.is_none() && pointer.is_some_and(|p| point_in_polygon(p, &points));
        if hovered && response.clicked() {
            clicked = Some(face.normal);
        }

        let fill = if hovered {
            Color32::from_rgb(90, 120, 170)
        } else {
            Color32::from_rgba_unmultiplied(200, 202, 208, 220)
        };
        painter.add(egui::Shape::convex_polygon(
            points.clone(),
            fill,
            Stroke::new(1.0_f32, Color32::from_rgb(70, 74, 82)),
        ));

        // Label every face we can actually read: at an isometric angle all
        // three visible faces sit near 0.58, so the cut-off has to be below it.
        let facing = -depth(face.normal);
        if facing > 0.3 {
            let middle =
                points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / points.len() as f32;
            painter.text(
                middle.to_pos2(),
                Align2::CENTER_CENTER,
                face.label,
                FontId::proportional(if hovered { 12.0 } else { 11.0 }),
                Color32::from_rgb(40, 44, 52),
            );
        }
    }

    // Highlight the corner under the pointer, once the faces are down.
    if let Some(index) = hovered_corner {
        painter.circle_filled(project(corners[index]), 5.0, Color32::from_rgb(90, 120, 170));
    }

    if let Some(normal) = clicked {
        camera.look_along(normal);
        return true;
    }
    false
}

/// Even-odd point-in-polygon, for hit-testing a projected face.
fn point_in_polygon(point: Pos2, polygon: &[Pos2]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[j]);
        if (a.y > point.y) != (b.y > point.y) {
            let x_at = a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if x_at > point.x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_and_corner_is_a_distinct_direction() {
        let normals: Vec<[f32; 3]> = faces().iter().map(|f| f.normal).collect();
        assert_eq!(normals.len(), 6);
        for (index, a) in normals.iter().enumerate() {
            for b in normals.iter().skip(index + 1) {
                assert_ne!(a, b, "two faces point the same way");
            }
        }
        let corners = corner_directions();
        assert_eq!(corners.len(), 8);
        assert!(corners.iter().all(|c| c.iter().all(|v| v.abs() == 1.0)));
    }

    #[test]
    fn face_corners_lie_on_their_own_face() {
        for face in faces() {
            let axis = face.normal.iter().position(|v| v.abs() > 0.5).expect("an axis");
            for corner in face.corners {
                assert_eq!(
                    corner[axis], face.normal[axis],
                    "a corner of {} is off its face",
                    face.label
                );
            }
        }
    }

    #[test]
    fn hit_testing_a_square() {
        let square = [
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
        ];
        assert!(point_in_polygon(Pos2::new(5.0, 5.0), &square));
        assert!(!point_in_polygon(Pos2::new(15.0, 5.0), &square));
        assert!(!point_in_polygon(Pos2::new(5.0, -2.0), &square));
    }
}
