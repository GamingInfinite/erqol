use crate::config;
use crate::ezstate_menu::{
    register_message, register_patcher, splice_option, update_message, StateGroup, SubMenu,
    SubMenuAction,
};
use crate::log::log;

// ---- Message IDs ----

const MSG_SETTINGS: i32 = 69_990_010;
const MSG_CANCEL: i32 = 69_990_003;

const MSG_DUNGEON_WARP: i32 = 69_990_020;
const MSG_MAP_IN_COMBAT: i32 = 69_990_021;
const MSG_AUTO_PICKUP: i32 = 69_990_022;
const MSG_SKIP_FLASK: i32 = 69_990_023;
const MSG_ANTI_FARM: i32 = 69_990_024;
const MSG_CONSUME_RUNES: i32 = 69_990_025;
const MSG_MERCHANT_BELL: i32 = 69_990_026;
const MSG_ROUNDTABLE: i32 = 69_990_027;

const OPTION_INDEX: i32 = 72;

// ---- Feature info ----

struct Feature {
    name: &'static str,
    message_id: i32,
    action: SubMenuAction,
    needs_reload: bool,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "dungeon_warp",
        message_id: MSG_DUNGEON_WARP,
        action: toggle_dungeon_warp,
        needs_reload: true,
    },
    Feature {
        name: "map_in_combat",
        message_id: MSG_MAP_IN_COMBAT,
        action: toggle_map_in_combat,
        needs_reload: true,
    },
    Feature {
        name: "auto_pickup",
        message_id: MSG_AUTO_PICKUP,
        action: toggle_auto_pickup,
        needs_reload: false,
    },
    Feature {
        name: "skip_flask_confirm",
        message_id: MSG_SKIP_FLASK,
        action: toggle_skip_flask,
        needs_reload: false,
    },
    Feature {
        name: "anti_farm_shop",
        message_id: MSG_ANTI_FARM,
        action: toggle_anti_farm,
        needs_reload: false,
    },
    Feature {
        name: "consume_all_runes",
        message_id: MSG_CONSUME_RUNES,
        action: toggle_consume_runes,
        needs_reload: false,
    },
    Feature {
        name: "merchant_bell_bearing",
        message_id: MSG_MERCHANT_BELL,
        action: toggle_merchant_bell,
        needs_reload: false,
    },
    Feature {
        name: "roundtable_at_home",
        message_id: MSG_ROUNDTABLE,
        action: toggle_roundtable,
        needs_reload: false,
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
        _ => true,
    }
}

fn feature_label(name: &str, enabled: bool, needs_reload: bool) -> String {
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
        _ => name,
    };
    format!("{state} {base}{suffix}")
}

fn register_feature_messages() {
    for feature in FEATURES {
        let enabled = config_value(feature.name);
        register_message(
            feature.message_id,
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
    update_message(MSG_AUTO_PICKUP, &feature_label(name, enabled, needs_reload));
    log("grace_settings: auto_pickup toggled; active now");
}

unsafe extern "C" fn toggle_skip_flask() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.skip_flask_confirm = !cfg.skip_flask_confirm;
    let (name, enabled, needs_reload) = ("skip_flask_confirm", cfg.skip_flask_confirm, false);
    drop(cfg);
    config::save();
    update_message(MSG_SKIP_FLASK, &feature_label(name, enabled, needs_reload));
    log("grace_settings: skip_flask_confirm toggled; active now");
}

unsafe extern "C" fn toggle_anti_farm() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.anti_farm_shop = !cfg.anti_farm_shop;
    let (name, enabled, needs_reload) = ("anti_farm_shop", cfg.anti_farm_shop, false);
    drop(cfg);
    config::save();
    update_message(MSG_ANTI_FARM, &feature_label(name, enabled, needs_reload));
    log("grace_settings: anti_farm_shop toggled; active now");
}

unsafe extern "C" fn toggle_consume_runes() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.consume_all_runes = !cfg.consume_all_runes;
    let (name, enabled, needs_reload) = ("consume_all_runes", cfg.consume_all_runes, false);
    drop(cfg);
    config::save();
    update_message(MSG_CONSUME_RUNES, &feature_label(name, enabled, needs_reload));
    log("grace_settings: consume_all_runes toggled; active now");
}

unsafe extern "C" fn toggle_merchant_bell() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.merchant_bell_bearing = !cfg.merchant_bell_bearing;
    let (name, enabled, needs_reload) = ("merchant_bell_bearing", cfg.merchant_bell_bearing, false);
    drop(cfg);
    config::save();
    update_message(MSG_MERCHANT_BELL, &feature_label(name, enabled, needs_reload));
    log("grace_settings: merchant_bell_bearing toggled; active now");
}

unsafe extern "C" fn toggle_roundtable() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.roundtable_at_home = !cfg.roundtable_at_home;
    let (name, enabled, needs_reload) = ("roundtable_at_home", cfg.roundtable_at_home, false);
    drop(cfg);
    config::save();
    update_message(MSG_ROUNDTABLE, &feature_label(name, enabled, needs_reload));
    log("grace_settings: roundtable_at_home toggled; active now");
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
    update_message(MSG_DUNGEON_WARP, &feature_label(name, enabled, needs_reload));
    log("grace_settings: dungeon_warp toggled; restart to fully apply");
}

unsafe extern "C" fn toggle_map_in_combat() {
    let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    cfg.map_in_combat = !cfg.map_in_combat;
    let (name, enabled, needs_reload) = ("map_in_combat", cfg.map_in_combat, true);
    drop(cfg);
    config::save();
    update_message(MSG_MAP_IN_COMBAT, &feature_label(name, enabled, needs_reload));
    log("grace_settings: map_in_combat toggled; restart to apply");
}

// ---- Feature wiring ----

pub(crate) fn init() {
    register_message(MSG_SETTINGS, "ERQoL Settings");
    register_message(MSG_CANCEL, "Cancel");
    register_feature_messages();

    register_patcher(patch);
}

pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    unsafe {
        let initial_state = (*state_group).initial_state;
        if initial_state.is_null() {
            return false;
        }

        let mut rows: Vec<(i32, i32, bool, Option<SubMenuAction>)> = Vec::new();

        for feature in FEATURES {
            rows.push((
                rows.len() as i32 + 1,
                feature.message_id,
                false,
                Some(feature.action),
            ));
        }

        rows.push((99, MSG_CANCEL, true, None));

        let submenu_state = SubMenu::link_from_rows_self_return(&rows, initial_state);

        splice_option(state_group, OPTION_INDEX, MSG_SETTINGS, submenu_state)
    }
}
