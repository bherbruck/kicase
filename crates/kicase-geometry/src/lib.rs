//! Neutral geometry primitives and the CAD kernel abstraction for KiCase.
//!
//! This crate is the boundary layer of the whole project:
//!
//! ```text
//! KiCad protobuf / board file
//!       -> kicase-kicad
//!       -> neutral geometry (this crate)
//!       -> semantic enclosure model
//!       -> CadKernel (this crate)
//!       -> OpenCascade
//! ```
//!
//! It depends on neither KiCad nor any CAD kernel.

pub mod error;
pub mod kernel;
pub mod profile;
pub mod types;
pub mod units;

pub use error::{GeometryError, Result};
pub use kernel::{CadKernel, NamedSolid};
pub use profile::{
    assemble_loops, assemble_profile, assemble_single_region, miter_join, Curve2, Loop2, Profile2d,
    JOIN_TOL,
};
pub use types::{
    Arc2, Bounds2, Bounds3, Circle2, LineSegment2, Plane3, Point2, Point3, Polygon2, Transform3d,
    TriangleMesh, Vector2, Vector3, TOL,
};
pub use units::{mm, Angle, Length};
