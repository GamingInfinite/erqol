//! The "You Died" → replaced-with-a-random-candidate string swap.

use crate::config;
use crate::ezstate_menu::reroute;

/// Candidate texts the death screen is randomly replaced with. Edit freely.
const CANDIDATES: &[&str] = &[
    "SKILL ISSUE",
    "NO BITCHES?",
    "AMONG US",
    "OW OOF HOT",
    "SMURF'D",
    "SPECTACLE WRIGGLED",
    "YOU DIED",
    "GUARDS ALERTED",
    "COPE HARDER",
    "TOUCH GRASS",
    "YOU GARFED",
    "OW OOF COLD",
    "I DID NOT SURVIVE",
    "WOW",
    "DAMAGE MANAGED",
    "SKILL CATASTROPHE",
    "TIM BURTON LAND",
    "A MIMIR",
    "PENIS DESTROYED",
    "FUCKING OBAMA NATION",
    "IT'S SPREADING",
    "GET MIYAZAKI'D",
    "WELCOME TO YOUKOSO JAPARI PARK",
    "FORESKIN EVISCERATED",
    "JOHN F. KENNEDY",
    "AMERICAN TRAFFIC",
];

/// Live-only gate from the grace menu (no section-wide "silly" toggle).
fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_you_died)
}

pub(crate) fn init() {
    reroute("YOU DIED", CANDIDATES, enabled);
    reroute(
        "GREAT ENEMY FELLED",
        &["GREAT ENEMY FELLED", "BALLS CRUNCHED"],
        enabled,
    );
}
