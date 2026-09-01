//! Reading board geometry.
//!
//! # Why the board document rather than typed IPC items
//!
//! KiCad 10's IPC API returns graphics as typed items, but the Rust client's
//! read model (`kicad-ipc-rs` 0.5.1) reports a polygon only as a vertex
//! *count*, drops pad drill positions relative to their footprint, and keeps
//! the raw protobuf payloads private, so there is no escape hatch. Reading the
//! board document — which the same IPC API hands over through
//! `SaveDocumentToString` — gives full fidelity, including every persistent
//! UUID, and it is the only route that recovers polygon vertices.
//!
//! It also means the geometry half of KiCase runs headlessly on a saved
//! `.kicad_pcb`, which is what makes the tests possible: KiCad 10 requires a
//! running GUI for IPC.
//!
//! # Coordinates
//!
//! KiCad board files store millimetres with Y pointing *down*. Everything above
//! this module works in a right-handed system with Y up, so Y is negated here,
//! once, at the boundary.

use crate::sexpr::{self, Node};
use kicase_geometry::profile::Curve2;
use kicase_geometry::types::{Circle2, Point2};
use kicase_geometry::units::{mm, Length};
use kicase_model::source::{BoardGraphic, BoardSource, KiCadUuid, LayerRole, MountingHole};
use std::collections::HashMap;

/// Canonical name of the PCB outline layer.
pub const EDGE_CUTS: &str = "Edge.Cuts";

/// Which canonical KiCad layer holds which kind of enclosure geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerRoles {
    pub outline: String,
    pub datums: String,
    pub cuts: String,
    pub top: String,
    pub bottom: String,
    pub solids: String,
}

impl LayerRoles {
    pub fn from_mapping(mapping: &kicase_model::config::LayerMapping) -> Self {
        LayerRoles {
            outline: mapping.outline.clone(),
            datums: mapping.datums.clone(),
            cuts: mapping.cuts.clone(),
            top: mapping.top.clone(),
            bottom: mapping.bottom.clone(),
            solids: mapping.solids.clone(),
        }
    }

    fn role_of(&self, layer: &str) -> Option<LayerRole> {
        if layer == EDGE_CUTS {
            Some(LayerRole::BoardOutline)
        } else if layer == self.outline {
            Some(LayerRole::Outline)
        } else if layer == self.datums {
            Some(LayerRole::Datums)
        } else if layer == self.cuts {
            Some(LayerRole::Cuts)
        } else if layer == self.top {
            Some(LayerRole::Top)
        } else if layer == self.bottom {
            Some(LayerRole::Bottom)
        } else if layer == self.solids {
            Some(LayerRole::Solids)
        } else {
            None
        }
    }
}

/// A layer as declared in the board's `(layers ...)` block.
///
/// Note that the ids in a board *file* are not the ids the IPC API uses: a
/// board file numbers `User.1` as 39 and `Edge.Cuts` as 25, while the API's
/// `BoardLayer` enum numbers them 53 and 47. Layers are therefore matched by
/// canonical name everywhere, and this id is kept only for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardLayer {
    /// Layer id *as written in the board file*, not the IPC layer id.
    pub id: i32,
    /// Canonical name, e.g. `User.1`.
    pub canonical: String,
    /// Display name the user (or KiCase) gave it, when set.
    pub display: Option<String>,
}

/// A footprint on the board, as far as KiCase cares about it.
#[derive(Debug, Clone, PartialEq)]
pub struct FootprintInfo {
    pub uuid: KiCadUuid,
    pub reference: Option<String>,
    /// Position in enclosure coordinates (millimetres, Y up).
    pub position: Point2,
    /// The footprint's own `at` angle in degrees, counter-clockwise once Y is
    /// up — which is the sense KiCad's own drill and STEP exports agree on.
    pub rotation: f64,
    /// True when the footprint sits on `B.Cu`, i.e. under the board.
    pub on_back: bool,
    /// The 3D shapes the footprint points at, in the order it lists them.
    ///
    /// Hidden models are dropped here rather than carried and filtered later:
    /// a model KiCad does not draw is not a component.
    pub models: Vec<ModelRef>,
}

/// One `(model ...)` entry, exactly as the board wrote it.
///
/// The path is left unresolved on purpose. `${KIPRJMOD}` means the project
/// directory, which this crate does not know and should not learn: resolution
/// belongs to whoever opened the project.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRef {
    /// The reference as written, KiCad variables and all.
    pub raw: String,
    /// Millimetres, in the model's own axes, applied after its rotation.
    pub offset: [f64; 3],
    pub scale: [f64; 3],
    /// Degrees about the model's own X, Y and Z.
    pub rotate: [f64; 3],
}

/// Everything read out of one board document.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardReading {
    pub source: BoardSource,
    /// Every layer the board declares.
    pub layers: Vec<BoardLayer>,
    /// Objects KiCase understood but had to skip, with a reason.
    pub skipped: Vec<String>,
    /// Footprints on the board, used to find the enclosure preview.
    pub footprints: Vec<FootprintInfo>,
    /// Canonical names of every layer that carries at least one object.
    ///
    /// Used when claiming user layers, so KiCase never takes a layer the user
    /// is already drawing on.
    pub used_layers: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the board document could not be parsed: {0}")]
    Sexpr(#[from] sexpr::ParseError),
    #[error("this does not look like a KiCad board (no `kicad_pcb` root)")]
    NotABoard,
}

/// The only top-level items an enclosure needs. Everything else in a board —
/// tracks, vias, zones and their fills — is skipped without being parsed, which
/// is the difference between a board opening in a moment and in minutes.
const WANTED: &[&str] = &[
    "layers",
    "general",
    "gr_line",
    "gr_arc",
    "gr_circle",
    "gr_rect",
    "gr_poly",
    "gr_curve",
    "footprint",
];

/// Parses a `.kicad_pcb` document.
pub fn read_board(text: &str, roles: &LayerRoles) -> Result<BoardReading, ReadError> {
    let selection = sexpr::parse_selected(text, WANTED)?;
    let root = Node::List(selection.items);
    if root.head() != Some("kicad_pcb") {
        return Err(ReadError::NotABoard);
    }

    let layers = read_layers(&root);
    let display_to_canonical: HashMap<&str, &str> = layers
        .iter()
        .filter_map(|layer| layer.display.as_deref().map(|d| (d, layer.canonical.as_str())))
        .collect();

    // (general (thickness 1.6)) — the board's own stackup total, which is why
    // KiCase never asks for a board thickness.
    let board_thickness =
        root.child("general").and_then(|g| g.child("thickness")).and_then(|t| t.number(0)).map(mm);
    let mut source = BoardSource { board_thickness, ..BoardSource::default() };
    let mut skipped = Vec::new();
    let mut used_layers: Vec<String> = Vec::new();
    let mut footprints: Vec<FootprintInfo> = Vec::new();
    root.walk(&mut |node: &Node| {
        if node.head() == Some("layer") {
            if let Some(name) = node.string(0) {
                let canonical = display_to_canonical.get(name).copied().unwrap_or(name).to_string();
                if !used_layers.contains(&canonical) {
                    used_layers.push(canonical);
                }
            }
        }
    });

    for node in root.as_list().unwrap_or(&[]) {
        let Some(head) = node.head() else { continue };
        match head {
            "gr_line" | "gr_arc" | "gr_circle" | "gr_rect" | "gr_poly" | "gr_curve" => {
                read_graphic(node, roles, &display_to_canonical, &mut source, &mut skipped);
            },
            "footprint" => read_footprint(node, &mut source, &mut footprints),
            _ => {},
        }
    }

    Ok(BoardReading { source, layers, skipped, used_layers, footprints })
}

fn read_layers(root: &Node) -> Vec<BoardLayer> {
    let Some(block) = root.child("layers") else {
        return Vec::new();
    };
    block
        .args()
        .iter()
        .filter_map(|entry| {
            let items = entry.as_list()?;
            let id: i32 = items.first()?.as_atom()?.parse().ok()?;
            let canonical = items.get(1)?.as_atom()?.to_string();
            // (53 "User.1" user "Enclosure") -- the fourth element, when
            // present, is the display name.
            let display = items.get(3).and_then(|n| n.as_atom()).map(|s| s.to_string());
            Some(BoardLayer { id, canonical, display })
        })
        .collect()
}

fn read_graphic(
    node: &Node,
    roles: &LayerRoles,
    display_to_canonical: &HashMap<&str, &str>,
    source: &mut BoardSource,
    skipped: &mut Vec<String>,
) {
    let Some(layer_name) = node.child_string("layer") else { return };
    // A board may refer to a user layer by its display name.
    let canonical = display_to_canonical.get(layer_name).copied().unwrap_or(layer_name);
    let Some(role) = roles.role_of(canonical) else { return };

    let uuid = node.child_string("uuid").unwrap_or_default().to_string();
    let head = node.head().unwrap_or_default();
    // The stroke width matters on the outline layer: it is the wall thickness.
    let stroke_width = node
        .child("stroke")
        .and_then(|stroke| stroke.child("width"))
        .and_then(|width| width.number(0))
        .map(mm);

    let (curves, closed) = match head {
        "gr_line" => match (node.child_xy("start"), node.child_xy("end")) {
            (Some(start), Some(end)) => (vec![Curve2::line(point(start), point(end))], false),
            _ => return,
        },
        "gr_arc" => match (node.child_xy("start"), node.child_xy("mid"), node.child_xy("end")) {
            (Some(start), Some(mid), Some(end)) => {
                (vec![Curve2::arc(point(start), point(mid), point(end))], false)
            },
            _ => return,
        },
        "gr_circle" => match (node.child_xy("center"), node.child_xy("end")) {
            (Some(center), Some(edge)) => {
                let center = point(center);
                let radius = (point(edge) - center).length();
                if !radius.is_positive() {
                    skipped.push(format!("circle {uuid} has zero radius"));
                    return;
                }
                (
                    kicase_geometry::profile::Loop2::circle(Circle2::new(center, radius))
                        .curves()
                        .to_vec(),
                    true,
                )
            },
            _ => return,
        },
        "gr_rect" => match (node.child_xy("start"), node.child_xy("end")) {
            (Some(start), Some(end)) => {
                let (a, b) = (point(start), point(end));
                let min = Point2::new(a.x.min(b.x), a.y.min(b.y));
                let max = Point2::new(a.x.max(b.x), a.y.max(b.y));
                // KiCad rectangles carry their own corner radius: `(radius N)`.
                // Respect it, so a rounded rectangle drawn in KiCad produces
                // rounded geometry rather than a sharp one.
                let radius =
                    node.child("radius").and_then(|r| r.number(0)).map(mm).unwrap_or(Length::ZERO);
                (
                    kicase_geometry::profile::Loop2::rounded_rectangle(min, max, radius)
                        .curves()
                        .to_vec(),
                    true,
                )
            },
            _ => return,
        },
        "gr_poly" => {
            let Some(pts) = node.child("pts") else { return };
            let points: Vec<Point2> = pts
                .children("xy")
                .filter_map(|xy| Some(point((xy.number(0)?, xy.number(1)?))))
                .collect();
            if points.len() < 3 {
                skipped.push(format!("polygon {uuid} has fewer than three points"));
                return;
            }
            match kicase_geometry::profile::Loop2::polygon(&points) {
                Ok(loop2) => (loop2.curves().to_vec(), true),
                Err(err) => {
                    skipped.push(format!("polygon {uuid} could not be closed: {err}"));
                    return;
                },
            }
        },
        "gr_curve" => {
            // Bezier curves are out of scope for v0.1; say so rather than
            // silently approximating them.
            skipped.push(format!(
                "bezier curve {uuid} on {canonical} was ignored: KiCase v0.1 supports lines and arcs"
            ));
            return;
        },
        _ => return,
    };

    let graphic = BoardGraphic { uuid: KiCadUuid::new(uuid), role, curves, closed, stroke_width };

    match role {
        LayerRole::BoardOutline => source.board_outline.extend(graphic.curves.iter().copied()),
        LayerRole::Outline => {
            // One width per curve, so each drawn segment keeps its own
            // thickness even after the outline is reassembled.
            let width = graphic.stroke_width.unwrap_or(Length::ZERO);
            for curve in &graphic.curves {
                source.enclosure_outline.push(*curve);
                source.enclosure_outline_widths.push(width);
            }
        },
        LayerRole::Datums
        | LayerRole::Cuts
        | LayerRole::Top
        | LayerRole::Bottom
        | LayerRole::Solids => source.graphics.push(graphic),
    }
}

/// Finds mounting-hole candidates: non-plated, circular through holes.
///
/// Detection is deliberately conservative — every candidate is reported, and
/// the user decides which ones become standoffs.
fn read_footprint(node: &Node, source: &mut BoardSource, footprints: &mut Vec<FootprintInfo>) {
    let Some((fx, fy)) = node.child_xy("at") else { return };
    let rotation = node.child("at").and_then(|n| n.number(2)).unwrap_or(0.0);
    let reference = node
        .children("property")
        .find(|p| p.string(0) == Some("Reference"))
        .and_then(|p| p.string(1))
        .map(|s| s.to_string());

    footprints.push(FootprintInfo {
        uuid: KiCadUuid::new(node.child_string("uuid").unwrap_or_default()),
        reference: reference.clone(),
        position: point((fx, fy)),
        rotation,
        on_back: node.child_string("layer").is_some_and(|layer| layer.starts_with("B.")),
        models: node.children("model").filter_map(read_model).collect(),
    });

    for pad in node.children("pad") {
        // (pad "" np_thru_hole circle (at dx dy) (size w h) (drill d) ...)
        if pad.string(1) != Some("np_thru_hole") {
            continue;
        }
        let Some(drill) = pad.child("drill") else { continue };
        // A circular drill is a single diameter; an oval one is
        // `(drill oval w h)` and is not a screw hole.
        if drill.string(0) == Some("oval") {
            continue;
        }
        let Some(diameter) = drill.number(0) else { continue };
        let Some((dx, dy)) = pad.child_xy("at") else { continue };

        // The pad offset is expressed in the footprint's rotated frame, and
        // the footprint's angle is counter-clockwise with Y *up*. In the file's
        // Y-down frame that is the clockwise sense — the opposite of the one
        // this used to apply, which mirrored the standoffs of every rotated
        // multi-pad mounting footprint. Verified against `kicad-cli pcb export
        // drill`: a footprint at (100, 50) turned 90 degrees with a pad at
        // local (1, 0) drills at (100, 49), not (100, 51).
        let angle = rotation.to_radians();
        let (sin, cos) = angle.sin_cos();
        let x = fx + dx * cos + dy * sin;
        let y = fy - dx * sin + dy * cos;

        let uuid = pad.child_string("uuid").unwrap_or_default().to_string();
        source.mounting_holes.push(MountingHole {
            uuid: KiCadUuid::new(uuid),
            reference: reference.clone(),
            position: point((x, y)),
            drill_diameter: mm(diameter),
        });
    }
}

/// Reads one `(model ...)` entry, or nothing when KiCad would not draw it.
fn read_model(node: &Node) -> Option<ModelRef> {
    // Both `(hide yes)` and a bare `(hide)` hide a model; only `(hide no)`
    // does not. Confirmed by exporting each spelling: both gave board-only
    // geometry.
    if let Some(hide) = node.child("hide") {
        if hide.string(0) != Some("no") {
            return None;
        }
    }
    let raw = node.string(0)?.to_string();

    // Boards written by KiCad 6 and later use `(offset (xyz ...))` in
    // millimetres. A board carried forward from KiCad 5 still says `(at (xyz
    // ...))`, and KiCad reads *that* one as inches — measured, a footprint at
    // x=120 with `(at (xyz 1 0 0))` puts its model at x=145.4.
    let (offset_node, to_mm) = match node.child("offset") {
        Some(offset) => (Some(offset), 1.0),
        None => (node.child("at"), 25.4),
    };
    let offset = xyz(offset_node, [0.0; 3]).map(|value| value * to_mm);

    Some(ModelRef {
        raw,
        offset,
        scale: xyz(node.child("scale"), [1.0; 3]),
        rotate: xyz(node.child("rotate"), [0.0; 3]),
    })
}

/// The three numbers of a `(<head> (xyz a b c))` block, or `fallback`.
fn xyz(node: Option<&Node>, fallback: [f64; 3]) -> [f64; 3] {
    let Some(xyz) = node.and_then(|n| n.child("xyz")) else { return fallback };
    [0, 1, 2].map(|index| xyz.number(index).unwrap_or(fallback[index]))
}

/// KiCad millimetres with Y down, to enclosure millimetres with Y up.
fn point((x, y): (f64, f64)) -> Point2 {
    Point2::new(mm(x), mm(-y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kicase_geometry::profile::{assemble_single_region, JOIN_TOL};

    fn roles() -> LayerRoles {
        LayerRoles {
            outline: "User.1".into(),
            datums: "User.2".into(),
            cuts: "User.3".into(),
            top: "User.4".into(),
            bottom: "User.5".into(),
            solids: "User.6".into(),
        }
    }

    const BOARD: &str = r#"
    (kicad_pcb
      (version 20241229)
      (generator "pcbnew")
      (layers
        (0 "F.Cu" signal)
        (31 "B.Cu" signal)
        (47 "Edge.Cuts" user)
        (53 "User.1" user "Enclosure")
        (54 "User.2" user "Enclosure.Datums")
        (55 "User.3" user "Enclosure.Cuts")
        (56 "User.4" user "Enclosure.Solids")
      )
      (gr_line (start 100 100) (end 150 100) (layer "Edge.Cuts") (uuid "e1"))
      (gr_line (start 150 100) (end 150 130) (layer "Edge.Cuts") (uuid "e2"))
      (gr_line (start 150 130) (end 100 130) (layer "Edge.Cuts") (uuid "e3"))
      (gr_line (start 100 130) (end 100 100) (layer "Edge.Cuts") (uuid "e4"))
      (gr_line (start 100 100) (end 150 100) (layer "User.2") (uuid "datum-front"))
      (gr_rect (start 120 95) (end 130 98) (layer "Enclosure.Cuts") (uuid "cut-usb"))
      (gr_rect (start 100 120) (end 120 130) (radius 2) (layer "User.3") (uuid "cut-rounded"))
      (gr_poly (pts (xy 110 110) (xy 120 110) (xy 120 115)) (layer "User.4") (uuid "solid-rib"))
      (gr_circle (center 125 115) (end 128 115) (layer "User.3") (uuid "cut-led"))
      (gr_curve (start 0 0) (end 1 1) (layer "User.3") (uuid "bezier"))
      (footprint "MountingHole:MountingHole_3.2mm"
        (layer "F.Cu")
        (at 105 105)
        (uuid "fp1")
        (property "Reference" "H1")
        (pad "" np_thru_hole circle (at 0 0) (size 6 6) (drill 3.2) (uuid "hole-1"))
      )
      (footprint "Connector:USB_C"
        (layer "F.Cu")
        (at 120 100 90)
        (uuid "fp2")
        (property "Reference" "J1")
        (pad "" np_thru_hole oval (at 1 0) (size 2 3) (drill oval 1 2) (uuid "slot-1"))
        (pad "1" smd rect (at 0 0) (size 1 1) (uuid "pad-1"))
        (model "${KICAD9_3DMODEL_DIR}/Connector.3dshapes/USB_C.step"
          (offset (xyz 0 0 1.5)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 -90))
        )
        (model "${KIPRJMOD}/hidden.step" (hide yes) (offset (xyz 0 0 0)))
      )
      (footprint "MountingHole:MountingHole_2.2mm"
        (layer "B.Cu")
        (at 100 50 90)
        (uuid "fp3")
        (property "Reference" "H9")
        (pad "" np_thru_hole circle (at 1 0) (size 4 4) (drill 2.2) (uuid "hole-9"))
        (model "legacy.wrl" (at (xyz 1 0 0)))
      )
    )"#;

    #[test]
    fn reads_the_board_thickness_from_the_stackup() {
        let board =
            BOARD.replace("(version 20241229)", "(version 20241229)\n(general (thickness 2.4))");
        let reading = read_board(&board, &roles()).expect("parses");
        assert_eq!(reading.source.board_thickness, Some(mm(2.4)));
    }

    #[test]
    fn reads_edge_cuts_into_a_closed_profile() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        assert_eq!(reading.source.board_outline.len(), 4);
        let profile =
            assemble_single_region(&reading.source.board_outline, JOIN_TOL).expect("closes");
        assert_eq!(profile.bounds().width(), mm(50.0));
        assert_eq!(profile.bounds().height(), mm(30.0));
    }

    #[test]
    fn flips_y_once_at_the_boundary() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let profile =
            assemble_single_region(&reading.source.board_outline, JOIN_TOL).expect("closes");
        // KiCad y = 100..130 becomes enclosure y = -130..-100.
        assert_eq!(profile.bounds().min.y, mm(-130.0));
        assert_eq!(profile.bounds().max.y, mm(-100.0));
    }

    #[test]
    fn recognises_layers_by_canonical_and_display_name() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let cuts: Vec<_> = reading.source.graphics_on(LayerRole::Cuts).collect();
        // The rectangle names the layer by its display name, the circle by its
        // canonical name; both must land on the cuts layer.
        assert_eq!(cuts.len(), 3);
        assert!(cuts.iter().all(|g| g.closed));
    }

    #[test]
    fn a_rectangle_keeps_the_corner_radius_kicad_gave_it() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let rounded = reading.source.graphic("cut-rounded").expect("rounded rect found");
        assert!(rounded.closed);
        // Four lines and four corner arcs, not a sharp four-sided rectangle.
        assert_eq!(rounded.curves.len(), 8);

        let sharp = reading.source.graphic("cut-usb").expect("plain rect found");
        assert_eq!(sharp.curves.len(), 4);
    }

    #[test]
    fn reads_datum_lines_and_solid_polygons() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let datum = reading.source.graphic("datum-front").expect("datum found");
        assert!(datum.as_line().is_some());
        assert!(!datum.closed);

        let rib = reading.source.graphic("solid-rib").expect("rib found");
        assert!(rib.closed);
        assert_eq!(rib.curves.len(), 3);
    }

    #[test]
    fn detects_circular_npth_holes_and_ignores_slots_and_smd_pads() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        assert_eq!(reading.source.mounting_holes.len(), 2);
        let hole = &reading.source.mounting_holes[0];
        assert_eq!(hole.reference.as_deref(), Some("H1"));
        assert_eq!(hole.drill_diameter, mm(3.2));
        assert_eq!(hole.position, Point2::new(mm(105.0), mm(-105.0)));
    }

    #[test]
    fn reports_unsupported_geometry_instead_of_approximating_it() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        assert!(
            reading.skipped.iter().any(|s| s.contains("bezier")),
            "skipped list was {:?}",
            reading.skipped
        );
    }

    #[test]
    fn reads_the_layer_table_with_display_names() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let user1 = reading.layers.iter().find(|l| l.canonical == "User.1").expect("User.1");
        assert_eq!(user1.id, 53);
        assert_eq!(user1.display.as_deref(), Some("Enclosure"));
    }

    #[test]
    fn records_footprint_positions_for_the_preview_lookup() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        assert_eq!(reading.footprints.len(), 3);
        let h1 = reading
            .footprints
            .iter()
            .find(|f| f.reference.as_deref() == Some("H1"))
            .expect("H1 is present");
        assert_eq!(h1.position, Point2::new(mm(105.0), mm(-105.0)));
    }

    /// The oracle is KiCad itself: `kicad-cli pcb export drill` on a footprint
    /// at (100, 50) turned 90 degrees with an `np_thru_hole` pad at local
    /// (1, 0) emits `X100.0Y-49.0`, i.e. file coordinates (100, 49).
    ///
    /// Regression: the rotation used to be applied with the wrong sense, which
    /// mirrored the hole about the footprint origin — (100, 51) here, and 2 mm
    /// out for a 1 mm pad offset.
    #[test]
    fn a_rotated_mounting_hole_lands_where_kicad_drills_it() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let hole = reading
            .source
            .mounting_holes
            .iter()
            .find(|h| h.reference.as_deref() == Some("H9"))
            .expect("H9 is present");
        assert_eq!(hole.position, Point2::new(mm(100.0), mm(-49.0)));
    }

    #[test]
    fn reads_the_side_rotation_and_models_of_a_footprint() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        let j1 = reading
            .footprints
            .iter()
            .find(|f| f.reference.as_deref() == Some("J1"))
            .expect("J1 is present");
        assert_eq!(j1.rotation, 90.0);
        assert!(!j1.on_back);
        // The second model is hidden, so KiCad does not draw it and neither
        // does KiCase.
        assert_eq!(j1.models.len(), 1);
        assert_eq!(j1.models[0].raw, "${KICAD9_3DMODEL_DIR}/Connector.3dshapes/USB_C.step");
        assert_eq!(j1.models[0].offset, [0.0, 0.0, 1.5]);
        assert_eq!(j1.models[0].scale, [1.0, 1.0, 1.0]);
        assert_eq!(j1.models[0].rotate, [0.0, 0.0, -90.0]);

        let h9 = reading
            .footprints
            .iter()
            .find(|f| f.reference.as_deref() == Some("H9"))
            .expect("H9 is present");
        assert!(h9.on_back);
        // A KiCad 5 board writes `(at (xyz ...))` and means inches.
        assert_eq!(h9.models[0].offset, [25.4, 0.0, 0.0]);
    }

    #[test]
    fn tracks_which_layers_carry_objects() {
        let reading = read_board(BOARD, &roles()).expect("parses");
        assert!(reading.used_layers.iter().any(|l| l == "Edge.Cuts"));
        assert!(reading.used_layers.iter().any(|l| l == "User.2"));
        // The rectangle referenced the cuts layer by its display name; it must
        // be recorded under the canonical name.
        assert!(reading.used_layers.iter().any(|l| l == "User.3"));
        assert!(!reading.used_layers.iter().any(|l| l == "User.5"));
    }

    /// Reading a board has to stay linear in its size.
    ///
    /// Regression: quoted strings were decoded a character at a time, and each
    /// character re-validated the rest of the file as UTF-8. A nine-megabyte
    /// board took over five minutes; the same board now takes tens of
    /// milliseconds. The bound here is loose enough never to be flaky and
    /// tight enough that quadratic behaviour blows straight through it.
    #[test]
    fn a_large_board_reads_in_reasonable_time() {
        let mut board = String::from(
            "(kicad_pcb (version 20241229) (general (thickness 1.6))\n\
             (layers (0 \"F.Cu\" signal) (47 \"Edge.Cuts\" user) (53 \"User.1\" user))\n",
        );
        // Tracks with net names: the strings are what used to be quadratic.
        for index in 0..40_000 {
            board.push_str(&format!(
                "(segment (start {index} 0) (end {index} 10) (width 0.25) \
                 (layer \"F.Cu\") (net 3) (uuid \"net-name-{index}-with-some-length\"))\n"
            ));
        }
        // Footprints carrying models, because FOOTPRINT_KEEP is the other half
        // of what makes this fast and a board of pure tracks cannot see a
        // regression in it.
        for index in 0..2_000 {
            board.push_str(&format!(
                "(footprint \"lib:part-{index}\" (layer \"F.Cu\") (at {index} 20 90)\n\
                 (uuid \"fp-{index}\") (property \"Reference\" \"R{index}\")\n\
                 (fp_line (start 0 0) (end 1 0) (layer \"F.SilkS\") (uuid \"s{index}\"))\n\
                 (model \"${{KICAD9_3DMODEL_DIR}}/Resistor_SMD.3dshapes/R_0402.step\"\n\
                   (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) (rotate (xyz 0 0 0)))\n)\n"
            ));
        }
        board.push_str("(gr_line (start 0 0) (end 10 0) (layer \"Edge.Cuts\") (uuid \"e1\"))\n)");

        let started = std::time::Instant::now();
        let reading = read_board(&board, &roles()).expect("parses");
        let taken = started.elapsed();

        assert_eq!(reading.source.board_outline.len(), 1, "the one graphic was found");
        assert_eq!(reading.footprints.len(), 2_000, "every footprint was read");
        assert!(
            reading.footprints.iter().all(|fp| fp.models.len() == 1),
            "every footprint kept its model reference"
        );
        assert!(
            taken < std::time::Duration::from_secs(5),
            "reading a {} KB board took {taken:?}",
            board.len() / 1000
        );
    }

    #[test]
    fn rejects_documents_that_are_not_boards() {
        let err = read_board("(kicad_sch (version 1))", &roles()).expect_err("must fail");
        assert!(matches!(err, ReadError::NotABoard));
    }
}
