//! What the user drew, expressed in neutral geometry.
//!
//! `kicase-kicad` fills this in from a live KiCad board; the model and build
//! pipeline consume it without knowing where it came from, which is what lets
//! the geometry tests run headlessly.

use kicase_geometry::profile::Curve2;
use kicase_geometry::types::{LineSegment2, Point2};
use kicase_geometry::units::Length;

/// A KiCad object's persistent identifier.
///
/// Objects are *only* ever identified by this. Never by position, never by
/// name, never by layer alone.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KiCadUuid(pub String);

impl KiCadUuid {
    pub fn new(id: impl Into<String>) -> Self {
        KiCadUuid(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for KiCadUuid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which enclosure layer a graphic was found on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRole {
    /// `Edge.Cuts`: the PCB outline.
    BoardOutline,
    /// `Enclosure`: an optional user-drawn case outline.
    Outline,
    /// `Enclosure.Datums`.
    Datums,
    /// `Enclosure.Cuts`: openings through a side wall, via a datum.
    Cuts,
    /// `Enclosure.Top`: holes straight through the lid.
    Top,
    /// `Enclosure.Bottom`: holes straight through the floor.
    Bottom,
    /// `Enclosure.Solids`.
    Solids,
}

/// One graphic object as drawn in KiCad, already converted to millimetres and
/// to a Y-up coordinate system.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardGraphic {
    pub uuid: KiCadUuid,
    pub role: LayerRole,
    /// The curves making up this single graphic. A rectangle contributes four,
    /// a circle two half-arcs, a line exactly one.
    pub curves: Vec<Curve2>,
    /// True when this graphic on its own forms a closed region.
    pub closed: bool,
    /// The stroke width KiCad draws this graphic with.
    ///
    /// On the enclosure outline layer this is not decoration: the stroke *is*
    /// the wall, so its width is the wall thickness.
    pub stroke_width: Option<Length>,
}

impl BoardGraphic {
    /// Interprets the graphic as a single straight line, for datums.
    pub fn as_line(&self) -> Option<LineSegment2> {
        match self.curves.as_slice() {
            [Curve2::Line(line)] => Some(*line),
            _ => None,
        }
    }
}

/// A candidate PCB mounting hole: a non-plated, circular through hole.
#[derive(Debug, Clone, PartialEq)]
pub struct MountingHole {
    pub uuid: KiCadUuid,
    /// Footprint reference, when the hole came from one.
    pub reference: Option<String>,
    pub position: Point2,
    pub drill_diameter: Length,
}

/// Everything KiCase reads from a board.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoardSource {
    /// Raw, unordered `Edge.Cuts` curves.
    pub board_outline: Vec<Curve2>,
    /// Raw, unordered curves from the `Enclosure` outline layer.
    pub enclosure_outline: Vec<Curve2>,
    /// Stroke width of each curve in `enclosure_outline`, one for one.
    ///
    /// The outline is drawn at true size: the width of each line is the
    /// thickness of the wall along that stretch, so a thick back wall and thin
    /// sides are simply drawn that way.
    pub enclosure_outline_widths: Vec<Length>,
    /// Graphics from the datum, cut and solid layers.
    pub graphics: Vec<BoardGraphic>,
    /// Detected mounting-hole candidates.
    pub mounting_holes: Vec<MountingHole>,
    /// Board thickness, as the board itself states it. Never a setting: KiCad
    /// already knows this and the stackup is the authority.
    pub board_thickness: Option<Length>,
}

impl BoardSource {
    pub fn graphic(&self, uuid: &str) -> Option<&BoardGraphic> {
        self.graphics.iter().find(|g| g.uuid.as_str() == uuid)
    }

    pub fn graphics_on(&self, role: LayerRole) -> impl Iterator<Item = &BoardGraphic> {
        self.graphics.iter().filter(move |g| g.role == role)
    }

    pub fn mounting_hole(&self, uuid: &str) -> Option<&MountingHole> {
        self.mounting_holes.iter().find(|h| h.uuid.as_str() == uuid)
    }
}
