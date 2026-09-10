//! Silly string replacements.
//!
//! Submodules register string swaps with the generic registry in `ezstate_menu`
//! (via [`reroute`]), gated by the per-feature toggles declared in
//! `config::TOGGLES` under `Category::Silly` (silly_you_died, silly_area_names,
//! silly_boss_names). The grace-menu "Silly" section and its toggle rows are
//! driven by that registry in `grace_settings`, so this module only wires up
//! the replacements themselves.

pub(crate) mod area_names;
pub(crate) mod boss_names;
pub(crate) mod you_died;

/// Registers the per-feature replacements. Called from the MENU_INSTALLER
/// closure before `ezstate_menu::install()`.
pub(crate) fn init() {
    you_died::init();
    area_names::init();
    boss_names::init();
}