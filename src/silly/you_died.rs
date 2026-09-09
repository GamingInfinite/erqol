//! The "You Died" → replaced-with-a-random-candidate string swap.

use crate::config;
use crate::ezstate_menu::register_string_replacement_variants_if;
use crate::log::log;

/// Candidate texts the death screen is randomly replaced with. Edit freely.
const REPLACEMENTS: &[&str] = &[
    "SKILL ISSUE",
    "NO BITCHES?",
    "AMONG US",
    "OW OOF HOT",
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
];

/// Matched case-insensitively and as a substring, so "You Died", "YOU DIED",
/// and any item or dialog text that mentions it all get swapped.
const ORIGINAL: &str = "You Died";

/// Live-only gate from the grace menu (no section-wide "silly" toggle).
fn enabled() -> bool {
    config::with_feature(|cfg| cfg.silly_you_died)
}

pub(crate) fn init() {
    register_string_replacement_variants_if(ORIGINAL, REPLACEMENTS, enabled);
    log(&format!(
        "silly: registered '{ORIGINAL}' -> [{}] (case-insensitive, substring, random)",
        REPLACEMENTS.join(", ")
    ));
}
