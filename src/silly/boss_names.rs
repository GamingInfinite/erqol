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
    reroute(
        "Starscourge Radahn",
        &["Radahn, The Scourge of Reddit"],
        enabled,
    );
    reroute(
        "Maliketh, the Black Blade",
        &["My Furry OC (Do NOT Steal)"],
        enabled,
    );
    reroute("Red Wolf of Radagon", &["What the Dog Doing"], enabled);
    reroute("Godfrey", &["God Freed", "Jake Paul"], enabled);
    reroute("First Elden Lord", &["Bastard of the Badlands"], enabled);
    reroute("Grafted Scion", &["Spider-man"], enabled);
    reroute("Ancestor Spirit", &["Bambi", "Canadian"], enabled);
    reroute("Erdtree Avatar", &["Sapient Tree"], enabled);
}
