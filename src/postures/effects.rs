//! Runtime application of posture selections as SpEffects.
//!
//! Replaces the AuraFarmingPostures persistence layer: that mod stores
//! selections as dummy inventory goods (605xxx) and re-applies matching
//! SpEffects through looping `common.emevd` watcher events. We keep
//! selections in our own config and drive the same effect ids directly via
//! [`ChrInsExt::apply_speffect`], so no goods, ESD helpers or EMEVD events
//! are needed.
//!
//! Effect ids extracted from the mod's EMEVD/ESD:
//! - body styles: goods 605000..605100 -> effects 6805000..6805100
//! - right arm:   Match Body 6805600, styles 6805601..6805610
//! - left arm:    Match Body 6805740, styles 6805741..6805750
//! - maintain-while-moving ON toggles (Settings menu): walking 6805111,
//!   running 6805131, sprinting 6805141
//! - alternative landing toggle: good 605120 -> effect 6805121
//! - movement style (Walking/Running/Sprinting submenus): heavy-load anims
//!   walk 6805790 / run 6805792 / sprint 6805794; defaults 6805791/93/95
//! - 6805099: generic "posture config changed" ping granted after changes.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fromsoftware_shared::Subclass;

use eldenring::cs::{ChrInsExt, PlayerIns};

use crate::config;
use crate::log::log;

/// "Posture configuration changed" ping the mod grants after every selection.
const REFRESH_EFFECT: i32 = 680_5099;

/// Effect per body style; index mirrors `config.posture_body`.
/// 0 = Normal (no effect); 1 Type A / 2 Alternative share the base effect
/// (good 605000) — the game picks the variant itself.
const BODY_EFFECTS: [i32; 13] = [
    0,
    680_5000,
    680_5000,
    680_5010,
    680_5020,
    680_5030,
    680_5040,
    680_5050,
    680_5060,
    680_5070,
    680_5080,
    680_5090,
    680_5100,
];

/// Effect per arm style; index mirrors `config.posture_right_arm` /
/// `config.posture_left_arm`. The mod's general arm lists are Match Body +
/// Chivalric..Lithe; our extra "Force Normal" entry maps to clearing the arm
/// range entirely (no arm override effect).
const RIGHT_ARM_EFFECTS: [i32; 12] = [
    680_5600, 0, 680_5601, 680_5602, 680_5603, 680_5604, 680_5605, 680_5606, 680_5607, 680_5608,
    680_5609, 680_5610,
];

const LEFT_ARM_EFFECTS: [i32; 12] = [
    680_5740, 0, 680_5741, 680_5742, 680_5743, 680_5744, 680_5745, 680_5746, 680_5747, 680_5748,
    680_5749, 680_5750,
];

/// Every managed effect id; anything not currently selected is removed from
/// the player when found (this is what keeps stale selections from stacking).
const MANAGED_EFFECTS: [i32; 43] = [
    680_5000, 680_5010, 680_5020, 680_5030, 680_5040, 680_5050, 680_5060, 680_5070, 680_5080,
    680_5090, 680_5100, //
    680_5600, 680_5601, 680_5602, 680_5603, 680_5604, 680_5605, 680_5606, 680_5607, 680_5608,
    680_5609, 680_5610, //
    680_5740, 680_5741, 680_5742, 680_5743, 680_5744, 680_5745, 680_5746, 680_5747, 680_5748,
    680_5749, 680_5750, //
    680_5111, 680_5131, 680_5141, // maintain-while-moving (ON toggles)
    680_5121,                     // alternative landing
    680_5790, 680_5792, 680_5794, // movement: heavy walk/run/sprint
    680_5791, 680_5793, 680_5795, // movement: default walk/run/sprint
];

/// "Maintain General Posture while walking/running/sprinting" — the mod's
/// Settings toggles. Baked on: always applied while postures are enabled.
const MAINTAIN_MOVING_EFFECTS: [i32; 3] = [680_5111, 680_5131, 680_5141];

/// Alternative landing toggle effect (good 605120 in the mod).
const ALT_LANDING_EFFECT: i32 = 680_5121;

/// Movement style effects; heavy switches locomotion anims to the heavy-load
/// set, default mirrors the mod's explicit "Default" selection grants.
const MOVE_HEAVY_EFFECTS: [i32; 3] = [680_5790, 680_5792, 680_5794];
const MOVE_DEFAULT_EFFECTS: [i32; 3] = [680_5791, 680_5793, 680_5795];

static REQUEST_SYNC: AtomicBool = AtomicBool::new(true);
static TICK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Re-sync interval in frames (~0.5 s at 60 fps), mirroring the mod's watcher
/// cadence of re-asserting effects after death/respawn clears them.
const SYNC_INTERVAL_TICKS: u64 = 30;

/// Called by menu actions so a selection applies immediately instead of on the
/// next periodic sync.
pub fn request_sync() {
    REQUEST_SYNC.store(true, Ordering::Relaxed);
}

fn desired_effects() -> Vec<i32> {
    let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
    if !cfg.postures_enabled {
        return Vec::new();
    }
    let pick = |table: &[i32], value: i32| -> i32 {
        table[value.rem_euclid(table.len() as i32) as usize]
    };
    let mut desired = vec![
        pick(&BODY_EFFECTS, cfg.posture_body),
        pick(&RIGHT_ARM_EFFECTS, cfg.posture_right_arm),
        pick(&LEFT_ARM_EFFECTS, cfg.posture_left_arm),
    ];
    desired.extend_from_slice(&MAINTAIN_MOVING_EFFECTS);
    if cfg.posture_alternative_landing {
        desired.push(ALT_LANDING_EFFECT);
    }
    if cfg.posture_movement != 0 {
        desired.extend_from_slice(&MOVE_HEAVY_EFFECTS);
    } else {
        desired.extend_from_slice(&MOVE_DEFAULT_EFFECTS);
    }
    desired.retain(|&effect| effect != 0);
    desired
}

unsafe fn has_effect(player: &PlayerIns, effect_id: i32) -> bool {
    unsafe {
        let special_effect = player.superclass().special_effect.as_ptr();
        if special_effect.is_null() {
            return false;
        }
        (*special_effect).entries().any(|e| e.param_id == effect_id)
    }
}

/// Returns true if any effect was applied or removed this pass.
unsafe fn sync_effects(desired: &[i32]) -> bool {
    unsafe {
        let Ok(player) = PlayerIns::local_player_mut() else {
            return false;
        };

        let mut changed = false;
        for &effect in MANAGED_EFFECTS.iter() {
            if desired.contains(&effect) {
                continue;
            }
            if has_effect(&player, effect) {
                player.remove_speffect(effect);
                changed = true;
            }
        }
        for &effect in desired {
            if !has_effect(&player, effect) {
                player.apply_speffect(effect, false);
                player.apply_speffect(REFRESH_EFFECT, false);
                changed = true;
            }
        }
        changed
    }
}

/// Per-frame entry point (`CSTaskGroupIndex::FrameBegin`). Applies missing
/// selected effects, removes unselected ones, and re-asserts selections after
/// the game clears speffects (death, respawn, area transitions).
pub fn tick() {
    let force = REQUEST_SYNC.swap(false, Ordering::Relaxed);
    let ticks = TICK_COUNTER.fetch_add(1, Ordering::Relaxed);
    if !force && ticks % SYNC_INTERVAL_TICKS != 0 {
        return;
    }

    let desired = desired_effects();
    let changed = unsafe { sync_effects(&desired) };
    if changed {
        log(&format!("postures: synced effects {:?}", desired));
    }
}
