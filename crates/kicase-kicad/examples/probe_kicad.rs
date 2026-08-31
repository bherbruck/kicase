//! A minimal live probe against a running KiCad.
//!
//! Used to discover, rather than guess, what the IPC API actually accepts:
//! section 41 of the specification says to test uncertain behaviour against a
//! real KiCad and write down what was found.
//!
//! Run it with a board open in KiCad and the IPC API enabled:
//!
//! ```sh
//! cargo run -p kicase-kicad --example probe_kicad
//! ```

use kicad_ipc_rs::{KiCadClientBlocking, PcbItem, PcbObjectTypeCode};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = KiCadClientBlocking::connect()?;
    println!("connected to KiCad {}", client.get_version()?.full_version);
    println!("project: {}", client.get_current_project_path()?.display());

    let footprints =
        client.get_items_by_type_codes(vec![PcbObjectTypeCode::new_footprint().code])?;
    println!("{} footprint(s) on the board", footprints.len());

    let first = footprints.iter().find_map(|item| match item {
        PcbItem::Footprint(footprint) => footprint.id.clone(),
        _ => None,
    });

    if let Some(id) = first {
        client.clear_selection()?;
        client.add_to_selection(vec![id])?;
        let dump = client.get_selection_as_string()?;
        println!("--- how KiCad serialises a selected footprint ---");
        println!("{dump:#?}");
        client.clear_selection()?;
    }

    Ok(())
}
