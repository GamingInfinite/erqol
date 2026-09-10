//! Area-name reroutes (silly road-sign swaps): swap a vanilla area name for
//! one or more random candidates. Add more area names as more `reroute` calls.

use crate::config;
use crate::ezstate_menu::reroute;

fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_boss_names)
}

pub(crate) fn init() {
    reroute("Margit", &["Margaret Thatcher", "Marge Simpson"], enabled);
    reroute(
        "The Omen King",
        &["The Fell Omen", "The Omen King"],
        enabled,
    );
    reroute(
        "The Fell Omen",
        &["the Fell Refund", "the Fell Down the Stairs"],
        enabled,
    );
    reroute(
        "Godrick the Grafted",
        &["Godrick the Grafted", "Godrick Grafton"],
        enabled,
    );
    reroute("Godrick", &["William", "Godrick"], enabled);
    reroute("Grafted", &["Garfted", "Minecrafted"], enabled);
    reroute(
        "Starscourge Radahn",
        &[
            "Radahn, Scourge of the Stars",
            "Radahn, Slayer of Stars",
            "Radahn, The Scourge of Reddit",
            "Radahn, Captain of the Hospice",
            "Raidboss Radahn",
        ],
        enabled,
    );
    reroute("Radahn", &["Ramadan", "Radahn"], enabled);
    reroute(
        "Maliketh, the Black Blade",
        &["My Furry OC (Do NOT Steal)"],
        enabled,
    );
    reroute("Red Wolf of Radagon", &["What the Dog Doing"], enabled);
    reroute("Rennala", &["Ben Shapiro"], enabled);
    reroute("Godfrey", &["God Freed", "Jake Paul"], enabled);
    reroute("First Elden Lord", &["Bastard of the Badlands"], enabled);
    reroute("Grafted Scion", &["Spider-man"], enabled);
    reroute("Ancestor Spirit", &["Bambi", "Canadian"], enabled);
    reroute("Erdtree Avatar", &["Sapient Tree"], enabled);
    reroute("Alexander, Warrior Jar", &["Jar Jar Binks"], enabled);
}
