use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::log::{self, log};

#[derive(Debug, Clone)]
pub struct Config {
    pub dungeon_warp: bool,
    pub map_in_combat: bool,
    pub auto_pickup: bool,
    pub skip_flask_confirm: bool,
    pub anti_farm_shop: bool,
    pub consume_all_runes: bool,
    pub merchant_bell_bearing: bool,
    pub roundtable_at_home: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dungeon_warp: true,
            map_in_combat: true,
            auto_pickup: true,
            skip_flask_confirm: true,
            anti_farm_shop: true,
            consume_all_runes: true,
            merchant_bell_bearing: true,
            roundtable_at_home: true,
        }
    }
}

static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();

pub fn config() -> &'static Mutex<Config> {
    CONFIG.get_or_init(|| Mutex::new(Config::default()))
}

/// Returns `false` if the requested feature is disabled in the config. This is
/// the standard guard used by feature patchers before doing any work.
pub fn with_feature<F>(extract: F) -> bool
where
    F: FnOnce(&Config) -> bool,
{
    let cfg = config().lock().unwrap_or_else(|e| e.into_inner());
    extract(&cfg)
}

fn settings_path() -> Option<PathBuf> {
    log::dll_parent().map(|dir| dir.join("erqol_settings.toml"))
}

fn parse_bool(value: &str) -> bool {
    matches!(value.trim(), "true" | "1" | "yes")
}

pub fn load() {
    let Some(path) = settings_path() else {
        log("config: could not resolve DLL path; using defaults");
        return;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        log(format!("config: no settings file at {}; using defaults", path.display()));
        save(); // write defaults so the user can edit them
        return;
    };

    let mut cfg = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "dungeon_warp" => cfg.dungeon_warp = parse_bool(value),
            "map_in_combat" => cfg.map_in_combat = parse_bool(value),
            "auto_pickup" => cfg.auto_pickup = parse_bool(value),
            "skip_flask_confirm" => cfg.skip_flask_confirm = parse_bool(value),
            "anti_farm_shop" => cfg.anti_farm_shop = parse_bool(value),
            "consume_all_runes" => cfg.consume_all_runes = parse_bool(value),
            "merchant_bell_bearing" => cfg.merchant_bell_bearing = parse_bool(value),
            "roundtable_at_home" => cfg.roundtable_at_home = parse_bool(value),
            _ => {}
        }
    }

    log(format!(
        "config: loaded from {}; dw={} mc={} ap={} sf={} afs={} car={} mbb={} rth={}",
        path.display(),
        cfg.dungeon_warp,
        cfg.map_in_combat,
        cfg.auto_pickup,
        cfg.skip_flask_confirm,
        cfg.anti_farm_shop,
        cfg.consume_all_runes,
        cfg.merchant_bell_bearing,
        cfg.roundtable_at_home,
    ));

    *config().lock().unwrap_or_else(|e| e.into_inner()) = cfg;
}

pub fn save() {
    let Some(path) = settings_path() else {
        return;
    };
    let cfg = config().lock().unwrap_or_else(|e| e.into_inner());
    let content = format!(
        "# ERQoL Settings\n\
         # Edit values and restart the game for changes to take effect.\n\
         # Runtime-toggleable features (no restart needed):\n\
         #   auto_pickup, skip_flask_confirm, anti_farm_shop,\n\
         #   consume_all_runes, merchant_bell_bearing, roundtable_at_home\n\
         \n\
         dungeon_warp = {dungeon_warp}\n\
         map_in_combat = {map_in_combat}\n\
         auto_pickup = {auto_pickup}\n\
         skip_flask_confirm = {skip_flask_confirm}\n\
         anti_farm_shop = {anti_farm_shop}\n\
         consume_all_runes = {consume_all_runes}\n\
         merchant_bell_bearing = {merchant_bell_bearing}\n\
         roundtable_at_home = {roundtable_at_home}\n",
        dungeon_warp = cfg.dungeon_warp,
        map_in_combat = cfg.map_in_combat,
        auto_pickup = cfg.auto_pickup,
        skip_flask_confirm = cfg.skip_flask_confirm,
        anti_farm_shop = cfg.anti_farm_shop,
        consume_all_runes = cfg.consume_all_runes,
        merchant_bell_bearing = cfg.merchant_bell_bearing,
        roundtable_at_home = cfg.roundtable_at_home,
    );
    drop(cfg);

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match fs::write(&path, &content) {
        Ok(()) => log(format!("config: saved to {}", path.display())),
        Err(e) => log(format!("config: failed to save: {e}")),
    }
}

// ---- Feature-specific runtime toggle for dungeon_warp ----

use std::sync::atomic::{AtomicBool, Ordering};

pub static DUNGEON_WARP_ENABLED: AtomicBool = AtomicBool::new(true);

/// Applies the current config to runtime toggle state. Called after loading.
pub fn apply_to_runtime() {
    let cfg = config().lock().unwrap_or_else(|e| e.into_inner());
    DUNGEON_WARP_ENABLED.store(cfg.dungeon_warp, Ordering::Relaxed);
}
