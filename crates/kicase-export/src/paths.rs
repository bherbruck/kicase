//! Where generated files live.
//!
//! Everything KiCase writes is project-local and reproducible; nothing is
//! stashed in the user's home directory, the registry or a temporary folder.

use kicase_model::config::{CONFIG_FILE, GENERATED_DIR, OPENSCAD_DIR, PROJECT_DIR};
use std::path::{Path, PathBuf};

/// Resolved output paths for one KiCad project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPaths {
    pub project_dir: PathBuf,
    pub enclosure_dir: PathBuf,
    pub generated_dir: PathBuf,
    pub openscad_dir: PathBuf,
}

impl ExportPaths {
    pub fn new(project_dir: impl AsRef<Path>) -> Self {
        let project_dir = project_dir.as_ref().to_path_buf();
        let enclosure_dir = project_dir.join(PROJECT_DIR);
        ExportPaths {
            generated_dir: enclosure_dir.join(GENERATED_DIR),
            openscad_dir: enclosure_dir.join(OPENSCAD_DIR),
            enclosure_dir,
            project_dir,
        }
    }

    pub fn config(&self) -> PathBuf {
        self.enclosure_dir.join(CONFIG_FILE)
    }

    pub fn bottom_step(&self) -> PathBuf {
        self.generated_dir.join("bottom.step")
    }

    pub fn lid_step(&self) -> PathBuf {
        self.generated_dir.join("lid.step")
    }

    /// The combined assembly, in preview coordinates.
    pub fn enclosure_step(&self) -> PathBuf {
        self.generated_dir.join("enclosure.step")
    }

    /// The bottom shell in preview coordinates, attached to the preview
    /// footprint as its own 3D model so it can be hidden independently.
    pub fn preview_bottom_step(&self) -> PathBuf {
        self.generated_dir.join("preview-bottom.step")
    }

    /// The lid in preview coordinates, attached as its own 3D model.
    pub fn preview_lid_step(&self) -> PathBuf {
        self.generated_dir.join("preview-lid.step")
    }

    pub fn bottom_stl(&self) -> PathBuf {
        self.generated_dir.join("bottom.stl")
    }

    pub fn lid_stl(&self) -> PathBuf {
        self.generated_dir.join("lid.stl")
    }

    pub fn generated_scad(&self) -> PathBuf {
        self.openscad_dir.join("generated.scad")
    }

    /// Never overwritten once it exists.
    pub fn custom_scad(&self) -> PathBuf {
        self.openscad_dir.join("custom.scad")
    }

    /// Project-local footprint library holding the preview footprint.
    pub fn preview_library_dir(&self) -> PathBuf {
        self.enclosure_dir.join("kicase.pretty")
    }

    /// Paths KiCad stores in the preview footprint, using its own project
    /// variable so the board stays portable.
    ///
    /// The bottom and the lid are attached as two separate models, because
    /// KiCad lets a footprint's models be shown and hidden one at a time — so
    /// hiding the lid to look inside the case needs no support from KiCase.
    pub fn preview_model_references() -> [&'static str; 2] {
        [
            "${KIPRJMOD}/.enclosure/generated/preview-bottom.step",
            "${KIPRJMOD}/.enclosure/generated/preview-lid.step",
        ]
    }
}
