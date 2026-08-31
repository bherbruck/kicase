//! OpenCascade backend for KiCase.
//!
//! This crate is the *only* place in the project that may name an OpenCascade
//! type. Everything it exposes is either a neutral `kicase-geometry` type or an
//! opaque handle.

mod convert;
mod kernel;
mod measure;

pub use kernel::{OccKernel, OccProfile, OccSolid};
