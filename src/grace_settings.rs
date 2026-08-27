use std::sync::LazyLock;

use crate::config;
use crate::ezstate_menu::{
    alloc_message_id, register_message, register_patcher, splice_option, update_message, StateGroup,
    SubMenu, SubMenuAction,
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

static MSG_DUNGEON_WARP: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_MAP_IN_COMBAT: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_AUTO_PICKUP: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_SKIP_FLASK: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_ANTI_FARM: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CONSUME_RUNES: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_MERCHANT_BELL: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_ROUNDTABLE: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_HP_BAR_TRACKS: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_SPIRIT_SUMMON: LazyLock<i32> = LazyLock::new(alloc_message_id);

const OPTION_INDEX: i32 = 72;

// ---- Feature info ----

/// Category a feature belongs to, controlling which settings submenu holds it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    /// Changes that don't affect combat results — they skip quit-outs, prevent
    /// a warp, or speed things up (e.g. dungeon_warp, auto_pickup).
    Qol,
    /// Direct gameplay tweaks that alter how a fight can be played out
    /// (e.g. spirit summoning everywhere).
    Tweaks,
}

struct Feature {
    name: &'static str,
    msg: &'static LazyLock<i32>,
    action: SubMenuAction,
    needs_reload: bool,
    category: Category,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "dungeon_warp",
        msg: &MSG_DUNGEON_WARP,
        action: toggle_dungeon_warp,
        needs_reload: true,
        category: Category::Qol,
    },
    Feature {
        name: "map_in_combat",
        msg: &MSG_MAP_IN_COMBAT,
        action: toggle_map_in_combat,
        needs_reload: true,
        category: Category::Qol,
    },
    Feature {
        name: "auto_pickup",
        msg: &MSG_AUTO_PICKUP,
        action: toggle_auto_pickup,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "skip_flask_confirm",
        msg: &MSG_SKIP_FLASK,
        action: toggle_skip_flask,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "anti_farm_shop",
        msg: &MSG_ANTI_FARM,
        action: toggle_anti_farm,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "consume_all_runes",
        msg: &MSG_CONSUME_RUNES,
        action: toggle_consume_runes,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "merchant_bell_bearing",
        msg: &MSG_MERCHANT_BELL,
        action: toggle_merchant_bell,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "roundtable_at_home",
        msg: &MSG_ROUNDTABLE,
        action: toggle_roundtable,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "hp_bar_tracks",
        msg: &MSG_HP_BAR_TRACKS,
        action: cycle_hp_bar_tracks,
        needs_reload: false,
        category: Category::Qol,
    },
    Feature {
        name: "spirit_summon_everywhere",
        msg: &MSG_SPIRIT_SUMMON,
        action: toggle_spirit_summon,
        needs_reload: true,
        category: Category::Tweaks,
    },
];

fn config_value(name: &str) -> bool {
    let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    match name {
        "dungeon_warp" => cfg.dungeon_warp,
        "map_in_combat" => cfg.map_in_combat,
        "auto_pickup" => cfg.auto_pickup,
        "skip_flask_confirm" => cfg.skip_flask_confirm,
        "anti_farm_shop" => cfg.anti_farm_shop,
        "consume_all_runes" => cfg.consume_all_runes,
        "merchant_bell_bearing" => cfg.merchant_bell_bearing,
        "roundtable_at_home" => cfg.roundtable_at_home,
        "hp_bar_tracks" => cfg.hp_bar_tracks == config::HpBarTracks::Posture,
        "spirit_summon_everywhere" => cfg.spirit_summon_everywhere,
        _ => true,
    }
}

fn feature_label(name: &str, enabled: bool, needs_reload: bool) -> String {
    if name == "hp_bar_tracks" {
        let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        let tracks = match cfg.hp_bar_tracks {
            config::HpBarTracks::Hp => "HP",
            config::HpBarTracks::Posture => "Posture",
        };
        return format!("HP Bar Tracks: {tracks}");
    }

    let state = if enabled { "[ON]" } else { "[OFF]" };
    let suffix = if needs_reload {
        " (restart to change)"
    } else {
        ""
    };
    let base = match name {
        "dungeon_warp" => "Dungeon Warp",
        "map_in_combat" => "Map in Combat",
        "auto_pickup" => "Auto Pickup",
        "skip_flask_confirm" => "Skip Flask Confirm",
        "anti_farm_shop" => "Anti-Farm Shop",
        "consume_all_runes" => "Consume All Runes",
        "merchant_bell_bearing" => "Merchant Bell Bearing",
        "roundtable_at_home" => "Roundtable at Home",
        "spirit_summon_everywhere" => "Spirit Summons Everywhere",
        _ => name,
    };
    format!("{state} {base}{suffix}")
}

fn register_feature_messages() {
    register_message(*MSG_CAT_QOL, "QoL");
    register_message(*MSG_CAT_TWEAKS, "Tweaks");
    for feature in FEATURES {
        let enabled = config_value(feature.name);
        register_message(
            **feature.msg,
            &feature_label(feature.name, enabled, feature.needs_reload),
        );
    }
}

// ---- Toggle actions ----

unsafe extern "C" fn toggle_auto_pickup() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.auto_pickup = !cfg.auto_pickup;
    let (name, enabled, needs_reload) = ("auto_pickup", cfg.auto_pickup, false);
    drop(cfg);
    config::save();
    update_message(*MSG_AUTO_PICKUP, &feature_label(name, enabled, needs_reload));
    log("grace_settings: auto_pickup toggled; active now");
}

unsafe extern "C" fn toggle_skip_flask() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.skip_flask_confirm = !cfg.skip_flask_confirm;
    let (name, enabled, needs_reload) = ("skip_flask_confirm", cfg.skip_flask_confirm, false);
    drop(cfg);
    config::save();
    update_message(*MSG_SKIP_FLASK, &feature_label(name, enabled, needs_reload));
    log("grace_settings: skip_flask_confirm toggled; active now");
}

unsafe extern "C" fn toggle_anti_farm() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.anti_farm_shop = !cfg.anti_farm_shop;
    let (name, enabled, needs_reload) = ("anti_farm_shop", cfg.anti_farm_shop, false);
    drop(cfg);
    config::save();
    update_message(*MSG_ANTI_FARM, &feature_label(name, enabled, needs_reload));
    log("grace_settings: anti_farm_shop toggled; active now");
}

unsafe extern "C" fn toggle_consume_runes() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.consume_all_runes = !cfg.consume_all_runes;
    let (name, enabled, needs_reload) = ("consume_all_runes", cfg.consume_all_runes, false);
    drop(cfg);
    config::save();
    update_message(*MSG_CONSUME_RUNES, &feature_label(name, enabled, needs_reload));
    log("grace_settings: consume_all_runes toggled; active now");
}

unsafe extern "C" fn toggle_merchant_bell() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.merchant_bell_bearing = !cfg.merchant_bell_bearing;
    let (name, enabled, needs_reload) = ("merchant_bell_bearing", cfg.merchant_bell_bearing, false);
    drop(cfg);
    config::save();
    update_message(*MSG_MERCHANT_BELL, &feature_label(name, enabled, needs_reload));
    log("grace_settings: merchant_bell_bearing toggled; active now");
}

unsafe extern "C" fn toggle_roundtable() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.roundtable_at_home = !cfg.roundtable_at_home;
    let (name, enabled, needs_reload) = ("roundtable_at_home", cfg.roundtable_at_home, false);
    drop(cfg);
    config::save();
    update_message(*MSG_ROUNDTABLE, &feature_label(name, enabled, needs_reload));
    log("grace_settings: roundtable_at_home toggled; active now");
}

unsafe extern "C" fn cycle_hp_bar_tracks() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.hp_bar_tracks = match cfg.hp_bar_tracks {
        config::HpBarTracks::Hp => config::HpBarTracks::Posture,
        config::HpBarTracks::Posture => config::HpBarTracks::Hp,
    };
    drop(cfg);
    config::save();
    update_message(*MSG_HP_BAR_TRACKS, &feature_label("hp_bar_tracks", false, false));
    log("grace_settings: hp_bar_tracks cycled; live");
}

unsafe extern "C" fn toggle_spirit_summon() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.spirit_summon_everywhere = !cfg.spirit_summon_everywhere;
    let (name, enabled, needs_reload) = (
        "spirit_summon_everywhere",
        cfg.spirit_summon_everywhere,
        true,
    );
    drop(cfg);
    config::save();
    update_message(*MSG_SPIRIT_SUMMON, &feature_label(name, enabled, needs_reload));
    log("grace_settings: spirit_summon_everywhere toggled; restart to fully apply");
}

unsafe extern "C" fn toggle_dungeon_warp() {
    use std::sync::atomic::Ordering;

    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.dungeon_warp = !cfg.dungeon_warp;
    let (name, enabled, needs_reload) = ("dungeon_warp", cfg.dungeon_warp, true);
    let new_val = cfg.dungeon_warp;
    drop(cfg);
    config::save();
    config::DUNGEON_WARP_ENABLED.store(new_val, Ordering::Relaxed);
    update_message(*MSG_DUNGEON_WARP, &feature_label(name, enabled, needs_reload));
    log("grace_settings: dungeon_warp toggled; restart to fully apply");
}

unsafe extern "C" fn toggle_map_in_combat() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.map_in_combat = !cfg.map_in_combat;
    let (name, enabled, needs_reload) = ("map_in_combat", cfg.map_in_combat, true);
    drop(cfg);
    config::save();
    update_message(*MSG_MAP_IN_COMBAT, &feature_label(name, enabled, needs_reload));
    log("grace_settings: map_in_combat toggled; restart to apply");
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
fn rows_for_category(category: Category) -> Vec<(i32, i32, bool, Option<SubMenuAction>)> {
    let mut rows: Vec<(i32, i32, bool, Option<SubMenuAction>)> = Vec::new();
    for feature in FEATURES.iter().filter(|f| f.category == category) {
        rows.push((
            rows.len() as i32 + 1,
            **feature.msg,
            false,
            Some(feature.action),
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
            (99, MSG_CANCEL, true, None),
        ];
        let (settings_menu, settings_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&settings_rows, initial_state);

        // Each category submenu returns to the settings list on Cancel/back.
        let qol_rows = rows_for_category(Category::Qol);
        let tweaks_rows = rows_for_category(Category::Tweaks);
        let (_qol_menu, qol_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&qol_rows, settings_state);
        let (_tweaks_menu, tweaks_state) =
            SubMenu::link_from_rows_self_return_with_ptr(&tweaks_rows, settings_state);

        // Open the category submenu when its row is selected.
        SubMenu::set_option_target(settings_menu, 0, qol_state);
        SubMenu::set_option_target(settings_menu, 1, tweaks_state);

        splice_option(state_group, OPTION_INDEX, *MSG_SETTINGS, settings_state)
    }
}
