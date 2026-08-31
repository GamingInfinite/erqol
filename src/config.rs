use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::log::{self, log};

/// What the damage readout above boss health bars tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpBarTracks {
    /// Default game behaviour: shows the recent HP damage dealt.
    Hp,
    /// Shows the boss's remaining posture (stagger) instead.
    Posture,
}

impl Default for HpBarTracks {
    fn default() -> Self {
        HpBarTracks::Hp
    }
}

impl HpBarTracks {
    fn as_str(self) -> &'static str {
        match self {
            HpBarTracks::Hp => "hp",
            HpBarTracks::Posture => "posture",
        }
    }
}

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
    pub postures_enabled: bool,
    pub postures_hks_inject: bool,
    pub posture_body: i32,
    pub posture_right_arm: i32,
    pub posture_left_arm: i32,
    pub posture_movement: i32,
    pub posture_alternative_landing: bool,
    pub hp_bar_tracks: HpBarTracks,
    pub spirit_summon_everywhere: bool,
    pub hook_enter_state: bool,
    pub hook_lookup_entry: bool,
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
            postures_enabled: true,
            postures_hks_inject: true,
            posture_body: 0,
            posture_right_arm: 0,
            posture_left_arm: 0,
            posture_movement: 0,
            posture_alternative_landing: false,
            hp_bar_tracks: HpBarTracks::Hp,
            spirit_summon_everywhere: true,
            hook_enter_state: true,
            hook_lookup_entry: true,
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

fn parse_hp_bar_tracks(value: &str) -> HpBarTracks {
    match value.trim().to_ascii_lowercase().as_str() {
        "posture" | "p" | "poise" => HpBarTracks::Posture,
        _ => HpBarTracks::Hp,
    }
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
            "postures_enabled" => cfg.postures_enabled = parse_bool(value),
            "postures_hks_inject" => cfg.postures_hks_inject = parse_bool(value),
            "posture_body" => cfg.posture_body = value.parse().unwrap_or(0),
            "posture_right_arm" => cfg.posture_right_arm = value.parse().unwrap_or(0),
            "posture_left_arm" => cfg.posture_left_arm = value.parse().unwrap_or(0),
            "posture_movement" => cfg.posture_movement = value.parse().unwrap_or(0),
            "posture_alternative_landing" => cfg.posture_alternative_landing = parse_bool(value),
            "hp_bar_tracks" => cfg.hp_bar_tracks = parse_hp_bar_tracks(value),
            "spirit_summon_everywhere" => cfg.spirit_summon_everywhere = parse_bool(value),
            "hook_enter_state" => cfg.hook_enter_state = parse_bool(value),
            "hook_lookup_entry" => cfg.hook_lookup_entry = parse_bool(value),
            _ => {}
        }
    }

    log(format!(
         "config: loaded from {}; dw={} mc={} ap={} sf={} afs={} car={} mbb={} rth={} pose={} phi={} pb={} pra={} pla={} pm={} pal={} sse={} hes={} hle={}",
        path.display(),
        cfg.dungeon_warp,
        cfg.map_in_combat,
        cfg.auto_pickup,
        cfg.skip_flask_confirm,
        cfg.anti_farm_shop,
        cfg.consume_all_runes,
        cfg.merchant_bell_bearing,
        cfg.roundtable_at_home,
        cfg.postures_enabled,
        cfg.postures_hks_inject,
         cfg.posture_body,
        cfg.posture_right_arm,
        cfg.posture_left_arm,
         cfg.posture_movement,
         cfg.posture_alternative_landing,
         cfg.spirit_summon_everywhere,
         cfg.hook_enter_state,
         cfg.hook_lookup_entry,
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
         #   consume_all_runes, merchant_bell_bearing, roundtable_at_home,\n\
         #   postures_enabled\n\
         # A/B debug toggles for ezstate_menu (both default true):\n\
         #   hook_enter_state  -- EzState::EnterState entry hook\n\
         #   hook_lookup_entry -- MsgRepositoryImp::LookupEntry entry hook\n\
         \n\
         dungeon_warp = {dungeon_warp}\n\
         map_in_combat = {map_in_combat}\n\
         auto_pickup = {auto_pickup}\n\
         skip_flask_confirm = {skip_flask_confirm}\n\
         anti_farm_shop = {anti_farm_shop}\n\
         consume_all_runes = {consume_all_runes}\n\
         merchant_bell_bearing = {merchant_bell_bearing}\n\
         roundtable_at_home = {roundtable_at_home}\n\
         postures_enabled = {postures_enabled}\n\
         postures_hks_inject = {postures_hks_inject}\n\
         posture_body = {posture_body}\n\
         posture_right_arm = {posture_right_arm}\n\
         posture_left_arm = {posture_left_arm}\n\
         posture_movement = {posture_movement}\n\
          posture_alternative_landing = {posture_alternative_landing}\n\
hp_bar_tracks = {hp_bar_tracks}\n\
           spirit_summon_everywhere = {spirit_summon_everywhere}\n\
           hook_enter_state = {hook_enter_state}\n\
           hook_lookup_entry = {hook_lookup_entry}\n",
        dungeon_warp = cfg.dungeon_warp,
        map_in_combat = cfg.map_in_combat,
        auto_pickup = cfg.auto_pickup,
        skip_flask_confirm = cfg.skip_flask_confirm,
        anti_farm_shop = cfg.anti_farm_shop,
        consume_all_runes = cfg.consume_all_runes,
        merchant_bell_bearing = cfg.merchant_bell_bearing,
        roundtable_at_home = cfg.roundtable_at_home,
        postures_enabled = cfg.postures_enabled,
        postures_hks_inject = cfg.postures_hks_inject,
        posture_body = cfg.posture_body,
        posture_right_arm = cfg.posture_right_arm,
        posture_left_arm = cfg.posture_left_arm,
        posture_movement = cfg.posture_movement,
        posture_alternative_landing = cfg.posture_alternative_landing,
        hp_bar_tracks = cfg.hp_bar_tracks.as_str(),
        spirit_summon_everywhere = cfg.spirit_summon_everywhere,
        hook_enter_state = cfg.hook_enter_state,
        hook_lookup_entry = cfg.hook_lookup_entry,
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
