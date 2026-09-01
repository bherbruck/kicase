//! Reading a STEP model into triangles, for the viewport only.
//!
//! Footprints point at STEP files — KiCad ships about seven thousand of them —
//! and a connector you cannot see is a connector you cannot check against a
//! wall. These meshes are decoration: they are [`TriangleMesh`] and never a
//! `CadKernel::Solid`, so nothing here can reach the enclosure booleans or the
//! STEP and STL exports, which take solids.

use crate::kernel::append_triangles;
use kicase_geometry::error::{GeometryError, Result};
use kicase_geometry::types::TriangleMesh;
use kicase_geometry::units::{mm, Length};
use monstertruck_io::step::load::Table;
use monstertruck_meshing::prelude::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Chord tolerance for component models.
///
/// Deliberately unrelated to the enclosure's boolean tolerance: nothing here
/// is ever cut, so this is a pixels-and-memory decision rather than a
/// correctness one. Measured over 600 of KiCad's shipped models, 0.05 mm gives
/// a median 1.8k triangles and a 99th percentile of 27k, and costs nothing in
/// time — parsing the STEP text is 79% of the work and tessellation only 14%,
/// so 0.01 mm buys four to eight times the triangles for the same clock. Going
/// coarser is what actually breaks: at 0.5 mm a spline-heavy inductor collapses
/// from 442 triangles to 40 and stops looking like the part.
pub const COMPONENT_MESH_TOLERANCE: Length = mm(0.05);

/// Turns the text of a STEP file into one merged mesh, in the model's own
/// coordinates (millimetres, origin at the footprint origin).
///
/// Every solid in the file is merged rather than kept apart. Splitting would
/// only pay for itself if it bought per-solid colour, and it does not:
/// monstertruck-io reads no `COLOUR_RGB` or `STYLED_ITEM` at all, and KiCad's
/// colour is per-face anyway. Merging also keeps a 97-solid DIN connector one
/// draw call instead of ninety-seven.
pub fn load_step_mesh(bytes: &[u8], tolerance: Length) -> Result<TriangleMesh> {
    GeometryError::require_positive("component mesh tolerance", tolerance)?;
    // A model is arbitrary third-party data reached through a parser we do not
    // own, and one bad file must still leave the enclosure on screen.
    catch_unwind(AssertUnwindSafe(|| load(bytes, tolerance))).unwrap_or_else(|_| {
        Err(GeometryError::kernel("load_step_mesh", "the STEP reader panicked on this model"))
    })
}

fn load(bytes: &[u8], tolerance: Length) -> Result<TriangleMesh> {
    let table = Table::from_step_bytes(bytes)
        .map_err(|err| GeometryError::kernel("load_step_mesh", err.to_string()))?;

    // Every B-rep in the file, rather than the products an assembly walk finds.
    //
    // KiCad's models are single-product files — none of the ones sampled has a
    // NEXT_ASSEMBLY_USAGE_OCCURRENCE — so there is no placement to apply, and
    // the walk is what *loses* geometry: a vendor AP214 model whose product
    // representation holds only placements yields zero shells through it while
    // holding 24 solids the table can see directly.
    let mut out = TriangleMesh::default();
    let mut shells = 0usize;
    let mut failures = 0usize;

    for solid in table.manifold_solid_brep.values() {
        match table.to_compressed_solid(solid) {
            Ok(solid) => {
                for shell in solid.boundaries {
                    shells += 1;
                    append_shell(&mut out, shell, tolerance);
                }
            },
            Err(_) => failures += 1,
        }
    }
    for model in table.shell_based_surface_model.values() {
        match table.to_compressed_shells(model) {
            Ok(converted) => {
                for shell in converted {
                    shells += 1;
                    append_shell(&mut out, shell, tolerance);
                }
            },
            Err(_) => failures += 1,
        }
    }

    if shells == 0 {
        return Err(GeometryError::kernel(
            "load_step_mesh",
            format!("the file holds no solid or shell this reader understands ({failures} failed)"),
        ));
    }
    if out.is_empty() {
        return Err(GeometryError::kernel(
            "load_step_mesh",
            format!("{shells} shell(s) tessellated to nothing"),
        ));
    }
    Ok(out)
}

/// Tessellates one shell and adds it to the mesh being built.
///
/// No `add_smooth_normals` here, unlike the kernel's own `mesh()`: truck's
/// extruded solids arrive without normals, but a triangulated STEP shell
/// carries true per-face ones from its analytic surfaces. Smoothing them would
/// round off the hard edges of a chip package or a pin header, which is the
/// wrong look for a clearance check.
fn append_shell<S>(out: &mut TriangleMesh, shell: S, tolerance: Length)
where
    S: RobustMeshableShape,
    S::MeshedShape: MeshedShape,
{
    let mut polygon = shell.robust_triangulation(tolerance.mm()).to_polygon();
    polygon.triangulate();
    append_triangles(out, &polygon);
}
