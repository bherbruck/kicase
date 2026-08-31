//! Probes which KiCad tool actions are reachable over IPC.
//!
//! `RunAction` answers `RAS_INVALID` for a name the target frame does not know,
//! so this discovers real action names instead of guessing at them. Only
//! view-related actions are listed here: running an unknown editing action on
//! someone's open board would be reckless.

use kicad_ipc_rs::{KiCadClientBlocking, RunActionStatus};

const CANDIDATES: &[&str] = &[
    // 3D viewer, various plausible namespaces.
    "3DViewer.Control.reloadBoard",
    "3DViewer.Control.rerender",
    "3DViewer.Control.redraw",
    "3DViewer.Control.refresh",
    "3DViewer.Control.viewReload",
    "3DViewer.Control.zoomFitScreen",
    "3DViewer.Control.toggleOrtho",
    // Opening the viewer from the board editor.
    "pcbnew.Control.show3DViewer",
    "pcbnew.EditorControl.show3DViewer",
    "pcbnew.Control.show3DFrame",
    // Generic redraw paths in the board editor.
    "common.Control.zoomFitScreen",
    "common.Control.refreshView",
    "pcbnew.Control.zoomFitScreen",
];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = KiCadClientBlocking::connect()?;
    println!("KiCad {}", client.get_version()?.full_version);

    for action in CANDIDATES {
        let result = client.run_action(action.to_string());
        let verdict = match result {
            Ok(RunActionStatus::Ok) => "OK".to_string(),
            Ok(RunActionStatus::Invalid) => "invalid (frame does not know it)".to_string(),
            Ok(other) => format!("{other:?}"),
            Err(err) => format!("error: {err}"),
        };
        println!("{action:45} {verdict}");
    }
    Ok(())
}
