use std::sync::LazyLock;

use crate::config::{self, Category};
use crate::ezstate_menu::{
    SubMenu, SubMenuAction, alloc_message_id, register_message, register_patcher, splice_option,
    update_message, StateGroup,
};
use crate::log::log;

// ---- Message IDs ----
//
// Every menu row shown here gets its message id from the shared allocator in
// `ezstate_menu` so it can never collide with the ids another module uses.

/// Generic "Cancel" list row; deliberately a shared, module-independent id.
const MSG_CANCEL: i32 = 69_990_003;

static MSG_SETTINGS: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CAT_QOL: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CAT_TWEAKS: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CAT_SILLY: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_HP_BAR_TRACKS: LazyLock<i32> = LazyLock::new(alloc_message_id);

const OPTION_INDEX: i32 = 72;

// ---- Menu labels ----

/// The HP bar readout is the one settings knob that isn't a bool toggle, so it
/// keeps a small hand-written cycle row.
fn hp_bar_label() -> String {
    let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    match cfg.hp_bar_tracks {
        config::HpBarTracks::Hp => "HP Bar Tracks: HP".to_string(),
        config::HpBarTracks::Posture => "HP Bar Tracks: Posture".to_string(),
    }
}

/// Registers the category rows and one row per menu-visible config toggle. All
/// bool feature rows are driven by `config::TOGGLES`, so nothing else needs
/// touching when a feature is added.
fn register_feature_messages() {
    register_message(*MSG_CAT_QOL, "QoL");
    register_message(*MSG_CAT_TWEAKS, "Tweaks");
    register_message(*MSG_CAT_SILLY, "Silly");
    for toggle in config::TOGGLES {
        if toggle.category.is_some() {
            register_message(config::menu_msg_id(toggle.key), &config::menu_label(toggle.key));
        }
    }
    register_message(*MSG_HP_BAR_TRACKS, &hp_bar_label());
}

// ---- Toggle actions ----

unsafe extern "C" fn cycle_hp_bar_tracks() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.hp_bar_tracks = match cfg.hp_bar_tracks {
        config::HpBarTracks::Hp => config::HpBarTracks::Posture,
        config::HpBarTracks::Posture => config::HpBarTracks::Hp,
    };
    drop(cfg);
    config::save();
    update_message(*MSG_HP_BAR_TRACKS, &hp_bar_label());
    log("grace_settings: hp_bar_tracks cycled; live");
}

// ---- Feature wiring ----

pub(crate) fn init() {
    register_message(*MSG_SETTINGS, "ERQoL Settings");
    register_message(MSG_CANCEL, "Cancel");
    register_feature_messages();

    register_patcher(patch);
}

/// Builds the row list (feature toggles + a Cancel/back row) for one category
/// submenu. Row indexes are 1-based sequential; Cancel is last and `is_default`.
/// Bool feature rows come straight from `config::TOGGLES`.
fn rows_for_category(category: Category) -> Vec<(i32, i32, bool, Option<SubMenuAction>)> {
    let mut rows: Vec<(i32, i32, bool, Option<SubMenuAction>)> = Vec::new();
    for toggle in config::TOGGLES.iter().filter(|t| t.category == Some(category)) {
        rows.push((
            rows.len() as i32 + 1,
            config::menu_msg_id(toggle.key),
            false,
            toggle.action,
        ));
    }
    if category == Category::Qol {
        // Non-bool knob: the HP bar readout cycle row.
        rows.push((
            rows.len() as i32 + 1,
            *MSG_HP_BAR_TRACKS,
            false,
            Some(cycle_hp_bar_tracks),
        ));
    }
    rows.push((99, MSG_CANCEL, true, None));
    rows
}

pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    unsafe {
        let initial_state = (*state_group).initial_state;
        if initial_state.is_null() {
            return false;
        }

        // Top-level settings list: one row per category (no action) + Cancel.
        // Its Cancel closes the settings menu back to the grace menu.
        let settings_rows: Vec<(i32, i32, bool, Option<SubMenuAction>)> = vec![
            (1, *MSG_CAT_QOL, false, None),
            (2, *MSG_CAT_TWEAKS, false, None),
            (3, *MSG_CAT_SILLY, false, None),
            (99, MSG_CANCEL, true, None),
        ];
        let (settings_menu, settings_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&settings_rows, initial_state);

        // Each category submenu returns to the settings list on Cancel/back.
        let qol_rows = rows_for_category(Category::Qol);
        let tweaks_rows = rows_for_category(Category::Tweaks);
        let silly_rows = rows_for_category(Category::Silly);
        let (_qol_menu, qol_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&qol_rows, settings_state);
        let (_tweaks_menu, tweaks_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&tweaks_rows, settings_state);
        let (_silly_menu, silly_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&silly_rows, settings_state);

        // Open the category submenu when its row is selected.
        SubMenu::set_option_target(settings_menu, 0, qol_state);
        SubMenu::set_option_target(settings_menu, 1, tweaks_state);
        SubMenu::set_option_target(settings_menu, 2, silly_state);

        splice_option(state_group, OPTION_INDEX, *MSG_SETTINGS, settings_state)
    }
}