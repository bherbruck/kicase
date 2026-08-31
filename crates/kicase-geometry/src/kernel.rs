//! The CAD kernel abstraction.
//!
//! Everything above this trait — the KiCad adapter, `enclosure.toml`, datum
//! behaviour, the UI — is written against [`CadKernel`] and never against a
//! concrete kernel. Swapping OpenCascade for another B-rep kernel must not
//! require changes outside the backend crate that implements this trait.

use crate::error::Result;
use crate::profile::Profile2d;
use crate::types::{Bounds3, Plane3, Transform3d, TriangleMesh};
use crate::units::Length;
use std::path::Path;

/// A named solid, used when writing multi-part STEP assemblies.
pub struct NamedSolid<'a, S> {
    pub name: &'a str,
    pub solid: &'a S,
}

/// A B-rep modelling backend.
///
/// Implementations own their native topology types; callers only ever hold the
/// opaque associated types.
///
/// Deliberately small. Anything that can be done in neutral geometry — curve
/// offsetting, strokes, corner mitres — is done there instead, so that a new
/// backend has only to place profiles, extrude, boolean, transform and export.
pub trait CadKernel {
    /// A closed planar contour living on a specific plane in 3D.
    type Profile;
    /// A 3D solid body.
    type Solid;

    /// Places a neutral 2D profile onto `plane`, mapping profile X to the
    /// plane's `u` axis and profile Y to its `v` axis.
    fn make_profile(&self, profile: &Profile2d, plane: &Plane3) -> Result<Self::Profile>;

    /// Extrudes a profile along its plane normal.
    fn extrude(&self, profile: &Self::Profile, distance: Length) -> Result<Self::Solid>;

    fn union(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid>;

    fn subtract(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid>;

    fn intersect(&self, a: &Self::Solid, b: &Self::Solid) -> Result<Self::Solid>;

    /// Applies a rigid transform.
    fn transform(&self, solid: &Self::Solid, transform: Transform3d) -> Result<Self::Solid>;

    /// Axis-aligned bounding box, used by geometry tests and diagnostics.
    fn bounds(&self, solid: &Self::Solid) -> Result<Bounds3>;

    /// Volume in cubic millimetres, used by geometry tests.
    fn volume(&self, solid: &Self::Solid) -> Result<f64>;
    fn mesh(&self, solid: &Self::Solid, tolerance: Length) -> Result<TriangleMesh>;

    /// Number of disjoint solid bodies, used by geometry tests.
    fn solid_count(&self, solid: &Self::Solid) -> Result<usize>;

    fn export_step(&self, solid: &Self::Solid, path: &Path) -> Result<()>;

    /// Writes several named solids into one STEP assembly.
    fn export_step_assembly(
        &self,
        solids: &[NamedSolid<'_, Self::Solid>],
        path: &Path,
    ) -> Result<()>;

    fn export_stl(&self, solid: &Self::Solid, path: &Path, tolerance: Length) -> Result<()>;
}
