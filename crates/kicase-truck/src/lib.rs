//! A pure-Rust B-rep backend for KiCase, built on truck.
//!
//! Same [`CadKernel`](kicase_geometry::kernel::CadKernel) contract as the
//! OpenCascade backend, with no C++ toolchain to install and nothing to
//! cross-compile: `cargo build` is the whole story on every platform.

mod convert;
mod cylinders;
mod kernel;

pub use kernel::{TruckKernel, TruckProfile, TruckSolid};
