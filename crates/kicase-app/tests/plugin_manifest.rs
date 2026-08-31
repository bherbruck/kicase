//! The KiCad plugin manifest must stay in step with the binary.
//!
//! KiCad validates `plugin.json` against its own `api.v1` schema and refuses to
//! load a plugin that does not match, so these checks catch a broken manifest
//! here rather than as a silent no-show in the Plugins menu.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Both manifests: the one KiCad loads on Linux and macOS, and the Windows
/// one, which differs only in needing the `.exe` on its entrypoint.
fn manifest_paths() -> [PathBuf; 2] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("plugin");
    [root.join("plugin.json"), root.join("plugin.windows.json")]
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("plugin/plugin.json")
}

fn manifest() -> Value {
    let text = std::fs::read_to_string(manifest_path()).expect("plugin.json exists");
    serde_json::from_str(&text).expect("plugin.json is valid JSON")
}

#[test]
fn manifest_has_the_fields_kicad_requires() {
    let manifest = manifest();
    for key in ["identifier", "name", "description", "runtime", "actions"] {
        assert!(manifest.get(key).is_some(), "plugin.json is missing `{key}`");
    }
    assert_eq!(manifest["runtime"]["type"], "exec", "KiCase is an executable plugin");

    let identifier = manifest["identifier"].as_str().expect("identifier is a string");
    assert!(
        identifier.len() <= 100
            && identifier.starts_with(|c: char| c.is_ascii_alphabetic())
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'),
        "identifier {identifier} does not match KiCad's pattern"
    );
}

#[test]
fn every_action_launches_a_real_subcommand() {
    let manifest = manifest();
    let actions = manifest["actions"].as_array().expect("actions is an array");
    assert!(!actions.is_empty());

    // The subcommands the binary actually accepts.
    let known = ["designer", "rebuild", "export", "init", "validate", "list"];

    for action in actions {
        for key in ["identifier", "name", "description", "entrypoint"] {
            assert!(action.get(key).is_some(), "an action is missing `{key}`");
        }
        assert_eq!(action["entrypoint"], "kicase", "actions must launch the kicase binary");

        let args = action["args"].as_array().expect("each action passes a subcommand");
        let subcommand = args[0].as_str().expect("subcommand is a string");
        assert!(known.contains(&subcommand), "unknown subcommand `{subcommand}`");

        for scope in action["scopes"].as_array().expect("scopes") {
            assert_eq!(scope, "pcb", "KiCase only acts on boards");
        }
    }

    // The two actions the specification calls for are both present.
    let identifiers: Vec<&str> = actions.iter().filter_map(|a| a["identifier"].as_str()).collect();
    assert!(identifiers.contains(&"designer"));
    assert!(identifiers.contains(&"rebuild"));
}

#[test]
fn every_referenced_icon_exists_and_is_a_png() {
    let manifest = manifest();
    let root = manifest_path().parent().expect("plugin dir").to_path_buf();

    for action in manifest["actions"].as_array().expect("actions") {
        for key in ["icons-light", "icons-dark"] {
            for icon in action[key].as_array().expect(key) {
                let path = root.join(icon.as_str().expect("icon path is a string"));
                let bytes = std::fs::read(&path)
                    .unwrap_or_else(|_| panic!("missing icon {}", path.display()));
                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{} is not a PNG", path.display());
            }
        }
    }
}

/// The reference designator the preview footprint uses is duplicated between
/// the export crate and the KiCad adapter to keep them decoupled; they must
/// agree.
#[test]
fn preview_reference_is_consistent() {
    let footprint = kicase_export::preview_footprint_sexpr(
        &kicase_export::ExportPaths::preview_model_references(),
        kicase_geometry::units::mm(1.6),
    );
    assert!(footprint.contains(kicase_export::PREVIEW_REFERENCE));
    assert!(footprint.contains("ENCLOSURE_PREVIEW"));
    // Both parts are attached separately so either can be hidden in KiCad.
    assert_eq!(footprint.matches("(model ").count(), 2);
}

/// The Windows manifest must stay identical to the other one apart from the
/// executable name, or the two drift and only one platform gets a fix.
#[test]
fn the_windows_manifest_matches_apart_from_the_exe_suffix() {
    let [unix, windows] = manifest_paths();
    let mut unix: Value =
        serde_json::from_str(&std::fs::read_to_string(unix).expect("plugin.json")).unwrap();
    let windows: Value =
        serde_json::from_str(&std::fs::read_to_string(windows).expect("plugin.windows.json"))
            .unwrap();

    for action in windows["actions"].as_array().expect("actions") {
        assert_eq!(
            action["entrypoint"], "kicase.exe",
            "Windows needs the extension or KiCad cannot launch the plugin"
        );
    }

    // Rewrite the entrypoints and the two should be indistinguishable.
    for action in unix["actions"].as_array_mut().expect("actions") {
        action["entrypoint"] = Value::String("kicase.exe".to_string());
    }
    assert_eq!(unix, windows, "the two manifests have drifted apart");
}
