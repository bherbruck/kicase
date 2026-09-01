//! Opening a project, from a live KiCad session or from a board file.
//!
//! KiCad 10's IPC API needs a running GUI, so the live path is the normal one.
//! The file path exists because geometry work — rebuilding, exporting,
//! validating — does not need KiCad at all once the board has been saved, and
//! that is what makes KiCase scriptable and testable.

use anyhow::{anyhow, Context, Result};
use kicase_kicad::board::{read_board, BoardReading, LayerRoles};
use kicase_kicad::client::KiCadSession;
use kicase_model::config::EnclosureConfig;
use kicase_model::ModelError;
use std::path::{Path, PathBuf};

/// Where board geometry comes from.
pub enum Origin {
    /// A running KiCad instance.
    Live(Box<KiCadSession>),
    /// A saved `.kicad_pcb` file.
    File(PathBuf),
}

impl Origin {
    pub fn is_live(&self) -> bool {
        matches!(self, Origin::Live(_))
    }

    pub fn session(&self) -> Option<&KiCadSession> {
        match self {
            Origin::Live(session) => Some(session),
            Origin::File(_) => None,
        }
    }
}

/// An open KiCase project.
pub struct Project {
    pub dir: PathBuf,
    pub config: EnclosureConfig,
    pub origin: Origin,
    /// True when `enclosure.toml` did not exist and defaults are in use.
    pub is_new: bool,
}

impl Project {
    /// Connects to KiCad and loads the project file for the open board.
    pub fn open_live() -> Result<Self> {
        let session = KiCadSession::connect()?;
        let dir = session.project_dir()?;
        let (config, is_new) = load_or_default(&dir)?;
        Ok(Project { dir, config, origin: Origin::Live(Box::new(session)), is_new })
    }

    /// Opens a saved board file without touching KiCad.
    pub fn open_file(board: &Path) -> Result<Self> {
        if !board.exists() {
            return Err(anyhow!("no such board file: {}", board.display()));
        }
        let dir = board
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("board file has no parent directory"))?;
        let (config, is_new) = load_or_default(&dir)?;
        Ok(Project { dir, config, origin: Origin::File(board.to_path_buf()), is_new })
    }

    /// The board file this project is about.
    ///
    /// Known outright when working from a file. With KiCad on the other end
    /// only the project directory is available, so the board is the one
    /// `.kicad_pcb` in it — a KiCad project has exactly one.
    pub fn board_file(&self) -> Result<PathBuf> {
        if let Origin::File(path) = &self.origin {
            return Ok(path.clone());
        }
        let mut boards: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .with_context(|| format!("reading {}", self.dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "kicad_pcb"))
            .collect();
        boards.sort();
        match boards.len() {
            1 => Ok(boards.remove(0)),
            0 => Err(anyhow!("no .kicad_pcb in {}", self.dir.display())),
            n => {
                Err(anyhow!("{n} board files in {}; cannot tell which is open", self.dir.display()))
            },
        }
    }

    /// Opens whichever source is available: KiCad if a board path was not
    /// given, otherwise the file.
    pub fn open(board: Option<&Path>) -> Result<Self> {
        match board {
            Some(path) => Project::open_file(path),
            None => Project::open_live(),
        }
    }

    /// The board document as text, from KiCad or from the file.
    pub fn board_text(&self) -> Result<String> {
        match &self.origin {
            Origin::Live(session) => Ok(session.board_text()?),
            Origin::File(path) => {
                std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
            },
        }
    }

    /// Converts board text that has already been fetched.
    pub fn read_board_text(&self, text: &str) -> Result<BoardReading> {
        let roles = LayerRoles::from_mapping(&self.config.layers);
        Ok(read_board(text, &roles)?)
    }

    /// Reads the board and converts it to neutral geometry.
    pub fn read_board(&self) -> Result<BoardReading> {
        match &self.origin {
            Origin::Live(session) => Ok(session.read_board(&self.config.layers)?),
            Origin::File(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let roles = LayerRoles::from_mapping(&self.config.layers);
                Ok(read_board(&text, &roles)?)
            },
        }
    }

    pub fn save_config(&self) -> Result<()> {
        self.config.save(&self.dir).with_context(|| {
            format!("writing {}", EnclosureConfig::config_path(&self.dir).display())
        })
    }

    /// Highlights offending objects in KiCad, when there is a live session.
    pub fn select_in_kicad(&self, uuids: Vec<String>) {
        if let Origin::Live(session) = &self.origin {
            if let Err(err) = session.select(uuids) {
                tracing::debug!("could not select objects in KiCad: {err}");
            }
        }
    }
}

fn load_or_default(dir: &Path) -> Result<(EnclosureConfig, bool)> {
    match EnclosureConfig::load(dir) {
        Ok(config) => Ok((config, false)),
        Err(ModelError::NotInitialized { .. }) => Ok((EnclosureConfig::default(), true)),
        Err(err) => Err(err.into()),
    }
}
