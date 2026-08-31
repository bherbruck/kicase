//! The resolved semantic enclosure model.
//!
//! [`Enclosure::resolve`] turns `enclosure.toml` plus what the user drew into a
//! self-contained description of the enclosure. It never touches a CAD kernel,
//! so it is cheap, testable and reusable by the UI for validation feedback.

use crate::config::{CutFace, DatumNormal, EnclosureConfig};
use crate::error::{ModelError, Result, Warning};
use crate::source::{BoardSource, KiCadUuid, LayerRole};
use kicase_geometry::profile::{assemble_single_region, Curve2, Loop2, Profile2d, JOIN_TOL};
use kicase_geometry::types::{LineSegment2, Plane3, Point2, Point3, Vector2, Vector3};
use kicase_geometry::units::Length;

/// Used only when a board does not state its own thickness.
pub const DEFAULT_BOARD_THICKNESS: Length = kicase_geometry::units::mm(1.6);

/// Vertical layout of the enclosure.
///
/// Zero is the **outside bottom of the case** — the surface it stands on. Every
/// height in KiCase is measured from there, including datums, so there is one
/// origin to reason about rather than several.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZLayout {
    /// Outside of the case floor.
    pub case_bottom: Length,
    /// Inside of the case floor; the PCB standoffs start here.
    pub cavity_floor: Length,
    /// Underside of the PCB. Always zero, by definition.
    pub pcb_bottom: Length,
    /// Top copper surface of the PCB.
    pub pcb_top: Length,
    /// Top rim of the bottom shell, where the lid lands.
    pub rim: Length,
    /// Outside of the lid.
    pub lid_top: Length,
}

impl ZLayout {
    /// `board_thickness` comes from the board itself, never from a setting.
    pub fn from_config(config: &EnclosureConfig, board_thickness: Length) -> Self {
        let shell = &config.shell;
        ZLayout {
            case_bottom: Length::ZERO,
            cavity_floor: shell.floor,
            pcb_bottom: shell.pcb_height,
            pcb_top: shell.pcb_height + board_thickness,
            rim: shell.total_height - config.lid.thickness,
            lid_top: shell.total_height,
        }
    }

    /// Overall exterior height of the assembled enclosure.
    pub fn total_height(&self) -> Length {
        self.lid_top - self.case_bottom
    }

    /// Height of the bottom shell on its own.
    pub fn shell_height(&self) -> Length {
        self.rim - self.case_bottom
    }

    /// Interior cavity height, floor to rim.
    pub fn cavity_height(&self) -> Length {
        self.rim - self.cavity_floor
    }
}

/// A vertical reference plane derived from a line drawn on the datum layer.
///
/// The plane's local axes are the ones the specification calls for:
/// `U` runs along the datum line, `V` is world Z, and `N` is the horizontal
/// wall normal.
#[derive(Debug, Clone, PartialEq)]
pub struct SideDatum {
    pub id: String,
    pub uuid: KiCadUuid,
    /// The line as drawn, in board XY.
    pub line: LineSegment2,
    /// Z of the datum line: always the bottom of the case.
    pub z: Length,
    /// Horizontal wall normal.
    pub normal: Vector2,
    pub plane: Plane3,
}

impl SideDatum {
    /// Direction along the datum line.
    pub fn u_dir(&self) -> Vector2 {
        self.line.direction().unwrap_or(Vector2::new(Length::ZERO, Length::ZERO))
    }
}

/// Where a cutting solid gets swept from.
#[derive(Debug, Clone, PartialEq)]
pub enum CutPlacement {
    /// Through a side wall, along a datum normal.
    Side { datum: String, plane: Plane3 },
    /// Straight down or up, from the face it was drawn on.
    Vertical { face: CutFace },
}

/// Material removed from the enclosure.
#[derive(Debug, Clone, PartialEq)]
pub struct Cutout {
    pub id: String,
    pub uuid: KiCadUuid,
    /// Profile in the coordinates of its placement plane.
    pub profile: Profile2d,
    pub placement: CutPlacement,
    pub clearance: Length,
    /// How far the cut reaches in from the surface it starts at.
    ///
    /// `None` leaves it to the placement: a side opening goes through its wall,
    /// a top or bottom hole goes the default depth.
    pub depth: Option<Length>,
}

/// Material added to the enclosure: a boss, a rib or a plain extrusion.
#[derive(Debug, Clone, PartialEq)]
pub struct AddedSolid {
    pub id: String,
    pub uuid: KiCadUuid,
    /// Profile in board XY.
    pub profile: Profile2d,
    pub z_start: Length,
    pub height: Length,
}

/// A wall drawn inside the enclosure outline: a divider or a compartment.
///
/// It is read exactly like the outline is — the drawn path is the centre line
/// and the stroke width is the thickness, half either side — because it is the
/// same thing, drawn in the same place, and a wall the user put inside the box
/// is still a wall.
#[derive(Debug, Clone, PartialEq)]
pub struct Island {
    /// Centre line in board XY.
    pub outline: Loop2,
    /// Thickness along each curve of `outline`, in the same order.
    pub widths: Vec<Length>,
}

/// The shell: an outline, wall and floor thicknesses, and its Z extent.
#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    /// Interior cavity outline in board XY.
    pub cavity_profile: Profile2d,
    /// Interior walls, outermost first.
    ///
    /// The builder hollows each island by boring its whole interior from the
    /// cavity floor past the rim, so an island nested inside another has to be
    /// welded on after the one enclosing it or that bore would erase it.
    pub islands: Vec<Island>,
    /// Thickness of the wall along each curve of `cavity_profile.outer`, in the
    /// same order. Each drawn segment keeps the width it was drawn with.
    pub wall_widths: Vec<Length>,
    /// How the drawn outline relates to the wall.
    ///
    /// When the user draws the outline, the stroke they drew *is* the wall:
    /// the path is its centre line and the stroke width is its thickness, so
    /// what KiCad shows in 2D is the wall at true size. Nothing is derived from
    /// a setting in that case.
    pub wall_from_drawing: bool,
    pub wall: Length,
    pub floor: Length,
}

/// Lid parameters, resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lid {
    pub thickness: Length,
    pub fit_clearance: Length,
    pub lip_depth: Length,
    pub lip_thickness: Length,
}

/// A project entry whose KiCad graphic no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub id: String,
    pub uuid: String,
    pub kind: OrphanKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanKind {
    Datum,
    Feature,
    MountingHole,
}

/// The complete semantic enclosure.
#[derive(Debug, Clone, PartialEq)]
pub struct Enclosure {
    pub shell: Shell,
    pub lid: Lid,
    pub layout: ZLayout,
    /// The PCB outline, kept for the preview and for diagnostics.
    pub board_profile: Profile2d,
    pub datums: Vec<SideDatum>,
    pub cutouts: Vec<Cutout>,
    pub solids: Vec<AddedSolid>,
    /// Entries in `enclosure.toml` whose graphics were deleted in KiCad.
    /// They are never re-bound to another object by guesswork.
    pub orphans: Vec<Orphan>,
    pub warnings: Vec<Warning>,
    /// Things worth telling the user that are not problems, such as a setting
    /// being inert because the drawing supersedes it.
    pub notes: Vec<String>,
}

impl Enclosure {
    /// Builds the semantic model from the project file and the board.
    pub fn resolve(config: &EnclosureConfig, source: &BoardSource) -> Result<Self> {
        config.validate()?;

        let mut warnings = Vec::new();
        let mut orphans = Vec::new();
        let mut notes: Vec<String> = Vec::new();

        if source.board_outline.is_empty() {
            return Err(ModelError::NoBoardOutline);
        }
        let board_profile = assemble_single_region(&source.board_outline, JOIN_TOL)
            .map_err(ModelError::BoardOutline)?;

        // The wall is drawn, never derived: there is no clearance setting to
        // guess a shape from. If nothing is on the Enclosure layer there is
        // nothing to build, and saying so beats inventing an outline.
        if source.enclosure_outline.is_empty() {
            return Err(ModelError::NoEnclosureOutline);
        }
        let cavity_profile = assemble_single_region(&source.enclosure_outline, JOIN_TOL)
            .map_err(ModelError::EnclosureOutline)?;

        // A drawn outline is taken completely literally: its stroke width is
        // the wall thickness and its arcs are the corner radii. The settings
        // for those two only fill in when there is nothing drawn.
        let drawn_wall = drawn_wall_thickness(&source.enclosure_outline_widths);
        // Reassembly reorders and reverses curves, so the widths are matched
        // back onto the assembled loop by geometry rather than by position.
        let wall_widths = match_widths(
            cavity_profile.outer.curves(),
            &source.enclosure_outline,
            &source.enclosure_outline_widths,
            drawn_wall.unwrap_or(config.shell.wall),
        );
        let wall = drawn_wall.unwrap_or(config.shell.wall);
        if let Some(drawn) = drawn_wall {
            notes.push(format!(
                "wall thickness {drawn} comes from the width of the outline you drew, \
                 not from the wall setting"
            ));
        }

        // A closed loop drawn inside the outline is another wall, with its own
        // stroke width per segment. Sorted here rather than relied on from the
        // assembler, so that a change to how contours are ordered cannot
        // silently start deleting nested dividers in the builder.
        let mut islands: Vec<Island> = cavity_profile
            .holes
            .iter()
            .map(|hole| Island {
                widths: match_widths(
                    hole.curves(),
                    &source.enclosure_outline,
                    &source.enclosure_outline_widths,
                    wall,
                ),
                outline: hole.clone(),
            })
            .collect();
        islands.sort_by(|a, b| {
            let (a, b) = (a.outline.signed_area().abs(), b.outline.signed_area().abs());
            b.partial_cmp(&a).unwrap_or(std::cmp::Ordering::Equal)
        });

        let layout =
            ZLayout::from_config(config, source.board_thickness.unwrap_or(DEFAULT_BOARD_THICKNESS));
        let center = cavity_profile.bounds().center();

        // --- datums -------------------------------------------------------
        let mut datums: Vec<SideDatum> = Vec::new();
        for datum_config in &config.datums {
            let Some(graphic) = source.graphic(&datum_config.graphic_uuid) else {
                orphans.push(Orphan {
                    id: datum_config.id.clone(),
                    uuid: datum_config.graphic_uuid.clone(),
                    kind: OrphanKind::Datum,
                });
                warnings.push(Warning::about(
                    &datum_config.graphic_uuid,
                    format!("datum \"{}\" references a deleted KiCad graphic", datum_config.id),
                ));
                continue;
            };
            if graphic.role != LayerRole::Datums {
                warnings.push(Warning::about(
                    graphic.uuid.as_str(),
                    format!(
                        "datum \"{}\" points at a graphic that is no longer on the datum layer",
                        datum_config.id
                    ),
                ));
            }
            let Some(line) = graphic.as_line() else {
                return Err(ModelError::DatumNotALine { id: datum_config.id.clone() });
            };
            let Some(u_dir) = line.direction() else {
                return Err(ModelError::DatumZeroLength { id: datum_config.id.clone() });
            };

            // The wall normal is the line direction turned 90 degrees; which of
            // the two choices is used is either explicit or, by default, the
            // one pointing away from the enclosure centre.
            let right = Vector2::new(u_dir.y, -u_dir.x);
            let normal = match datum_config.normal {
                DatumNormal::Right => right,
                DatumNormal::Left => -right,
                DatumNormal::Auto => {
                    let outward = line.midpoint() - center;
                    if right.dot(outward) >= 0.0 {
                        right
                    } else {
                        -right
                    }
                },
            };

            // The drawn line is the bottom edge of the case wall.
            let z = layout.case_bottom;
            let origin = Point3::new(line.start.x, line.start.y, z);
            let u3 = Vector3::new(u_dir.x, u_dir.y, Length::ZERO);
            // U along the line, V vertical: exactly the sketch plane in the
            // specification. The plane normal follows from u x v.
            let mut plane = Plane3::new(origin, u3, Vector3::Z);
            let plane_normal = plane.normal();
            if plane_normal.xy().dot(normal) < 0.0 {
                // Flip the plane so its normal matches the chosen wall normal.
                plane = Plane3::new(origin, -u3, Vector3::Z);
            }

            datums.push(SideDatum {
                id: datum_config.id.clone(),
                uuid: KiCadUuid::new(&datum_config.graphic_uuid),
                line,
                z,
                normal,
                plane,
            });
        }

        // --- features -----------------------------------------------------
        // Every closed shape on a feature layer counts, with no entry needed:
        // the layer says what it does. An entry in the project file only adds
        // what a drawing cannot carry — a datum, a clearance, a height.
        let mut cutouts = Vec::new();
        let mut solids = Vec::new();

        for graphic in &source.graphics {
            let role = graphic.role;
            if !matches!(
                role,
                LayerRole::Cuts | LayerRole::Top | LayerRole::Bottom | LayerRole::Solids
            ) {
                continue;
            }
            let uuid = graphic.uuid.to_string();
            let entry = config.features.iter().find(|f| f.graphic_uuid == uuid);
            if entry.is_some_and(|f| !f.enabled) {
                continue;
            }
            let id = entry.map(|f| f.id.clone()).unwrap_or_else(|| default_id(role, &uuid));

            if !graphic.closed {
                warnings.push(Warning::about(
                    &uuid,
                    format!(
                        "\"{id}\" is not a closed shape, so it was skipped. Draw a rectangle, \
                         circle or closed polygon."
                    ),
                ));
                continue;
            }
            let Ok(profile) = assemble_single_region(&graphic.curves, JOIN_TOL) else {
                warnings.push(Warning::about(
                    &uuid,
                    format!("\"{id}\" could not be closed into a region, so it was skipped"),
                ));
                continue;
            };

            match role {
                LayerRole::Top | LayerRole::Bottom => {
                    let face = if role == LayerRole::Top { CutFace::Top } else { CutFace::Bottom };
                    cutouts.push(Cutout {
                        id,
                        uuid: KiCadUuid::new(&uuid),
                        profile,
                        placement: CutPlacement::Vertical { face },
                        clearance: entry.map(|f| f.clearance).unwrap_or(Length::ZERO),
                        depth: entry.and_then(|f| f.depth).filter(|d| d.is_positive()),
                    });
                },
                LayerRole::Cuts => {
                    // A side opening has to know which wall it belongs to, and
                    // that cannot be read off the drawing.
                    let Some(datum_id) = entry.and_then(|f| f.datum.clone()) else {
                        warnings.push(Warning::about(
                            &uuid,
                            format!(
                                "\"{id}\" is on the cuts layer but is not attached to a datum, \
                                 so it was skipped. Add it as a cutout against a datum."
                            ),
                        ));
                        continue;
                    };
                    let Some(datum) = datums.iter().find(|d| d.id == datum_id) else {
                        warnings.push(Warning::about(
                            &uuid,
                            format!(
                                "\"{id}\" was skipped: its datum \"{datum_id}\" is unavailable"
                            ),
                        ));
                        continue;
                    };
                    let folded = unfold_onto_datum(&profile, datum, &mut warnings)?;
                    cutouts.push(Cutout {
                        id,
                        uuid: KiCadUuid::new(&uuid),
                        profile: folded,
                        placement: CutPlacement::Side {
                            datum: datum.id.clone(),
                            plane: datum.plane,
                        },
                        clearance: entry.map(|f| f.clearance).unwrap_or(Length::ZERO),
                        depth: entry.and_then(|f| f.depth).filter(|d| d.is_positive()),
                    });
                },
                LayerRole::Solids => {
                    // A drawn shape on the solids layer rises from the cavity
                    // floor to the underside of the board by default, which
                    // makes a plain circle a standoff without saying anything.
                    let z_start = entry.and_then(|f| f.z_start).unwrap_or(layout.cavity_floor);
                    let height = entry
                        .and_then(|f| f.height)
                        .filter(|h| h.is_positive())
                        .unwrap_or(layout.pcb_bottom - z_start);
                    if !height.is_positive() {
                        warnings.push(Warning::about(
                            &uuid,
                            format!(
                                "\"{id}\" has no height above the cavity floor, so it was skipped"
                            ),
                        ));
                        continue;
                    }
                    solids.push(AddedSolid {
                        id,
                        uuid: KiCadUuid::new(&uuid),
                        profile,
                        z_start,
                        height,
                    });
                },
                _ => {},
            }
        }

        // Entries naming a graphic that is gone are orphans, never re-bound.
        for feature in &config.features {
            if source.graphic(&feature.graphic_uuid).is_none() {
                orphans.push(Orphan {
                    id: feature.id.clone(),
                    uuid: feature.graphic_uuid.clone(),
                    kind: OrphanKind::Feature,
                });
                warnings.push(Warning::about(
                    &feature.graphic_uuid,
                    format!("feature \"{}\" references a deleted KiCad graphic", feature.id),
                ));
            }
        }

        Ok(Enclosure {
            shell: Shell {
                cavity_profile,
                islands,
                wall,
                floor: config.shell.floor,
                wall_widths,
                wall_from_drawing: drawn_wall.is_some(),
            },
            lid: Lid {
                thickness: config.lid.thickness,
                fit_clearance: config.lid.fit_clearance,
                lip_depth: config.lid.lip_depth,
                lip_thickness: config.lid.lip_thickness,
            },
            layout,
            board_profile,
            datums,
            cutouts,
            solids,
            orphans,
            warnings,
            notes,
        })
    }
}

/// A readable name for a shape nobody has named, from its layer and UUID.
fn default_id(role: LayerRole, uuid: &str) -> String {
    let prefix = match role {
        LayerRole::Top => "top",
        LayerRole::Bottom => "bottom",
        LayerRole::Cuts => "cut",
        LayerRole::Solids => "solid",
        _ => "feature",
    };
    let short: String = uuid.chars().take(8).collect();
    format!("{prefix}-{short}")
}

/// Pairs each curve of the assembled outline with the width it was drawn at.
///
/// Curves come back from assembly reordered and sometimes reversed, so they are
/// matched on their endpoints rather than their position in the list.
fn match_widths(
    assembled: &[Curve2],
    drawn: &[Curve2],
    widths: &[Length],
    fallback: Length,
) -> Vec<Length> {
    let same = |a: Point2, b: Point2| (a - b).length() <= JOIN_TOL;
    assembled
        .iter()
        .map(|curve| {
            drawn
                .iter()
                .zip(widths.iter())
                .find(|(candidate, _)| {
                    (same(candidate.start(), curve.start()) && same(candidate.end(), curve.end()))
                        || (same(candidate.start(), curve.end())
                            && same(candidate.end(), curve.start()))
                })
                .map(|(_, width)| *width)
                .filter(|w| w.is_positive())
                .unwrap_or(fallback)
        })
        .collect()
}

/// The wall thickness the user drew.
///
/// This is only the *representative* thickness, used for messages and for the
/// lid's lip. Each segment keeps its own width; see [`Shell::wall_widths`].
fn drawn_wall_thickness(widths: &[Length]) -> Option<Length> {
    let usable: Vec<Length> = widths.iter().copied().filter(|w| w.is_positive()).collect();
    let first = *usable.first()?;

    let widest = usable.iter().copied().fold(first, Length::max);
    Some(widest)
}

/// Folds a shape drawn next to a datum line up onto that datum's plane.
///
/// The rule is deliberately simple, because it has to be readable from the
/// drawing alone:
///
/// * distance **along** the line becomes `U`, the position across the wall;
/// * distance **from** the line becomes `V`, the height up the wall.
///
/// So the edge of the shape nearest the datum line becomes the bottom of the
/// opening, and the further an edge is drawn from the line, the higher up the
/// wall it lands. Which side of the line you draw on makes no difference.
fn unfold_onto_datum(
    profile: &Profile2d,
    datum: &SideDatum,
    warnings: &mut Vec<Warning>,
) -> Result<Profile2d> {
    let u_dir = datum
        .line
        .direction()
        .ok_or_else(|| ModelError::DatumZeroLength { id: datum.id.clone() })?;
    let perp = u_dir.perpendicular();
    let origin = datum.line.start;

    // Height is the unsigned distance from the line, so the side the shape was
    // drawn on is irrelevant. A shape that straddles the line would fold onto
    // itself, which is never what was meant, so say so.
    let bounds = profile.bounds();
    let corners = [
        bounds.min,
        Point2::new(bounds.max.x, bounds.min.y),
        bounds.max,
        Point2::new(bounds.min.x, bounds.max.y),
    ];
    let sides: Vec<f64> = corners.iter().map(|c| (*c - origin).dot(perp)).collect();
    if sides.iter().any(|d| *d < 0.0) && sides.iter().any(|d| *d > 0.0) {
        warnings.push(Warning::new(format!(
            "the shape for datum \"{}\" crosses the datum line; move it fully to one side, \
             since distance from the line is the height up the wall",
            datum.id
        )));
    }

    // The plane's U axis may have been flipped to satisfy the requested normal;
    // follow it so that the shape lands the right way round on the wall.
    let plane_u = datum.plane.u.xy();
    let u_sign = if plane_u.dot(u_dir) < 0.0 { -1.0 } else { 1.0 };

    let map = |p: Point2| -> Point2 {
        let d = p - origin;
        Point2::from_mm(d.dot(u_dir) * u_sign, d.dot(perp).abs())
    };

    Ok(Profile2d::new(
        map_loop(&profile.outer, map),
        profile.holes.iter().map(|hole| map_loop(hole, map)).collect(),
    ))
}

fn map_loop(loop2: &Loop2, map: impl Fn(Point2) -> Point2 + Copy) -> Loop2 {
    let curves: Vec<Curve2> = loop2
        .curves()
        .iter()
        .map(|curve| match curve {
            Curve2::Line(l) => Curve2::line(map(l.start), map(l.end)),
            Curve2::Arc(a) => Curve2::arc(map(a.start), map(a.mid), map(a.end)),
        })
        .collect();
    Loop2::from_ordered(curves).expect("a rigid map preserves closure")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DatumConfig, FeatureConfig};
    use crate::source::BoardGraphic;
    use kicase_geometry::units::mm;

    fn rect_curves(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Curve2> {
        Loop2::rectangle(Point2::from_mm(x0, y0), Point2::from_mm(x1, y1)).curves().to_vec()
    }

    fn board_source() -> BoardSource {
        // A 54 x 34 wall centre line drawn 2 mm wide, around a 50 x 30 board.
        let outline = rect_curves(-2.0, -2.0, 52.0, 32.0);
        BoardSource {
            board_outline: rect_curves(0.0, 0.0, 50.0, 30.0),
            enclosure_outline_widths: vec![mm(2.0); outline.len()],
            enclosure_outline: outline,
            ..BoardSource::default()
        }
    }

    #[test]
    fn every_height_is_measured_from_the_bottom_of_the_case() {
        let config = EnclosureConfig::default();
        let layout = ZLayout::from_config(&config, mm(1.6));
        // Defaults: 15 mm tall, PCB 6 mm up, 2 mm bottom, 2 mm lid.
        assert_eq!(layout.case_bottom, Length::ZERO, "the case bottom is the origin");
        assert_eq!(layout.cavity_floor, mm(2.0));
        assert_eq!(layout.pcb_bottom, mm(6.0));
        assert_eq!(layout.pcb_top, mm(7.6));
        assert_eq!(layout.rim, mm(13.0));
        assert_eq!(layout.lid_top, mm(15.0));
        assert_eq!(layout.total_height(), mm(15.0));
    }

    #[test]
    fn resolves_a_board_with_a_drawn_wall() {
        let config = EnclosureConfig::default();
        let enclosure = Enclosure::resolve(&config, &board_source()).expect("resolves");
        // The wall came from the drawing, not from the setting.
        assert!(enclosure.shell.wall_from_drawing);
        assert_eq!(enclosure.shell.wall, mm(2.0));
        assert_eq!(enclosure.shell.cavity_profile.bounds().width(), mm(54.0));
    }

    #[test]
    fn a_board_with_no_drawn_wall_is_an_actionable_error() {
        let mut source = board_source();
        source.enclosure_outline.clear();
        source.enclosure_outline_widths.clear();
        let err = Enclosure::resolve(&EnclosureConfig::default(), &source)
            .expect_err("there is nothing to build");
        assert!(matches!(err, ModelError::NoEnclosureOutline));
    }

    #[test]
    fn datum_plane_axes_match_the_specification() {
        let mut source = board_source();
        source.graphics.push(BoardGraphic {
            uuid: KiCadUuid::new("datum-1"),
            role: LayerRole::Datums,
            // A line along +X at y = 0: the front wall.
            curves: vec![Curve2::line(Point2::from_mm(0.0, 0.0), Point2::from_mm(50.0, 0.0))],
            closed: false,
            stroke_width: None,
        });
        let mut config = EnclosureConfig::default();
        config.datums.push(DatumConfig {
            id: "front".into(),
            graphic_uuid: "datum-1".into(),
            normal: DatumNormal::Auto,
        });

        let enclosure = Enclosure::resolve(&config, &source).expect("resolves");
        let datum = &enclosure.datums[0];
        // U runs along the line, V is vertical.
        assert!((datum.plane.u.xy().length().mm() - 1.0).abs() < 1e-9);
        assert_eq!(datum.plane.v, Vector3::Z);
        // The board centre is at y = 15, so "auto" points at -Y, away from it.
        assert!(datum.normal.y.mm() < 0.0, "normal was {:?}", datum.normal);
        // The drawn line is the bottom of the case, always.
        assert_eq!(datum.z, Length::ZERO);
    }

    #[test]
    fn side_cutout_is_folded_onto_its_datum() {
        let mut source = board_source();
        source.graphics.push(BoardGraphic {
            uuid: KiCadUuid::new("datum-1"),
            role: LayerRole::Datums,
            curves: vec![Curve2::line(Point2::from_mm(0.0, 0.0), Point2::from_mm(50.0, 0.0))],
            closed: false,
            stroke_width: None,
        });
        // A 10 x 4 opening drawn 2 mm above the datum line, starting at x = 20.
        source.graphics.push(BoardGraphic {
            uuid: KiCadUuid::new("cut-1"),
            role: LayerRole::Cuts,
            curves: rect_curves(20.0, 2.0, 30.0, 6.0),
            closed: true,
            stroke_width: None,
        });

        let mut config = EnclosureConfig::default();
        config.datums.push(DatumConfig {
            id: "front".into(),
            graphic_uuid: "datum-1".into(),
            normal: DatumNormal::Auto,
        });
        config.features.push(FeatureConfig {
            id: "usb".into(),
            graphic_uuid: "cut-1".into(),
            datum: Some("front".into()),
            depth: None,
            clearance: Length::ZERO,
            z_start: None,
            height: None,
            enabled: true,
        });

        let enclosure = Enclosure::resolve(&config, &source).expect("resolves");
        let cutout = &enclosure.cutouts[0];
        let bounds = cutout.profile.bounds();
        // Distance along the line becomes U, distance from the line becomes V.
        assert!((bounds.width().mm() - 10.0).abs() < 1e-9);
        assert!((bounds.height().mm() - 4.0).abs() < 1e-9);
        assert!((bounds.min.y.mm() - 2.0).abs() < 1e-9, "V was {:?}", bounds.min.y);
        assert!(matches!(cutout.placement, CutPlacement::Side { .. }));
    }

    #[test]
    fn deleted_graphics_become_orphans_and_are_never_rebound() {
        let mut source = board_source();
        // A graphic exists, but it is not the one the project file names.
        source.graphics.push(BoardGraphic {
            uuid: KiCadUuid::new("some-other-graphic"),
            role: LayerRole::Datums,
            curves: vec![Curve2::line(Point2::from_mm(0.0, 0.0), Point2::from_mm(50.0, 0.0))],
            closed: false,
            stroke_width: None,
        });
        let mut config = EnclosureConfig::default();
        config.datums.push(DatumConfig {
            id: "front".into(),
            graphic_uuid: "deleted-uuid".into(),
            normal: DatumNormal::Auto,
        });

        let enclosure = Enclosure::resolve(&config, &source).expect("resolution continues");
        assert!(enclosure.datums.is_empty());
        assert_eq!(enclosure.orphans.len(), 1);
        assert_eq!(enclosure.orphans[0].kind, OrphanKind::Datum);
        assert_eq!(enclosure.orphans[0].uuid, "deleted-uuid");
        assert_eq!(enclosure.warnings.len(), 1);
    }

    #[test]
    fn empty_edge_cuts_is_an_actionable_error() {
        let config = EnclosureConfig::default();
        let err = Enclosure::resolve(&config, &BoardSource::default()).expect_err("must fail");
        assert!(matches!(err, ModelError::NoBoardOutline));
    }
}
