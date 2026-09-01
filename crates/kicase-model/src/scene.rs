//! The displayable scene: the enclosure and the board as separate, named parts.
//!
//! Kept kernel-agnostic and free of any UI type, so the viewport only has to
//! draw triangles and the same scene can be reused for tests or another
//! front end.

use crate::builder::EnclosureSolids;
use crate::error::Result;
use crate::model::Enclosure;
use kicase_geometry::kernel::CadKernel;
use kicase_geometry::types::{Bounds3, Plane3, Point3, Transform3d, TriangleMesh, Vector3};
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
    /// The parts the footprints carry 3D models for.
    ///
    /// Display only. Components exist as triangles and never as a kernel
    /// solid, which is what keeps them out of the enclosure geometry, the
    /// exports and the fitment booleans — every one of those takes a solid, so
    /// the compiler enforces it rather than a convention.
    Components,
}

impl PartId {
    pub const ALL: [PartId; 4] = [PartId::Pcb, PartId::Bottom, PartId::Lid, PartId::Components];

    pub fn label(self) -> &'static str {
        match self {
            PartId::Pcb => "PCB",
            PartId::Bottom => "Enclosure bottom",
            PartId::Lid => "Enclosure lid",
            PartId::Components => "Components",
        }
    }

    /// Linear RGB, chosen so the parts stay distinct when a section cuts
    /// through all of them.
    pub fn color(self) -> [f32; 3] {
        match self {
            PartId::Pcb => [0.07, 0.35, 0.20],
            PartId::Bottom => [0.78, 0.79, 0.82],
            PartId::Lid => [0.42, 0.52, 0.68],
            // Dark and matte, so components read as what is being checked
            // rather than as part of the enclosure.
            PartId::Components => [0.18, 0.18, 0.21],
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

    /// Bounding box of the enclosure and the board, for framing the camera and
    /// for sizing the section sweep.
    ///
    /// Components are left out on purpose. They come from third-party files
    /// this code did not write, and one model that arrives a metre wide — a
    /// STEP file authored in inches, say — would otherwise push the camera and
    /// the section slider off the enclosure entirely.
    pub fn bounds(&self) -> Option<Bounds3> {
        self.parts
            .iter()
            .filter(|part| part.id != PartId::Components)
            .filter_map(|part| part.mesh.bounds())
            .reduce(|a, b| Bounds3 {
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

/// One component model, placed on the board.
#[derive(Debug, Clone, Copy)]
pub struct ComponentInstance<'a> {
    /// Model space to world: scale, then rotate, then translate.
    pub transform: Transform3d,
    /// The same rotation without the scale, for normals. A model scaled
    /// unevenly would otherwise have its normals skewed away from its faces.
    pub normals: Transform3d,
    /// The model's mesh in its own coordinates, shared between every footprint
    /// that references the same file.
    pub mesh: &'a TriangleMesh,
}

/// Merges placed component models into the one part the viewport draws.
///
/// One part rather than one per component: the viewport issues a draw call and
/// a colour per [`ScenePart`], and the section is a per-fragment clip on world
/// position, so a merged mesh sections exactly as a solid does — while two
/// hundred parts would mean two hundred draw calls and two hundred checkboxes.
///
/// Nothing here touches the CAD kernel, and the result is a mesh. Components
/// cannot become enclosure geometry by accident because there is no path from
/// a mesh back to a solid.
pub fn components_part<'a>(
    instances: impl IntoIterator<Item = ComponentInstance<'a>>,
) -> ScenePart {
    let mut mesh = TriangleMesh::default();
    for instance in instances {
        let base = mesh.positions.len() as u32;
        for (index, position) in instance.mesh.positions.iter().enumerate() {
            mesh.positions.push(instance.transform.apply(*position));
            let normal = instance.mesh.normals.get(index).copied().unwrap_or(Vector3::Z);
            mesh.normals
                .push(instance.normals.apply_vector(normal).normalized().unwrap_or(Vector3::Z));
        }
        // Placement is a rotation, even on the back of the board — a part
        // mounted underneath is turned over, not reflected — so this normally
        // does nothing. A board that scales a model by a negative factor does
        // reflect it, and the shader would then read every face as back-facing
        // and paint the whole component as cut-open material.
        let mirrored = instance.transform.determinant() < 0.0;
        let (triangles, _) = instance.mesh.indices.as_chunks::<3>();
        for triangle in triangles {
            let [a, b, c] = triangle.map(|index| index + base);
            if mirrored {
                mesh.indices.extend_from_slice(&[a, c, b]);
            } else {
                mesh.indices.extend_from_slice(&[a, b, c]);
            }
        }
    }
    ScenePart { id: PartId::Components, mesh }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kicase_geometry::types::{Point3, Vector3};

    fn block(size: f64) -> TriangleMesh {
        TriangleMesh {
            positions: vec![
                Point3::ZERO,
                Point3::from_mm(size, 0.0, 0.0),
                Point3::from_mm(0.0, size, size),
            ],
            normals: vec![Vector3::Z; 3],
            indices: vec![0, 1, 2],
        }
    }

    /// A component the size of a room — a model authored in metres, say — must
    /// not drag the camera and the section slider off the enclosure.
    #[test]
    fn framing_ignores_the_components() {
        let scene = Scene {
            parts: vec![
                ScenePart { id: PartId::Pcb, mesh: block(10.0) },
                ScenePart { id: PartId::Components, mesh: block(4000.0) },
            ],
        };
        let bounds = scene.bounds().expect("the board has bounds");
        assert_eq!(bounds.max.x, mm(10.0));
        // The components are still drawn, and still counted.
        assert_eq!(scene.triangle_count(), 2);
    }

    /// A mirrored placement reverses every triangle, or the shader reads the
    /// whole component as back-facing and paints it as cut-open material.
    #[test]
    fn a_mirrored_placement_reverses_the_winding() {
        let mesh = block(1.0);
        let mut mirror = Transform3d::IDENTITY;
        mirror.x_axis = Vector3::new(mm(-1.0), Length::ZERO, Length::ZERO);
        let part = components_part([
            ComponentInstance {
                transform: Transform3d::IDENTITY,
                normals: Transform3d::IDENTITY,
                mesh: &mesh,
            },
            ComponentInstance { transform: mirror, normals: mirror, mesh: &mesh },
        ]);
        assert_eq!(part.mesh.indices[..3], [0, 1, 2]);
        assert_eq!(part.mesh.indices[3..], [3, 5, 4]);
    }
}
