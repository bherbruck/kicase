//! Export errors.

use kicase_geometry::error::GeometryError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ExportError>;

#[derive(Debug, Error)]
pub enum ExportError {
    #[error("could not write {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("geometry export failed: {0}")]
    Geometry(#[from] GeometryError),
}

impl ExportError {
    pub fn io(path: impl std::fmt::Display, source: std::io::Error) -> Self {
        ExportError::Io { path: path.to_string(), source }
    }
}
