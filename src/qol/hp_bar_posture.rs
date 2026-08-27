//! Reimplements Erd-Tools-CPP's boss "poise meter" hook: it can repurpose the
//! damage readout above boss health bars to display the boss's remaining
//! posture (stagger) instead of the recent HP damage dealt.
//!
//! Ported from `Erd-Tools-CPP/Hook/FeHook.cpp` (`writePoiseToBossBar`). The
//! target is `FUN_140776520` — the per-frame boss bar damage-fade timer that
//! iterates all 3 `bossHealthDisplays` entries every frame. We hook it with a
//! retn hook: call the original first (fade timer logic), then overwrite
//! `damageTaken` / `isHit` with posture when in Posture mode.
//!
//! Unlike Erd-Tools we do *not* NOP `_applyBossBarDmg` — leaving it intact keeps
//! the stock HP-damage readout working in `Hp` mode, and in `Posture` mode our
//! per-frame write runs after the original, so it always wins.

use std::sync::Once;

use eldenring::cs::{CSFeManImp, WorldChrMan};
use fromsoftware_shared::FromStatic;
use ilhook::x64::Registers;

use crate::{config, hooks, log, memory, scan};

/// AOB for `FUN_140776520` — the per-frame boss bar damage-fade timer.
/// Reads/writes `damageTaken`, `field7_0x14`, `field_0x18` (isHit),
/// `field_0x1c` (fade timer) on each of the 3 `bossHealthDisplays` entries.
/// Wildcards: two RIP-relative offsets in MOV RCX and MOVSS XMM7.
const UPDATE_BOSS_BAR_TIMER: &str =
    "48 83 EC 48 48 8B 0D ? ? ? ? 0F 29 74 24 30 0F 28 F1 48 85 C9 0F 84 ? ? ? ? 48 89 5C 24 50 48 89 74 24 58 BE FF FF FF FF 48 89 7C 24 40 33 FF 0F 29 7C 24 20 8B DF F3 0F 10 3D ? ? ? ?";

/// `ChrIns + 0x190` -> `ChrModuleBag*` (Erd-Tools-CPP `ChrIns.chrModulelBag`).
const OFFSET_CHR_MODULE_BAG: u64 = 0x190;
/// `ChrModuleBag + 0x40` -> `StaggerModule*` (Erd-Tools-CPP `staggerModule`).
const OFFSET_STAGGER_MODULE: u64 = 0x40;
/// `StaggerModule + 0x10` -> `stagger` (remaining posture, f32).
const OFFSET_STAGGER: u64 = 0x10;

pub static HP_BAR_POSTURE_INSTALLER: Once = Once::new();

/// Writes each visible boss's remaining posture into its health-bar readout.
fn write_posture_to_boss_bars() {
    unsafe {
        let Ok(fe) = CSFeManImp::instance_mut() else {
            return;
        };
        let Ok(world) = WorldChrMan::instance() else {
            return;
        };

        for i in 0..3 {
            let handle = fe.boss_health_displays[i].field_ins_handle;
            if handle.is_empty() {
                continue;
            }
            let Some(chr) = world.chr_ins_by_handle(&handle) else {
                continue;
            };

            let chr_addr = chr as *const _ as u64;
            let module_bag = memory::read_qword(chr_addr + OFFSET_CHR_MODULE_BAG);
            if module_bag == 0 {
                continue;
            }
            let stagger_module = memory::read_qword(module_bag + OFFSET_STAGGER_MODULE);
            if stagger_module == 0 {
                continue;
            }

            let stagger = memory::read_f32(stagger_module + OFFSET_STAGGER);
            let damage = stagger as i32;

            let entry = &mut fe.boss_health_displays[i];
            if stagger > 0.0 {
                entry.damage_taken = damage;
                entry.is_hit = true;
            } else if entry.damage_taken > 0 && entry.damage_taken != damage {
                entry.damage_taken = damage;
                entry.is_hit = true;
            }
        }
    }
}

/// Retn-hook detour for the per-frame boss bar timer. Calls the original
/// first (fade timer logic), then overwrites `damageTaken` with posture
/// when Posture mode is active.
fn boss_bar_timer_detour(regs: *mut Registers, original: usize) -> usize {
    // Call the original — signature: void FUN_140776520(void* rcx, float deltaTime_xmm1)
    let original_fn: extern "win64" fn(u64, f32) = unsafe { std::mem::transmute(original) };
    let (rcx, xmm1) = unsafe { ((*regs).rcx, (*regs).xmm1) };
    let delta_time = f32::from_bits((xmm1 & 0xFFFF_FFFF) as u32);
    original_fn(rcx, delta_time);

    // In Posture mode, overwrite damageTaken / isHit with stagger.
    if config::with_feature(|c| c.hp_bar_tracks == config::HpBarTracks::Posture) {
        write_posture_to_boss_bars();
    }

    0 // void return
}

/// Installs the posture-readout hook. The hook is always installed; the
/// `HP Bar Tracks` setting (read live each frame) decides whether posture is
/// written or the stock HP-damage readout is left untouched.
pub fn install() {
    let Some(timer_addr) = scan::scan_pattern(UPDATE_BOSS_BAR_TIMER) else {
        log::log("hp_bar_posture: ERROR: boss bar timer signature not found");
        return;
    };
    log::log(format!("hp_bar_posture: boss_bar_timer={timer_addr:#x}"));

    hooks::install_retn("hp_bar_posture", timer_addr, boss_bar_timer_detour);
    log::log("hp_bar_posture: installed (live toggle via HP Bar Tracks setting)");
}
