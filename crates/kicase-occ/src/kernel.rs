//! [`CadKernel`] implemented on OpenCascade.

use crate::convert::{from_dvec3, loop_to_wire, to_dvec3};
use crate::measure;
use glam::{dvec3, DVec3};
use kicase_geometry::error::{GeometryError, Result};
use kicase_geometry::kernel::{CadKernel, NamedSolid};
use kicase_geometry::profile::Profile2d;
use kicase_geometry::types::{Bounds3, Plane3, Transform3d, TriangleMesh};
use kicase_geometry::units::Length;
use opencascade::primitives::{Face, Shape, Wire};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

/// A closed contour living on a known plane in 3D.
pub struct OccProfile {
    outer: Wire,
    holes: Vec<Wire>,
    plane: Plane3,
}

impl OccProfile {
    fn to_face(&self) -> Face {
        if self.holes.is_empty() {
            Face::from_wire(&self.outer)
        } else {
            Face::from_wire_with_holes(&self.outer, &self.holes)
        }
    }

    pub fn plane(&self) -> &Plane3 {
        &self.plane
    }
}

/// An opaque solid body.
pub struct OccSolid {
    shape: Shape,
}

impl OccSolid {
    fn new(shape: Shape) -> Self {
        Self { shape }
    }
}

impl AsRef<Shape> for OccSolid {
    fn as_ref(&self) -> &Shape {
        &self.shape
    }
}

/// The OpenCascade B-rep backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct OccKernel;

impl OccKernel {
    pub fn new() -> Self {
        OccKernel
    }
}

/// Runs an OpenCascade call that is known to fail loudly on awkward topology,
/// converting a panic into an ordinary error so the caller can degrade
/// gracefully instead of taking the whole plugin down.
fn guarded<T>(operation: &'static str, f: impl FnOnce() -> T) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => Ok(value),
        Err(_) => Err(GeometryError::kernel(operation, "OpenCascade rejected this topology")),
    }
}

impl CadKernel for OccKernel {
    type Profile = OccProfile;
    type Solid = OccSolid;

    fn make_profile(&self, profile: &Profile2d, plane: &Plane3) -> Result<Self::Profile> {
        let outer = loop_to_wire(&profile.outer, plane)?;
        let holes = profile
            .holes
            .iter()
            .map(|hole| loop_to_wire(hole, plane))
            .collect::<Result<Vec<_>>>()?;
        Ok(OccProfile { outer, holes, plane: *plane })
    }

    fn extrude(&self, profile: &Self::Profile, distance: Length) -> Result<Self::Solid> {
        if distance.mm().abs() <= f64::EPSILON {
            return Err(GeometryError::NonPositive { name: "extrusion distance", value: distance });
        }
        let normal = profile.plane.normal();
        let dir = dvec3(normal.x.mm(), normal.y.mm(), normal.z.mm()) * distance.mm();
        let solid = guarded("extrude", || profile.to_face().extrude(dir))?;
        Ok(OccSolid::new(Shape::from(solid)))
    }

    fn union(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let shape = guarded("union", || Shape::from(a.shape.union(&b.shape)))?;
        Ok(OccSolid::new(shape))
    }

    fn subtract(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let shape = guarded("subtract", || Shape::from(a.shape.subtract(&b.shape)))?;
        Ok(OccSolid::new(shape))
    }

    fn intersect(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid> {
        let shape = guarded("intersect", || Shape::from(a.shape.intersect(&b.shape)))?;
        Ok(OccSolid::new(shape))
    }

    fn transform(&self, solid: &Self::Solid, transform: Transform3d) -> Result<Self::Solid> {
        let (axis, angle) = rotation_axis_angle(&transform)?;
        let translation = to_dvec3(transform.translation);
        let shape = guarded("transform", || {
            let rotated = if angle.abs() > 1e-12 {
                solid.shape.rotated(axis, angle)
            } else {
                solid.shape.translated(DVec3::ZERO)
            };
            rotated.translated(translation)
        })?;
        Ok(OccSolid::new(shape))
    }

    fn bounds(&self, solid: &Self::Solid) -> Result<Bounds3> {
        let bb = opencascade::bounding_box::aabb(&solid.shape);
        if bb.is_void() {
            return Err(GeometryError::kernel("bounds", "shape is empty"));
        }
        // Bnd_Box inflates by its gap; remove it so tests can assert real sizes.
        let gap = bb.gap_vec();
        Ok(Bounds3 { min: from_dvec3(bb.min() + gap), max: from_dvec3(bb.max() - gap) })
    }

    fn volume(&self, solid: &Self::Solid) -> Result<f64> {
        measure::volume(&solid.shape)
    }

    fn mesh(&self, solid: &Self::Solid, tolerance: Length) -> Result<TriangleMesh> {
        GeometryError::require_positive("mesh tolerance", tolerance)?;
        let mesh = solid
            .shape
            .mesh_with_tolerance(tolerance.mm())
            .map_err(|e| GeometryError::kernel("mesh", e.to_string()))?;

        Ok(TriangleMesh {
            positions: mesh.vertices.iter().map(|v| from_dvec3(*v)).collect(),
            normals: mesh.normals.iter().map(|n| from_dvec3(*n)).collect(),
            indices: mesh.indices.iter().map(|i| *i as u32).collect(),
        })
    }

    fn solid_count(&self, solid: &Self::Solid) -> Result<usize> {
        measure::solid_count(&solid.shape)
    }

    fn export_step(&self, solid: &Self::Solid, path: &Path) -> Result<()> {
        ensure_parent_dir(path)?;
        solid
            .shape
            .write_step(path)
            .map_err(|e| GeometryError::kernel("export_step", e.to_string()))
    }

    fn export_step_assembly(
        &self,
        solids: &[NamedSolid<'_, Self::Solid>],
        path: &Path,
    ) -> Result<()> {
        ensure_parent_dir(path)?;
        if solids.is_empty() {
            return Err(GeometryError::kernel("export_step_assembly", "no solids to write"));
        }
        let shapes: Vec<&Shape> = solids.iter().map(|named| &named.solid.shape).collect();
        Shape::write_all_step(shapes, path)
            .map_err(|e| GeometryError::kernel("export_step_assembly", e.to_string()))
    }

    fn export_stl(&self, solid: &Self::Solid, path: &Path, tolerance: Length) -> Result<()> {
        ensure_parent_dir(path)?;
        GeometryError::require_positive("stl tolerance", tolerance)?;
        solid
            .shape
            .write_stl_with_tolerance(path, tolerance.mm())
            .map_err(|e| GeometryError::kernel("export_stl", e.to_string()))
    }
}

impl OccKernel {}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| GeometryError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
    }
    Ok(())
}

/// Decomposes the rotational part of a transform into axis and angle.
fn rotation_axis_angle(t: &Transform3d) -> Result<(DVec3, f64)> {
    let trace = t.x_axis.x.mm() + t.y_axis.y.mm() + t.z_axis.z.mm();
    let cos_angle = ((trace - 1.0) / 2.0).clamp(-1.0, 1.0);
    let angle = cos_angle.acos();

    if angle.abs() < 1e-12 {
        return Ok((DVec3::Z, 0.0));
    }
    let sin_angle = angle.sin();
    if sin_angle.abs() < 1e-9 {
        // A 180 degree rotation needs a different decomposition; KiCase never
        // builds one, so refuse rather than emit silently wrong geometry.
        return Err(GeometryError::Unsupported { operation: "180 degree transform" });
    }
    let axis = dvec3(
        t.z_axis.y.mm() - t.y_axis.z.mm(),
        t.x_axis.z.mm() - t.z_axis.x.mm(),
        t.y_axis.x.mm() - t.x_axis.y.mm(),
    ) / (2.0 * sin_angle);
    Ok((axis.normalize(), angle))
}
