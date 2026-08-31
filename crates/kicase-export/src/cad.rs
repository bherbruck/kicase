//! STEP and STL output.

use crate::error::{ExportError, Result};
use crate::paths::ExportPaths;
use kicase_geometry::kernel::{CadKernel, NamedSolid};
use kicase_geometry::types::{Transform3d, Vector3};
use kicase_geometry::units::Length;
use kicase_model::builder::EnclosureSolids;
use std::path::PathBuf;

/// The files produced by one export run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportedFiles {
    pub step: Vec<PathBuf>,
    pub stl: Vec<PathBuf>,
    pub scad: Vec<PathBuf>,
}

impl ExportedFiles {
    pub fn all(&self) -> impl Iterator<Item = &PathBuf> {
        self.step.iter().chain(&self.stl).chain(&self.scad)
    }
}

/// Writes `bottom.step`, `lid.step` and the combined `enclosure.step`.
///
/// The combined file is the one KiCad's 3D viewer loads, so the lid is written
/// in its assembled position.
///
/// `preview_origin` is where the preview footprint sits on the board, in
/// enclosure coordinates. KiCad places a footprint's 3D model relative to the
/// footprint, so the assembly is shifted by the negative of that to make the
/// enclosure land around the board wherever the footprint was dropped. The
/// per-part files stay in board coordinates, which is what a CAD or slicing
/// tool wants.
pub fn export_step<K: CadKernel>(
    kernel: &K,
    solids: &EnclosureSolids<K>,
    paths: &ExportPaths,
    preview_origin: Vector3,
) -> Result<Vec<PathBuf>> {
    ensure_dir(&paths.generated_dir)?;

    let bottom = paths.bottom_step();
    let lid = paths.lid_step();
    let assembly = paths.enclosure_step();

    kernel.export_step(&solids.bottom, &bottom)?;
    kernel.export_step(&solids.lid, &lid)?;

    let shift = Transform3d::translation(Vector3::ZERO - preview_origin);
    let shifted_bottom = kernel.transform(&solids.bottom, shift)?;
    let shifted_lid = kernel.transform(&solids.lid, shift)?;
    kernel.export_step_assembly(
        &[
            NamedSolid { name: "bottom", solid: &shifted_bottom },
            NamedSolid { name: "lid", solid: &shifted_lid },
        ],
        &assembly,
    )?;

    // The same two parts again, separately, for the preview footprint: KiCad
    // can show and hide a footprint's 3D models one at a time, so keeping them
    // apart is what lets the lid be lifted off in the viewer.
    let preview_bottom = paths.preview_bottom_step();
    let preview_lid = paths.preview_lid_step();
    kernel.export_step(&shifted_bottom, &preview_bottom)?;
    kernel.export_step(&shifted_lid, &preview_lid)?;

    Ok(vec![bottom, lid, assembly, preview_bottom, preview_lid])
}

/// Writes printable `bottom.stl` and `lid.stl`.
pub fn export_stl<K: CadKernel>(
    kernel: &K,
    solids: &EnclosureSolids<K>,
    paths: &ExportPaths,
    tolerance: Length,
) -> Result<Vec<PathBuf>> {
    ensure_dir(&paths.generated_dir)?;

    let bottom = paths.bottom_stl();
    let lid = paths.lid_stl();
    kernel.export_stl(&solids.bottom, &bottom, tolerance)?;
    kernel.export_stl(&solids.lid, &lid, tolerance)?;

    Ok(vec![bottom, lid])
}

pub(crate) fn ensure_dir(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).map_err(|source| ExportError::io(dir.display(), source))
}
