//! The live KiCad IPC session.
//!
//! KiCad 10 requires a running GUI for IPC, so everything in this module needs
//! a KiCad window open with the API enabled. The geometry half of KiCase does
//! not: it works from the board document, which is why the two are separated.

use crate::board::{read_board, BoardReading, LayerRoles};
use crate::layers::{plan_layers, user_layer_name, LayerPlan};
use kicad_ipc_rs::{CommitAction, EditorFrameType, KiCadClientBlocking, KiCadError};
use kicase_model::config::{LayerMapping, LAYER_DISPLAY_NAMES};
use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, KiCadAdapterError>;

#[derive(Debug, Error)]
pub enum KiCadAdapterError {
    #[error("could not reach KiCad: {0}\nIs KiCad 10 running with Preferences > Plugins > Enable IPC API turned on?")]
    Connect(#[source] KiCadError),

    #[error("KiCad reported an error: {0}")]
    Api(#[from] KiCadError),

    #[error("no board is open in KiCad; open a PCB first")]
    NoBoard,

    #[error("KiCad did not report a project path; save the project first")]
    NoProject,

    #[error("the board document could not be read: {0}")]
    Read(#[from] crate::board::ReadError),

    #[error("no free KiCad user layer is available for the enclosure")]
    NoFreeLayers,
}

/// A connected KiCad session.
pub struct KiCadSession {
    client: KiCadClientBlocking,
    version: String,
}

/// What happened when the preview footprint was ensured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewOutcome {
    /// The footprint was already on the board and was left alone.
    Preserved,
    /// A new footprint was created.
    Created,
}

impl KiCadSession {
    /// Connects to a running KiCad and checks that a board is open.
    pub fn connect() -> Result<Self> {
        let client = KiCadClientBlocking::connect().map_err(KiCadAdapterError::Connect)?;
        let version = client.get_version().map_err(KiCadAdapterError::Connect)?.full_version;
        if !client.has_open_board()? {
            return Err(KiCadAdapterError::NoBoard);
        }
        Ok(KiCadSession { client, version })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    /// Directory of the open KiCad project; generated files live under it.
    pub fn project_dir(&self) -> Result<PathBuf> {
        let path = self.client.get_current_project_path()?;
        if path.as_os_str().is_empty() {
            return Err(KiCadAdapterError::NoProject);
        }
        // KiCad may report either the project file or its directory.
        if path.is_dir() {
            Ok(path)
        } else {
            path.parent().map(|p| p.to_path_buf()).ok_or(KiCadAdapterError::NoProject)
        }
    }

    /// The board document as KiCad serialises it. Used for change detection.
    pub fn board_text(&self) -> Result<String> {
        Ok(self.client.get_board_as_string()?)
    }

    /// Reads the whole board and converts it to neutral geometry.
    pub fn read_board(&self, mapping: &LayerMapping) -> Result<BoardReading> {
        let text = self.client.get_board_as_string()?;
        Ok(read_board(&text, &LayerRoles::from_mapping(mapping))?)
    }

    /// Ids of the layers KiCad currently has enabled.
    pub fn enabled_layer_ids(&self) -> Result<Vec<i32>> {
        Ok(self
            .client
            .get_board_enabled_layers()?
            .layers
            .into_iter()
            .map(|layer| layer.id)
            .collect())
    }

    /// Chooses and claims four user layers for the enclosure.
    ///
    /// Enabling layers is a board modification, so it runs inside a commit.
    /// Renaming them is attempted through the board stackup, which is the only
    /// documented route in the KiCad 10 API; when a user layer is not part of
    /// the stackup the rename is reported as unavailable rather than faked.
    pub fn claim_layers(
        &self,
        reading: &BoardReading,
        existing: Option<&LayerMapping>,
    ) -> Result<(LayerPlan, Vec<String>)> {
        let enabled = self.enabled_layer_ids()?;
        let plan =
            plan_layers(reading, existing, &enabled).ok_or(KiCadAdapterError::NoFreeLayers)?;
        let mut notes = Vec::new();

        if !plan.enable.is_empty() {
            // Keep the board's copper count exactly as it is: this call sets
            // the whole enabled-layer set, so the count has to be passed back
            // unchanged or KiCad would restructure the stackup.
            let copper = self.client.get_board_enabled_layers()?.copper_layer_count;
            let mut ids = enabled.clone();
            ids.extend(plan.enable.iter().copied());
            ids.sort_unstable();
            ids.dedup();

            let commit = self.client.begin_commit()?;
            let result = self.client.set_board_enabled_layers(copper, ids);
            match result {
                Ok(_) => {
                    self.client.end_commit(
                        commit,
                        CommitAction::Commit,
                        "KiCase: enable enclosure layers".to_string(),
                    )?;
                },
                Err(err) => {
                    let _ = self.client.end_commit(
                        commit,
                        CommitAction::Drop,
                        "KiCase: enable enclosure layers (failed)".to_string(),
                    );
                    return Err(err.into());
                },
            }
        }

        if !plan.rename.is_empty() {
            match self.rename_layers(&plan) {
                Ok(renamed) if renamed => {},
                Ok(_) | Err(_) => notes.push(format!(
                    "KiCad 10 does not expose layer renaming over IPC for these layers. \
                     Rename them by hand in Board Setup > Board Editor Layers: {}.",
                    plan.rename
                        .iter()
                        .map(|(id, name)| format!(
                            "{} -> {name}",
                            user_layer_name(*id).unwrap_or_else(|| id.to_string())
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }

        Ok((plan, notes))
    }

    /// Best-effort layer renaming through the board stackup.
    ///
    /// Returns `false` when the layers are simply not present in the stackup,
    /// which is the normal case for user layers on many boards.
    fn rename_layers(&self, plan: &LayerPlan) -> Result<bool> {
        let mut stackup = self.client.get_board_stackup()?;
        let mut changed = false;
        for (id, display) in &plan.rename {
            if let Some(layer) = stackup.layers.iter_mut().find(|l| l.layer.id == *id) {
                layer.user_name = display.clone();
                changed = true;
            }
        }
        if !changed {
            return Ok(false);
        }
        let commit = self.client.begin_commit()?;
        match self.client.update_board_stackup(stackup) {
            Ok(_) => {
                self.client.end_commit(
                    commit,
                    CommitAction::Commit,
                    "KiCase: name enclosure layers".to_string(),
                )?;
                Ok(true)
            },
            Err(err) => {
                let _ = self.client.end_commit(
                    commit,
                    CommitAction::Drop,
                    "KiCase: name enclosure layers (failed)".to_string(),
                );
                Err(err.into())
            },
        }
    }

    /// Creates the enclosure preview footprint if the board does not have one.
    ///
    /// An existing preview footprint is always preserved: it may have been
    /// moved, and recreating it would lose that.
    pub fn ensure_preview_footprint(&self, footprint_sexpr: &str) -> Result<PreviewOutcome> {
        if self.find_preview_footprint()?.is_some() {
            return Ok(PreviewOutcome::Preserved);
        }

        let commit = self.client.begin_commit()?;
        match self.client.parse_and_create_items_from_string(footprint_sexpr.to_string()) {
            Ok(_) => {
                self.client.end_commit(
                    commit,
                    CommitAction::Commit,
                    "KiCase: add enclosure preview footprint".to_string(),
                )?;
                Ok(PreviewOutcome::Created)
            },
            Err(err) => {
                let _ = self.client.end_commit(
                    commit,
                    CommitAction::Drop,
                    "KiCase: add enclosure preview footprint (failed)".to_string(),
                );
                Err(err.into())
            },
        }
    }

    /// Finds the preview footprint's item id, if it exists.
    pub fn find_preview_footprint(&self) -> Result<Option<String>> {
        let code = kicad_ipc_rs::PcbObjectTypeCode::new_footprint().code;
        let items = self.client.get_items_by_type_codes(vec![code])?;
        Ok(items.into_iter().find_map(|item| match item {
            kicad_ipc_rs::PcbItem::Footprint(footprint)
                if footprint.reference.as_deref() == Some(kicase_export_preview_reference()) =>
            {
                footprint.id
            },
            _ => None,
        }))
    }

    /// Asks KiCad to redraw.
    ///
    /// This refreshes the PCB editor. It does *not* guarantee that an already
    /// open 3D viewer reloads the STEP file; the user may need to close and
    /// reopen it. That limitation is KiCad's, and it is reported rather than
    /// worked around.
    pub fn refresh_editor(&self) -> Result<()> {
        self.client.refresh_editor(EditorFrameType::PcbEditor)?;
        Ok(())
    }

    /// Selects objects in the PCB editor so the user can see what a diagnostic
    /// is talking about.
    pub fn select(&self, uuids: Vec<String>) -> Result<()> {
        if uuids.is_empty() {
            return Ok(());
        }
        self.client.clear_selection()?;
        self.client.add_to_selection(uuids)?;
        Ok(())
    }

    /// Display names KiCase asks for, in mapping order.
    pub fn preferred_layer_names() -> [&'static str; 6] {
        LAYER_DISPLAY_NAMES
    }
}

/// The reference designator of the preview footprint.
///
/// Duplicated as a constant here to avoid a dependency cycle with the export
/// crate; the two are checked against each other in the app crate's tests.
fn kicase_export_preview_reference() -> &'static str {
    "ENCLOSURE_PREVIEW"
}
