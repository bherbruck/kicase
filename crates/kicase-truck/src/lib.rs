//! A pure-Rust B-rep backend for KiCase, built on truck.
//!
//! Same [`CadKernel`](kicase_geometry::kernel::CadKernel) contract as the
//! OpenCascade backend, with no C++ toolchain to install and nothing to
//! cross-compile: `cargo build` is the whole story on every platform.

mod convert;
mod cylinders;
mod kernel;
mod step_import;

pub use kernel::{TruckKernel, TruckProfile, TruckSolid};
pub use step_import::{load_step_mesh, COMPONENT_MESH_TOLERANCE};
