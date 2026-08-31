//! The KiCase application: project handling, the build pipeline, and the glue
//! between the designer window and everything else.

pub mod backend;
pub mod pipeline;
pub mod project;
pub mod watcher;

pub use backend::AppBackend;
pub use pipeline::{init, rebuild, validate, RebuildOptions, RebuildReport};
pub use project::{Origin, Project};
pub use watcher::{BoardWatcher, WatchSource};

/// The CAD backend the app builds with. Re-exported so callers and tests
/// follow the app's choice rather than naming a kernel of their own.
pub use kicase_truck::TruckKernel as Kernel;
