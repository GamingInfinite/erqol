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
    reroute(
        "Raya Lucaria",
        &["Malaysia Lucario", "Hog Warts", "Raging Lucario"],
        enabled,
    );
    reroute(
        "Caelid",
        &["Gary, Indiana", "Mexico", "Brasil", "Detroit"],
        enabled,
    );
    reroute("Aeonia", &["Florida"], enabled);
    reroute("Roundtable Hold", &["The Metaverse"], enabled);
    reroute("Limgrave", &["Derry, NI"], enabled);
    reroute(
        "Siofra River",
        &["Quebec", "Space Mountain", "Montreal"],
        enabled,
    );
    reroute("Weeping Peninsula", &["Upstate New York"], enabled);
    reroute("Castle Morne", &["FurFest 20XX"], enabled);
    reroute("Stormveil", &["Garfielf"], enabled);
    reroute(
        "Village of the Albinaurics",
        &["Elden Ring Albania"],
        enabled,
    );
    reroute("Dragon", &["Deez Nutz"], enabled);
}
