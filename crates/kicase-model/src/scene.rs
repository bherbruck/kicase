//! The displayable scene: the enclosure and the board as separate, named parts.
//!
//! Kept kernel-agnostic and free of any UI type, so the viewport only has to
//! draw triangles and the same scene can be reused for tests or another
//! front end.

use crate::builder::EnclosureSolids;
use crate::error::Result;
use crate::model::Enclosure;
use kicase_geometry::kernel::CadKernel;
use kicase_geometry::types::{Bounds3, Plane3, Point3, TriangleMesh};
use kicase_geometry::units::{mm, Length};

/// Chord tolerance used when triangulating for display. Fine enough that a
/// 3 mm fillet looks round, coarse enough to stay instant.
pub const DISPLAY_TOLERANCE: Length = mm(0.08);

/// Which part of the assembly a mesh belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PartId {
    /// The PCB itself.
    Pcb,
    /// The bottom shell.
    Bottom,
    /// The lid.
    Lid,
}

impl PartId {
    pub const ALL: [PartId; 3] = [PartId::Pcb, PartId::Bottom, PartId::Lid];

    pub fn label(self) -> &'static str {
        match self {
            PartId::Pcb => "PCB",
            PartId::Bottom => "Enclosure bottom",
            PartId::Lid => "Enclosure lid",
        }
    }

    /// Linear RGB, chosen so the three parts stay distinct when a section cuts
    /// through all of them.
    pub fn color(self) -> [f32; 3] {
        match self {
            PartId::Pcb => [0.07, 0.35, 0.20],
            PartId::Bottom => [0.78, 0.79, 0.82],
            PartId::Lid => [0.42, 0.52, 0.68],
        }
    }
}

/// One drawable part.
#[derive(Debug, Clone)]
pub struct ScenePart {
    pub id: PartId,
    pub mesh: TriangleMesh,
}

/// Everything the viewport draws.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub parts: Vec<ScenePart>,
}

impl Scene {
    pub fn part(&self, id: PartId) -> Option<&ScenePart> {
        self.parts.iter().find(|part| part.id == id)
    }

    /// Bounding box of every part, for framing the camera and for sizing the
    /// section sweep.
    pub fn bounds(&self) -> Option<Bounds3> {
        self.parts.iter().filter_map(|part| part.mesh.bounds()).reduce(|a, b| Bounds3 {
            min: Point3::new(a.min.x.min(b.min.x), a.min.y.min(b.min.y), a.min.z.min(b.min.z)),
            max: Point3::new(a.max.x.max(b.max.x), a.max.y.max(b.max.y), a.max.z.max(b.max.z)),
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|part| part.mesh.triangle_count()).sum()
    }
}

/// Triangulates the enclosure and the board into a drawable scene.
pub fn build_scene<K: CadKernel>(
    kernel: &K,
    enclosure: &Enclosure,
    solids: &EnclosureSolids<K>,
    tolerance: Length,
) -> Result<Scene> {
    build_scene_of(kernel, enclosure, &solids.bottom, &solids.lid, tolerance)
}

/// The same, for a caller holding the two parts on their own.
///
/// The designer window keeps its geometry in pieces so that an edit only
/// rebuilds the pieces it moved, and never has a whole [`EnclosureSolids`] in
/// hand to draw from.
pub fn build_scene_of<K: CadKernel>(
    kernel: &K,
    enclosure: &Enclosure,
    bottom: &K::Solid,
    lid: &K::Solid,
    tolerance: Length,
) -> Result<Scene> {
    let mut parts = Vec::with_capacity(3);

    // The board, drawn from its own outline so the enclosure can be checked
    // against the thing it has to fit around.
    let layout = enclosure.layout;
    let board_profile =
        kernel.make_profile(&enclosure.board_profile, &Plane3::xy_at(layout.pcb_bottom))?;
    let board = kernel.extrude(&board_profile, layout.pcb_top - layout.pcb_bottom)?;
    parts.push(ScenePart { id: PartId::Pcb, mesh: kernel.mesh(&board, tolerance)? });

    parts.push(ScenePart { id: PartId::Bottom, mesh: kernel.mesh(bottom, tolerance)? });
    parts.push(ScenePart { id: PartId::Lid, mesh: kernel.mesh(lid, tolerance)? });

    Ok(Scene { parts })
}
