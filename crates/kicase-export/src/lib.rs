//! Generated output: STEP, STL, the OpenSCAD derivative, and the KiCad preview
//! footprint.
//!
//! STEP is the canonical artefact. OpenSCAD is an optional editable derivative
//! and never feeds back into the model.

pub mod cad;
pub mod error;
pub mod openscad;
pub mod paths;
pub mod preview;

pub use cad::{export_step, export_stl, ExportedFiles};
pub use error::{ExportError, Result};
pub use openscad::export_openscad;
pub use paths::ExportPaths;
pub use preview::{
    preview_footprint_for, preview_footprint_sexpr, write_preview_library, PREVIEW_LIBRARY,
    PREVIEW_REFERENCE,
};
