//! Geometry-level errors.
//!
//! Errors carry enough context (a location, an index) for the KiCad adapter to
//! map them back onto the graphic the user actually drew.

use crate::types::Point2;
use crate::units::Length;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, GeometryError>;

#[derive(Debug, Error)]
pub enum GeometryError {
    #[error(
        "contour is not closed: nothing continues from ({}, {}){}",
        .at.x,
        .at.y,
        .nearest_gap
            .map(|g| format!(", and the nearest loose end is {g} away"))
            .unwrap_or_default()
    )]
    OpenContour {
        at: Point2,
        curve_index: usize,
        /// Distance to the closest unused endpoint, when there is one.
        nearest_gap: Option<Length>,
    },

    #[error("contour has {count} disconnected closed regions; expected exactly one")]
    DisconnectedContours { count: usize },

    #[error("no closed contour was found")]
    EmptyContour,

    #[error("contour contains a zero-length curve at index {curve_index}")]
    DegenerateCurve { curve_index: usize },

    #[error("{name} must be greater than zero (got {value})")]
    NonPositive { name: &'static str, value: Length },

    #[error("{name} is out of range: {value} (expected {expected})")]
    OutOfRange { name: &'static str, value: Length, expected: &'static str },

    #[error("kernel operation '{operation}' failed: {reason}")]
    KernelFailure { operation: &'static str, reason: String },

    #[error("kernel operation '{operation}' is not supported by this backend")]
    Unsupported { operation: &'static str },

    #[error("i/o error writing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl GeometryError {
    pub fn kernel(operation: &'static str, reason: impl Into<String>) -> Self {
        GeometryError::KernelFailure { operation, reason: reason.into() }
    }

    /// Validates that a user-supplied dimension is strictly positive.
    pub fn require_positive(name: &'static str, value: Length) -> Result<Length> {
        if value.is_positive() && value.is_finite() {
            Ok(value)
        } else {
            Err(GeometryError::NonPositive { name, value })
        }
    }
}
