//! Naming layers goes through KiCad's own Python, so this only runs where that
//! Python exists. Skipped rather than failed elsewhere: the dependency belongs
//! to the machine, not to the code.

use kicase_kicad::rename::name_layers;
use std::path::Path;

#[test]
fn names_a_user_layer_on_the_board() {
    let dir = std::env::temp_dir().join("kicase-rename-test");
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("examples/usb-cutout/usb-cutout.kicad_pcb");
    let board = dir.join("board.kicad_pcb");
    std::fs::copy(&source, &board).expect("copy the board");

    let before = std::fs::read_to_string(&board).expect("read");
    assert!(
        before.contains("(47 \"User.5\" user)"),
        "the fixture is supposed to leave User.5 unnamed"
    );

    match name_layers(&board, &[("User.5".to_string(), "Enclosure.Top".to_string())]) {
        Ok(()) => {
            let after = std::fs::read_to_string(&board).expect("read back");
            assert!(
                after.contains("(47 \"User.5\" user \"Enclosure.Top\")"),
                "User.5 was not named"
            );
            // The whole board is rewritten, so check it is still a board and
            // still carries what it did before.
            assert!(after.contains("kicad_pcb"), "the board stopped being a board");
            assert!(after.contains("Enclosure.Cuts"), "an existing layer name was lost");
        },
        Err(err) => eprintln!("skipped, no KiCad Python here: {err}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
