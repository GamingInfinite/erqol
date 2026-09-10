//! Silly string replacements.
//!
//! Modules use the generic registry in `ezstate_menu`
//! ([`register_string_replacement_if`]) to say "look for this string, swap it
//! with this". This module owns the grace-menu "Silly" config section and the
//! per-feature toggles that gate which replacements are active.

pub(crate) mod area_names;
pub(crate) mod boss_names;
pub(crate) mod you_died;

use std::sync::LazyLock;

use crate::config;
use crate::ezstate_menu::{SubMenuAction, alloc_message_id, register_message, update_message};
use crate::log::log;

// ---- Message IDs ----

static MSG_YOU_DIED: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_AREA_NAMES: LazyLock<i32> = LazyLock::new(alloc_message_id);

// ---- Labels ----

fn you_died_label(enabled: bool) -> String {
    let state = if enabled { "[ON]" } else { "[OFF]" };
    format!("{state} You Died Replacement")
}

fn area_names_label(enabled: bool) -> String {
    let state = if enabled { "[ON]" } else { "[OFF]" };
    format!("{state} Area Name Replacement")
}

fn you_died_active() -> bool {
    config::with_feature(|cfg| cfg.silly_you_died)
}

fn area_names_active() -> bool {
    config::with_feature(|cfg| cfg.silly_area_names)
}

// ---- Menu rows ----

/// Rows for the "Silly" category submenu (grace_settings splices this into the
/// ERQoL Settings list). Row indexes are 1-based; grace_settings appends its
/// own Cancel row.
pub(crate) fn rows() -> Vec<(i32, i32, bool, Option<SubMenuAction>)> {
    vec![
        (1, *MSG_YOU_DIED, false, Some(toggle_you_died)),
        (2, *MSG_AREA_NAMES, false, Some(toggle_area_names)),
    ]
}

// ---- Toggle actions ----

unsafe extern "C" fn toggle_you_died() {
    {
        let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        cfg.silly_you_died = !cfg.silly_you_died;
    }
    config::save();
    update_message(*MSG_YOU_DIED, &you_died_label(you_died_active()));
    log("silly: you_died toggled; active now");
}

unsafe extern "C" fn toggle_area_names() {
    {
        let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        cfg.silly_area_names = !cfg.silly_area_names;
    }
    config::save();
    update_message(*MSG_AREA_NAMES, &area_names_label(area_names_active()));
    log("silly: area_names toggled; active now");
}

// ---- Installer ----

/// Registers the Silly menu texts and the per-feature replacements. Called
/// from the MENU_INSTALLER closure before `ezstate_menu::install()`.
pub(crate) fn init() {
    register_message(*MSG_YOU_DIED, &you_died_label(you_died_active()));
    register_message(*MSG_AREA_NAMES, &area_names_label(area_names_active()));
    you_died::init();
    area_names::init();
    boss_names::init();
}
