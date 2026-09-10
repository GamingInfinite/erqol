//! Area-name reroutes (silly road-sign swaps): swap a vanilla area name for
//! one or more random candidates. Add more area names as more `reroute` calls.

use crate::config;
use crate::ezstate_menu::reroute;

fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_boss_names)
}

pub(crate) fn init() {
    reroute("Margit", &["Margaret Thatcher", "Marge Simpson"], enabled);
    reroute("The Fell Omen", &["the Fell Refund"], enabled);
    reroute(
        "Godrick the Grafted",
        &[
            "Godrick the Garfted",
            "William Grafton",
            "Godrick the Minecrafted",
        ],
        enabled,
    );
}
