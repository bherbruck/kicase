//! Naming the enclosure layers on the board itself.
//!
//! KiCad's IPC API cannot rename a layer: `board_commands.proto` has
//! `GetBoardLayerName` and no setter, and nothing that creates a layer either
//! (the set is fixed at `User.1`..`User.45` by the board format). KiCad's own
//! Python module can — `BOARD.SetLayerName` — so that is the route, driven
//! through the interpreter KiCad ships rather than whatever `python3` happens
//! to be on `PATH`.
//!
//! This writes the board file directly, which is safe only when KiCad is not
//! holding it open. Callers are expected to say so first.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum RenameError {
    #[error(
        "could not find the Python that KiCad ships, which is the only thing that can \
         name a layer. Set KICASE_PYTHON to it if it lives somewhere unusual."
    )]
    NoPython,
    #[error("naming the layers failed: {0}")]
    Failed(String),
    #[error("could not run {0}: {1}")]
    Io(String, #[source] std::io::Error),
}

/// The script is tiny and fixed, so it travels as a string rather than a file.
///
/// Layers are addressed by canonical name because the board file and the IPC
/// API number them differently — `User.1` is 39 to the board and 53 to the
/// API — and `GetLayerID` is the only thing that knows which is which here.
const SCRIPT: &str = r#"
import sys, pcbnew
path, pairs = sys.argv[1], sys.argv[2:]
board = pcbnew.LoadBoard(path)
for pair in pairs:
    canonical, wanted = pair.split("=", 1)
    layer = board.GetLayerID(canonical)
    if layer < 0:
        raise SystemExit("no layer called " + canonical + " on this board")
    board.SetLayerName(layer, wanted)
pcbnew.SaveBoard(path, board)
"#;

/// Names each `(canonical, wanted)` layer on the board at `path`.
pub fn name_layers(path: &Path, layers: &[(String, String)]) -> Result<(), RenameError> {
    if layers.is_empty() {
        return Ok(());
    }
    let python = kicad_python().ok_or(RenameError::NoPython)?;
    let mut command = Command::new(&python);
    command.arg("-c").arg(SCRIPT).arg(path);
    for (canonical, wanted) in layers {
        command.arg(format!("{canonical}={wanted}"));
    }
    let output = command.output().map_err(|e| RenameError::Io(python.display().to_string(), e))?;
    if output.status.success() {
        return Ok(());
    }
    // KiCad's Python prints an assertion warning on import that says nothing
    // about the outcome, so the last real line is the useful one.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .rfind(|line: &&str| !line.contains("property.h") && !line.contains("assert"))
        .unwrap_or("no reason given")
        .to_string();
    Err(RenameError::Failed(reason))
}

/// The interpreter that can `import pcbnew`.
///
/// KiCad ships its own on Windows and macOS. On Linux the distribution package
/// installs into the system interpreter instead, which is why the plain name is
/// a candidate — but only ever the system one, never whatever is first on
/// `PATH`, since a user's own Python will not have the module.
fn kicad_python() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(set) = std::env::var("KICASE_PYTHON") {
        candidates.push(PathBuf::from(set));
    }
    candidates.extend(
        [
            "/usr/bin/python3",
            "/Applications/KiCad/KiCad.app/Contents/Frameworks/Python.framework/Versions/Current/bin/python3",
            r"C:\Program Files\KiCad\10.0\bin\python.exe",
            r"C:\Program Files\KiCad\9.0\bin\python.exe",
        ]
        .iter()
        .map(PathBuf::from),
    );
    candidates.into_iter().find(|python| imports_pcbnew(python))
}

fn imports_pcbnew(python: &Path) -> bool {
    Command::new(python)
        .arg("-c")
        .arg("import pcbnew")
        .output()
        .is_ok_and(|out| out.status.success())
}
