//! Model and build errors.
//!
//! Every variant is phrased as something a user can act on, and carries the
//! KiCad UUID of the offending object where one exists so the caller can select
//! it in the PCB editor.

use kicase_geometry::error::GeometryError;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, ModelError>;

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("enclosure.toml has version {found}, but this build of KiCase understands version {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },

    #[error("enclosure.toml could not be parsed: {0}")]
    Parse(String),

    #[error("enclosure.toml could not be written: {0}")]
    Serialize(String),

    #[error("i/o error for {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no enclosure project found at {path}; run `kicase init` first")]
    NotInitialized { path: String },

    #[error("Edge.Cuts is empty; draw a closed board outline first")]
    NoBoardOutline,

    #[error("board outline could not be closed: {0}")]
    BoardOutline(#[source] GeometryError),

    #[error(
        "nothing is drawn on the Enclosure layer. Draw the wall there as a closed \
         outline: the path is the middle of the wall, the line width is its thickness, \
         and any arcs are the corner radii."
    )]
    NoEnclosureOutline,

    #[error("Enclosure outline could not be closed: {0}")]
    EnclosureOutline(#[source] GeometryError),

    #[error("datum \"{id}\" references a deleted KiCad graphic ({uuid})")]
    OrphanedDatum { id: String, uuid: String },

    #[error("feature \"{id}\" references a deleted KiCad graphic ({uuid})")]
    OrphanedFeature { id: String, uuid: String },

    #[error("cutout \"{id}\" references datum \"{datum}\", which does not exist")]
    UnknownDatum { id: String, datum: String },

    #[error("datum \"{id}\" must be a straight line on the datum layer")]
    DatumNotALine { id: String },

    #[error("datum \"{id}\" is zero length; draw a longer line")]
    DatumZeroLength { id: String },

    #[error("feature \"{id}\" must be a closed shape (rectangle, circle or closed polygon)")]
    FeatureNotClosed { id: String },

    #[error("duplicate id \"{id}\" in enclosure.toml")]
    DuplicateId { id: String },

    #[error("{name} must be greater than zero (got {value} mm)")]
    NonPositive { name: &'static str, value: f64 },

    #[error("the case cannot be built as described: {detail}")]
    ImpossibleStack { detail: String },

    #[error("geometry error: {0}")]
    Geometry(#[from] GeometryError),
}

impl ModelError {
    pub fn io(path: impl std::fmt::Display, source: std::io::Error) -> Self {
        ModelError::Io { path: path.to_string(), source }
    }
}

/// A non-fatal problem. Generation continues, and the message is surfaced in
/// the UI and on the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub message: String,
    /// KiCad UUID of the object the warning is about, when there is one.
    pub uuid: Option<String>,
}

impl Warning {
    pub fn new(message: impl Into<String>) -> Self {
        Warning { message: message.into(), uuid: None }
    }

    pub fn about(uuid: impl Into<String>, message: impl Into<String>) -> Self {
        Warning { message: message.into(), uuid: Some(uuid.into()) }
    }
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.uuid {
            Some(uuid) => write!(f, "{} [{}]", self.message, uuid),
            None => write!(f, "{}", self.message),
        }
    }
}
