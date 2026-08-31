//! Claiming KiCad user layers.
//!
//! Section 10: never assume `User.1` through `User.4` are free. KiCase looks at
//! what the board actually declares and what it draws on, then claims four user
//! layers that are genuinely unused — or re-claims the ones it used last time.
//!
//! The ids here are the **IPC API's** layer ids, which are what
//! `SetBoardEnabledLayers` expects. They are not the ids a board file uses for
//! the same layers, so layers are matched by canonical name and converted to an
//! id only at the point of the API call.

use crate::board::BoardReading;
use kicase_model::config::{LayerMapping, LAYER_DISPLAY_NAMES};

/// KiCad layer id of `User.1`.
pub const FIRST_USER_LAYER_ID: i32 = 53;
/// KiCad layer id of `User.45`, the last one.
pub const LAST_USER_LAYER_ID: i32 = 98;
/// `User.9` is id 61; id 62 is KiCad's internal `Rescue` layer, and `User.10`
/// resumes at 63. The numbering is not contiguous, so it is spelled out here
/// rather than computed with a single offset.
const RESCUE_LAYER_ID: i32 = 62;
/// How many user layers precede the `Rescue` gap.
const USER_LAYERS_BEFORE_GAP: i32 = 9;

/// Canonical name of a user layer id, or `None` if the id is not a user layer.
pub fn user_layer_name(id: i32) -> Option<String> {
    let index = match id {
        FIRST_USER_LAYER_ID..=61 => id - FIRST_USER_LAYER_ID + 1,
        63..=LAST_USER_LAYER_ID => id - FIRST_USER_LAYER_ID,
        _ => return None,
    };
    Some(format!("User.{index}"))
}

/// Layer id for a canonical user layer name such as `User.7`.
pub fn user_layer_id(name: &str) -> Option<i32> {
    let index: i32 = name.strip_prefix("User.")?.parse().ok()?;
    if index < 1 {
        return None;
    }
    let id = if index <= USER_LAYERS_BEFORE_GAP {
        FIRST_USER_LAYER_ID + index - 1
    } else {
        FIRST_USER_LAYER_ID + index
    };
    (id != RESCUE_LAYER_ID && (FIRST_USER_LAYER_ID..=LAST_USER_LAYER_ID).contains(&id))
        .then_some(id)
}

/// The outcome of choosing layers for a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerPlan {
    pub mapping: LayerMapping,
    /// Layers that must be enabled on the board.
    pub enable: Vec<i32>,
    /// Display names KiCase would like KiCad to show, paired with layer ids.
    pub rename: Vec<(i32, String)>,
    /// True when the plan simply confirms what the board already had.
    pub unchanged: bool,
}

/// Chooses four user layers for the enclosure.
///
/// Preference order:
/// 1. layers already displaying KiCase's own names, so a reopened project keeps
///    its layers;
/// 2. layers named in the existing project mapping;
/// 3. any user layer that carries no objects.
pub fn plan_layers(
    reading: &BoardReading,
    existing: Option<&LayerMapping>,
    enabled_ids: &[i32],
) -> Option<LayerPlan> {
    let mut chosen: Vec<String> = Vec::with_capacity(LAYER_DISPLAY_NAMES.len());

    for (index, display) in LAYER_DISPLAY_NAMES.iter().enumerate() {
        // 1. A layer already carrying the display name KiCase wants.
        let by_display = reading
            .layers
            .iter()
            .find(|layer| layer.display.as_deref() == Some(*display))
            .map(|layer| layer.canonical.clone());

        // 2. Whatever the project file already recorded.
        let by_config = existing.map(|mapping| match index {
            0 => mapping.outline.clone(),
            1 => mapping.datums.clone(),
            2 => mapping.cuts.clone(),
            3 => mapping.top.clone(),
            4 => mapping.bottom.clone(),
            _ => mapping.solids.clone(),
        });

        let candidate = by_display
            .or(by_config)
            .filter(|name| user_layer_id(name).is_some() && !chosen.contains(name));
        if let Some(name) = candidate {
            chosen.push(name);
        } else {
            // 3. The first user layer nobody is drawing on.
            let free = (FIRST_USER_LAYER_ID..=LAST_USER_LAYER_ID)
                .filter_map(user_layer_name)
                .find(|name| {
                    !chosen.contains(name) && !reading.used_layers.iter().any(|used| used == name)
                })?;
            chosen.push(free);
        }
    }

    let mapping = LayerMapping {
        outline: chosen[0].clone(),
        datums: chosen[1].clone(),
        cuts: chosen[2].clone(),
        top: chosen[3].clone(),
        bottom: chosen[4].clone(),
        solids: chosen[5].clone(),
    };

    let ids: Vec<i32> = chosen.iter().filter_map(|name| user_layer_id(name)).collect();
    let enable: Vec<i32> = ids.iter().copied().filter(|id| !enabled_ids.contains(id)).collect();

    let rename: Vec<(i32, String)> = ids
        .iter()
        .zip(LAYER_DISPLAY_NAMES.iter())
        .filter(|(id, display)| {
            let canonical = user_layer_name(**id).unwrap_or_default();
            reading
                .layers
                .iter()
                .find(|layer| layer.canonical == canonical)
                .map(|layer| layer.display.as_deref() != Some(**display))
                .unwrap_or(true)
        })
        .map(|(id, display)| (*id, (*display).to_string()))
        .collect();

    let unchanged = enable.is_empty() && rename.is_empty();
    Some(LayerPlan { mapping, enable, rename, unchanged })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{read_board, LayerRoles};

    fn roles() -> LayerRoles {
        LayerRoles {
            outline: "User.1".into(),
            datums: "User.2".into(),
            cuts: "User.3".into(),
            top: "User.4".into(),
            bottom: "User.5".into(),
            solids: "User.6".into(),
        }
    }

    fn board_with(extra: &str) -> String {
        format!(
            r#"(kicad_pcb (version 20241229)
                 (layers (0 "F.Cu" signal) (47 "Edge.Cuts" user)
                         (53 "User.1" user) (54 "User.2" user) (55 "User.3" user)
                         (56 "User.4" user) (57 "User.5" user) (58 "User.6" user)
                         (59 "User.7" user) (60 "User.8" user) (61 "User.9" user)
                         (63 "User.10" user) (64 "User.11" user) (65 "User.12" user))
                 {extra})"#
        )
    }

    #[test]
    fn layer_name_and_id_round_trip() {
        assert_eq!(user_layer_id("User.1"), Some(53));
        assert_eq!(user_layer_name(53).as_deref(), Some("User.1"));
        assert_eq!(user_layer_id("User.45"), Some(98));
        assert_eq!(user_layer_id("User.46"), None);
        // Id 62 is KiCad's Rescue layer, not a user layer.
        assert_eq!(user_layer_name(62), None);
        assert_eq!(user_layer_id("User.9"), Some(61));
        assert_eq!(user_layer_id("User.10"), Some(63));
        assert_eq!(user_layer_name(63).as_deref(), Some("User.10"));
        assert_eq!(user_layer_id("F.Cu"), None);
    }

    #[test]
    fn skips_user_layers_that_already_carry_objects() {
        let board = board_with(
            r#"(gr_line (start 0 0) (end 1 1) (layer "User.1") (uuid "a"))
               (gr_line (start 0 0) (end 1 1) (layer "User.3") (uuid "b"))"#,
        );
        let reading = read_board(&board, &roles()).expect("parses");
        let plan = plan_layers(&reading, None, &[]).expect("a plan exists");
        // User.1 and User.3 are in use, so the enclosure takes 2, 4, 5, 6, 7, 8.
        assert_eq!(plan.mapping.outline, "User.2");
        assert_eq!(plan.mapping.datums, "User.4");
        assert_eq!(plan.mapping.cuts, "User.5");
        assert_eq!(plan.mapping.top, "User.6");
        assert_eq!(plan.mapping.bottom, "User.7");
        assert_eq!(plan.mapping.solids, "User.8");
    }

    #[test]
    fn reuses_layers_that_already_have_the_kicase_display_names() {
        let board = r#"(kicad_pcb (version 20241229)
             (layers (47 "Edge.Cuts" user)
                     (53 "User.1" user)
                     (60 "User.8" user "Enclosure")
                     (61 "User.9" user "Enclosure.Datums")
                     (63 "User.10" user "Enclosure.Cuts")
                     (64 "User.11" user "Enclosure.Top")
                     (65 "User.12" user "Enclosure.Bottom")
                     (66 "User.13" user "Enclosure.Solids")))"#;
        let reading = read_board(board, &roles()).expect("parses");
        let plan = plan_layers(&reading, None, &[60, 61, 63, 64, 65, 66]).expect("a plan exists");
        assert_eq!(plan.mapping.outline, "User.8");
        assert_eq!(plan.mapping.top, "User.11");
        assert_eq!(plan.mapping.solids, "User.13");
        assert!(plan.unchanged, "nothing needs enabling or renaming: {plan:?}");
    }

    #[test]
    fn keeps_the_mapping_recorded_in_the_project_file() {
        let board = board_with("");
        let reading = read_board(&board, &roles()).expect("parses");
        let existing = LayerMapping {
            outline: "User.5".into(),
            datums: "User.6".into(),
            cuts: "User.7".into(),
            top: "User.8".into(),
            bottom: "User.9".into(),
            solids: "User.10".into(),
        };
        let plan = plan_layers(&reading, Some(&existing), &[57, 58, 59, 60, 61, 63])
            .expect("a plan exists");
        assert_eq!(plan.mapping, existing);
        assert!(plan.enable.is_empty());
        // The display names still need setting.
        assert_eq!(plan.rename.len(), 6);
    }

    #[test]
    fn reports_which_layers_need_enabling() {
        let board = board_with("");
        let reading = read_board(&board, &roles()).expect("parses");
        let plan = plan_layers(&reading, None, &[53]).expect("a plan exists");
        assert_eq!(plan.mapping.outline, "User.1");
        assert_eq!(plan.enable, vec![54, 55, 56, 57, 58]);
        assert!(!plan.unchanged);
    }
}
