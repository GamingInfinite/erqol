use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::ezstate_menu::{SubMenuAction, alloc_message_id, update_message};
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

/// Which settings submenu a feature's row appears in. `None` hides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    /// Changes that don't affect combat results — they skip quit-outs, prevent
    /// a warp, or speed things up (e.g. dungeon_warp, auto_pickup).
    Qol,
    /// Direct gameplay tweaks that alter how a fight can be played out
    /// (e.g. spirit summoning everywhere).
    Tweaks,
    /// Silly string replacements and other jokes (see the `silly` module).
    Silly,
}

/// A single on/off config knob. [`TOGGLES`] is the single source of truth for
/// every bool setting: defaults, toml load/save and the grace-menu toggle rows
/// all derive from it, so a new knob is one entry here plus its field on
/// [`BoolFlags`] — no edits in `load`, `save` or the settings menu.
pub(crate) struct BoolToggle {
    /// `erqol_settings.toml` key.
    pub(crate) key: &'static str,
    /// Grace-menu row label. Empty for knobs without a menu row.
    pub(crate) label: &'static str,
    /// Settings category; `None` keeps the knob out of the menus.
    pub(crate) category: Option<Category>,
    /// True if flipping the knob only takes full effect after a restart.
    pub(crate) needs_reload: bool,
    /// Value applied when the toml has no entry for the key.
    pub(crate) default: bool,
    pub(crate) get: fn(&Config) -> bool,
    pub(crate) set: fn(&mut Config, bool),
    /// Menu row action; `None` for knobs without a menu row.
    pub(crate) action: Option<SubMenuAction>,
    /// Side-effect run right after the knob is flipped (e.g. a runtime atomic
    /// mirror of a feature that takes effect without a restart).
    pub(crate) on_change: Option<fn(bool)>,
}

/// All bool config flags. `#[derive(Default)]` gives every flag `false`; the
/// per-toggle defaults in [`TOGGLES`] are applied on top of that.
#[derive(Debug, Clone, Default)]
pub struct BoolFlags {
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
    pub posture_alternative_landing: bool,
    pub spirit_summon_everywhere: bool,
    pub hook_enter_state: bool,
    pub hook_lookup_entry: bool,
    pub map_icons: bool,
    pub map_icon_points: bool,
    pub silly_you_died: bool,
    pub silly_area_names: bool,
    pub silly_boss_names: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Bool feature flags (see [`TOGGLES`]).
    pub flags: BoolFlags,
    pub posture_body: i32,
    pub posture_right_arm: i32,
    pub posture_left_arm: i32,
    pub posture_movement: i32,
    pub hp_bar_tracks: HpBarTracks,
    pub map_icon_point_label: String,
}

/// Convenience accessors keep every existing `cfg.<flag>` call site working
/// while the flags live in the nested [`BoolFlags`].
impl std::ops::Deref for Config {
    type Target = BoolFlags;
    fn deref(&self) -> &BoolFlags {
        &self.flags
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut BoolFlags {
        &mut self.flags
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut cfg = Self {
            flags: BoolFlags::default(),
            posture_body: 0,
            posture_right_arm: 0,
            posture_left_arm: 0,
            posture_movement: 0,
            hp_bar_tracks: HpBarTracks::Hp,
            map_icon_point_label: String::from("Point"),
        };
        for toggle in TOGGLES {
            (toggle.set)(&mut cfg, toggle.default);
        }
        cfg
    }
}

/// Single source of truth for every bool setting. Order roughly matches the
/// settings file layout; the settings menu groups rows by `category`.
pub(crate) static TOGGLES: &[BoolToggle] = &[
    BoolToggle {
        key: "dungeon_warp",
        label: "Dungeon Warp",
        category: Some(Category::Qol),
        needs_reload: true,
        default: true,
        get: |c| c.dungeon_warp,
        set: |c, v| c.dungeon_warp = v,
        action: Some(toggle_dungeon_warp),
        on_change: Some(|v| DUNGEON_WARP_ENABLED.store(v, Ordering::Relaxed)),
    },
    BoolToggle {
        key: "map_in_combat",
        label: "Map in Combat",
        category: Some(Category::Qol),
        needs_reload: true,
        default: true,
        get: |c| c.map_in_combat,
        set: |c, v| c.map_in_combat = v,
        action: Some(toggle_map_in_combat),
        on_change: None,
    },
    BoolToggle {
        key: "auto_pickup",
        label: "Auto Pickup",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.auto_pickup,
        set: |c, v| c.auto_pickup = v,
        action: Some(toggle_auto_pickup),
        on_change: None,
    },
    BoolToggle {
        key: "skip_flask_confirm",
        label: "Skip Flask Confirm",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.skip_flask_confirm,
        set: |c, v| c.skip_flask_confirm = v,
        action: Some(toggle_skip_flask),
        on_change: None,
    },
    BoolToggle {
        key: "anti_farm_shop",
        label: "Anti-Farm Shop",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.anti_farm_shop,
        set: |c, v| c.anti_farm_shop = v,
        action: Some(toggle_anti_farm),
        on_change: None,
    },
    BoolToggle {
        key: "consume_all_runes",
        label: "Consume All Runes",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.consume_all_runes,
        set: |c, v| c.consume_all_runes = v,
        action: Some(toggle_consume_runes),
        on_change: None,
    },
    BoolToggle {
        key: "merchant_bell_bearing",
        label: "Merchant Bell Bearing",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.merchant_bell_bearing,
        set: |c, v| c.merchant_bell_bearing = v,
        action: Some(toggle_merchant_bell),
        on_change: None,
    },
    BoolToggle {
        key: "roundtable_at_home",
        label: "Roundtable at Home",
        category: Some(Category::Qol),
        needs_reload: false,
        default: true,
        get: |c| c.roundtable_at_home,
        set: |c, v| c.roundtable_at_home = v,
        action: Some(toggle_roundtable),
        on_change: None,
    },
    BoolToggle {
        key: "spirit_summon_everywhere",
        label: "Spirit Summons Everywhere",
        category: Some(Category::Tweaks),
        needs_reload: true,
        default: true,
        get: |c| c.spirit_summon_everywhere,
        set: |c, v| c.spirit_summon_everywhere = v,
        action: Some(toggle_spirit_summon),
        on_change: None,
    },
    BoolToggle {
        key: "postures_enabled",
        label: "",
        category: None,
        needs_reload: true,
        default: true,
        get: |c| c.postures_enabled,
        set: |c, v| c.postures_enabled = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "postures_hks_inject",
        label: "",
        category: None,
        needs_reload: true,
        default: true,
        get: |c| c.postures_hks_inject,
        set: |c, v| c.postures_hks_inject = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "posture_alternative_landing",
        label: "",
        category: None,
        needs_reload: false,
        default: false,
        get: |c| c.posture_alternative_landing,
        set: |c, v| c.posture_alternative_landing = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "hook_enter_state",
        label: "",
        category: None,
        needs_reload: true,
        default: true,
        get: |c| c.hook_enter_state,
        set: |c, v| c.hook_enter_state = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "hook_lookup_entry",
        label: "",
        category: None,
        needs_reload: true,
        default: true,
        get: |c| c.hook_lookup_entry,
        set: |c, v| c.hook_lookup_entry = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "map_icons",
        label: "",
        category: None,
        needs_reload: true,
        default: true,
        get: |c| c.map_icons,
        set: |c, v| c.map_icons = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "map_icon_points",
        label: "",
        category: None,
        needs_reload: false,
        default: false,
        get: |c| c.map_icon_points,
        set: |c, v| c.map_icon_points = v,
        action: None,
        on_change: None,
    },
    BoolToggle {
        key: "silly_you_died",
        label: "You Died Replacement",
        category: Some(Category::Silly),
        needs_reload: false,
        default: true,
        get: |c| c.silly_you_died,
        set: |c, v| c.silly_you_died = v,
        action: Some(toggle_you_died),
        on_change: None,
    },
    BoolToggle {
        key: "silly_area_names",
        label: "Area Name Replacement",
        category: Some(Category::Silly),
        needs_reload: false,
        default: true,
        get: |c| c.silly_area_names,
        set: |c, v| c.silly_area_names = v,
        action: Some(toggle_area_names),
        on_change: None,
    },
    BoolToggle {
        key: "silly_boss_names",
        label: "Boss Name Replacement",
        category: Some(Category::Silly),
        needs_reload: false,
        default: true,
        get: |c| c.silly_boss_names,
        set: |c, v| c.silly_boss_names = v,
        action: Some(toggle_boss_names),
        on_change: None,
    },
];

// ---- Menu toggle actions ----

/// Every menu row's action delegates to [`apply_toggle`], so the flip, save,
/// menu refresh and side-effect handling stay in exactly one place.
unsafe extern "C" fn toggle_dungeon_warp() {
    apply_toggle("dungeon_warp");
}

unsafe extern "C" fn toggle_map_in_combat() {
    apply_toggle("map_in_combat");
}

unsafe extern "C" fn toggle_auto_pickup() {
    apply_toggle("auto_pickup");
}

unsafe extern "C" fn toggle_skip_flask() {
    apply_toggle("skip_flask_confirm");
}

unsafe extern "C" fn toggle_anti_farm() {
    apply_toggle("anti_farm_shop");
}

unsafe extern "C" fn toggle_consume_runes() {
    apply_toggle("consume_all_runes");
}

unsafe extern "C" fn toggle_merchant_bell() {
    apply_toggle("merchant_bell_bearing");
}

unsafe extern "C" fn toggle_roundtable() {
    apply_toggle("roundtable_at_home");
}

unsafe extern "C" fn toggle_spirit_summon() {
    apply_toggle("spirit_summon_everywhere");
}

unsafe extern "C" fn toggle_you_died() {
    apply_toggle("silly_you_died");
}

unsafe extern "C" fn toggle_area_names() {
    apply_toggle("silly_area_names");
}

unsafe extern "C" fn toggle_boss_names() {
    apply_toggle("silly_boss_names");
}

/// Message ids for the menu rows, allocated only for knobs that are shown in a
/// settings menu. Hidden knobs get -1.
static MENU_MSG_IDS: LazyLock<Vec<i32>> = LazyLock::new(|| {
    TOGGLES
        .iter()
        .map(|t| {
            if t.category.is_some() {
                alloc_message_id()
            } else {
                -1
            }
        })
        .collect()
});

/// Message id for a menu toggle's row, or -1 when it has no menu row.
pub(crate) fn menu_msg_id(key: &str) -> i32 {
    TOGGLES
        .iter()
        .position(|t| t.key == key)
        .map(|i| MENU_MSG_IDS[i])
        .unwrap_or(-1)
}

/// Current label text for a menu toggle's row, e.g. `[ON] Auto Pickup`.
pub(crate) fn menu_label(key: &str) -> String {
    let Some(toggle) = TOGGLES.iter().find(|t| t.key == key) else {
        return String::new();
    };
    let enabled = (toggle.get)(&config().lock().unwrap_or_else(|e| e.into_inner()));
    let state = if enabled { "[ON]" } else { "[OFF]" };
    let suffix = if toggle.needs_reload { " (restart to change)" } else { "" };
    format!("{state} {}{suffix}", toggle.label)
}

/// Flips a bool toggle by toml key, persists it, refreshes its menu row text
/// and runs the toggle's `on_change` side-effect. Shared by every menu action.
fn apply_toggle(key: &str) {
    let Some(idx) = TOGGLES.iter().position(|t| t.key == key) else {
        return;
    };
    let toggle = &TOGGLES[idx];
    let mut cfg = config().lock().unwrap_or_else(|e| e.into_inner());
    let before = (toggle.get)(&cfg);
    (toggle.set)(&mut cfg, !before);
    let now = (toggle.get)(&cfg);
    drop(cfg);
    save();
    if let Some(cb) = toggle.on_change {
        cb(now);
    }
    if MENU_MSG_IDS[idx] != -1 {
        update_message(MENU_MSG_IDS[idx], &menu_label(key));
    }
    let note = if toggle.needs_reload { " (restart to fully apply)" } else { "; active now" };
    log(&format!("grace_settings: {key} toggled{note}"));
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
        log(format!(
            "config: no settings file at {}; using defaults",
            path.display()
        ));
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

        // Every bool flag lives in the registry: parse it straight from there.
        if let Some(toggle) = TOGGLES.iter().find(|t| t.key == key) {
            (toggle.set)(&mut cfg, parse_bool(value));
            continue;
        }

        match key {
            "posture_body" => cfg.posture_body = value.parse().unwrap_or(0),
            "posture_right_arm" => cfg.posture_right_arm = value.parse().unwrap_or(0),
            "posture_left_arm" => cfg.posture_left_arm = value.parse().unwrap_or(0),
            "posture_movement" => cfg.posture_movement = value.parse().unwrap_or(0),
            "hp_bar_tracks" => cfg.hp_bar_tracks = parse_hp_bar_tracks(value),
            "map_icon_point_label" => cfg.map_icon_point_label = value.trim().to_string(),
            _ => {}
        }
    }

    let summary = TOGGLES
        .iter()
        .map(|t| format!("{}={}", t.key, (t.get)(&cfg)))
        .collect::<Vec<_>>()
        .join(" ");
    log(format!(
        "config: loaded from {}; {}; posture={},{},{},{}; hp={}; label={:?}",
        path.display(),
        summary,
        cfg.posture_body,
        cfg.posture_right_arm,
        cfg.posture_left_arm,
        cfg.posture_movement,
        cfg.hp_bar_tracks.as_str(),
        cfg.map_icon_point_label,
    ));

    *config().lock().unwrap_or_else(|e| e.into_inner()) = cfg;
}

pub fn save() {
    let Some(path) = settings_path() else {
        return;
    };
    let cfg = config().lock().unwrap_or_else(|e| e.into_inner());

    let mut content = format!(
        "# ERQoL Settings\n\
         # Edit values and restart the game for changes to take effect.\n\
         # Runtime-toggleable features can also be flipped from the in-game\n\
         # ERQoL Settings menu (no restart needed for those).\n\
         # Definitions:\n\
         #   hook_enter_state   -- EzState::EnterState entry hook (debug)\n\
         #   hook_lookup_entry  -- MsgRepositoryImp::LookupEntry entry hook (debug)\n\
         #   silly_you_died     -- replace the \"You Died\" death text (default on)\n\
         #   silly_area_names   -- replace area-name splash texts (default on)\n\
         #   silly_boss_names   -- replace boss-name texts (default on)\n\
         \n"
    );
    for toggle in TOGGLES {
        content.push_str(&format!("{} = {}\n", toggle.key, (toggle.get)(&cfg)));
    }
    content.push_str(&format!("posture_body = {}\n", cfg.posture_body));
    content.push_str(&format!("posture_right_arm = {}\n", cfg.posture_right_arm));
    content.push_str(&format!("posture_left_arm = {}\n", cfg.posture_left_arm));
    content.push_str(&format!("posture_movement = {}\n", cfg.posture_movement));
    content.push_str(&format!("hp_bar_tracks = {}\n", cfg.hp_bar_tracks.as_str()));
    content.push_str(&format!(
        "map_icon_point_label = {:?}\n",
        cfg.map_icon_point_label
    ));
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

pub static DUNGEON_WARP_ENABLED: AtomicBool = AtomicBool::new(true);

/// Applies the current config to runtime toggle state. Called after loading.
pub fn apply_to_runtime() {
    let cfg = config().lock().unwrap_or_else(|e| e.into_inner());
    DUNGEON_WARP_ENABLED.store(cfg.dungeon_warp, Ordering::Relaxed);
}