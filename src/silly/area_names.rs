//! Area-name reroutes (silly road-sign swaps): swap a vanilla area name for
//! one or more random candidates. Add more area names as more `reroute` calls.

use crate::config;
use crate::ezstate_menu::reroute;

fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_area_names)
}

pub(crate) fn init() {
    reroute(
        "Liurnia of the Lakes",
        &["Ligma of the Lakes", "Eastern Europe"],
        enabled,
    );
    reroute("Caelid", &["Gary, Indiana", "Mexico", "Detroit"], enabled);
    reroute("Aeonia", &["Florida"], enabled);
}
