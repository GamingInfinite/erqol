//! Item-name reroutes (silly shop-owner renames): swap a vanilla item name for
//! one or more random candidates. Add more item names as more `reroute` calls.

use crate::config;
use crate::ezstate_menu::reroute;

fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_item_names)
}

pub(crate) fn init() {
    reroute("Unalloyed Gold", &["Ketamine"], enabled);
    reroute(
        "Meteoric Ore Blade",
        &["London Bin Knife", "Pumped up Kicks"],
        enabled,
    );
}
