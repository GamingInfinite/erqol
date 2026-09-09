//! Allow spirit ashes to be summoned anywhere (skip the Rebirth-Monument gate).
//!
//! `SummonBuddyManager::SummonGoodsState` (1.16.2 `0x1404b6d90`, located by
//! signature in the live build) decides whether a spirit ash can be used and
//! returns one of `BuddyGoodsState`: `Use=0`, `Replace=1`, `SendBack=2`,
//! `Disabled=-1`. A *fresh* summon (no buddy alive) is only allowed near a
//! "buddy stone" (Rebirth Monument): the player must be close enough that the
//! stone's area logic applied a SpEffect with `stateInfo == 0x175` AND set
//! `manager.buddyStoneTalkChrEntityId != 0`; otherwise the function returns
//! `Disabled (-1)` and the summon silently does nothing.
//!
//! The no-alive-summon branch looks like:
//! ```text
//!   test al,al      ; AL = 0x175-effect present && stone-context valid
//!   jz  <Disabled>  ; 74 60 -> without the effect: block
//!   xor eax,eax     ; return Use (0)
//! ```
//! NOP-ing that `jz` means a fresh summon *always* returns `Use`, so
//! `ApplyBuddySpawnSpEffect` proceeds and the buddy spawns near the player
//! anywhere (the free-roam spawn path raycasts for ground around the player
//! and does not need stone context). The alive-summon branch (Replace/
//! SendBack/duplicate-group guards) is untouched.
//!
//! A second patch NOPs the per-frame "out of the monument's ranges -> retire
//! all summons" call (`DespawnAll`), otherwise a buddy summoned far from any
//! Rebirth Monument is despawned a couple of seconds after spawning.
//!
//! Signature derived from the 1.16.2 Ghidra analysis
//! (`0x1404b6dfc`, `SummonBuddyManager::SummonGoodsState`) and its unique
//! byte pattern confirmed against the live build in radare2 (1.17: match start
//! `0x1404b735c`, gate `0x1404b739b`). The window is stable between builds
//! apart from the rel32 call displacement (wildcarded).
use std::sync::Once;

use crate::{log, memory, scan};

/// SummonGoodsState gate: `mov edx,0x175` .. `test al,al; jz <gate>; xor eax,eax`.
/// The `E8 ? ? ? ?` call (HasSpecialEffectWithStateInfo) and the gate's branch
/// displacement are the only build-dependent bytes.
const GATE_PATTERN: &str = "BA 75 01 00 00 48 8B 88 78 01 00 00 E8 ? ? ? ? 84 C0 74 10 \
     8B 43 38 85 C0 74 09 3B 43 3C 74 04 B0 01 EB 02 32 C0 85 F6 78 0B \
     3B B3 E4 00 00 00 0F 94 C1 EB 02 B1 01 40 84 FF 75 16 84 C0 74 ? 33 C0";

/// Offset of the gating `jz` (the `74 ?` two bytes) from the match start.
const GATE_JZ_OFF: usize = 0x3f;

/// Out-of-range retirement check run from `WorldChrMan_PostPhysics` (the
/// per-frame buddy lifetime function, 1.16.2 `FUN_1404b8d90`): when no summon
/// is in progress (`requestSummonSpEffect < 0`, `disappearRequested` reset)
/// and the player is outside both the active stone's warn range (+0xb7) and
/// activate range (+0xb5), the manager retires every summon. This is the
/// vanilla "you left the monument area -> summons return" mechanic that would
/// otherwise kill a buddy summoned far from any monument. The short branch
/// displacements and the call rel32 are the only build-dependent bytes.
const DESPAWN_ALL_PATTERN: &str = "41 83 7F 20 00 41 C6 47 28 00 7D ? \
     41 80 BF B7 00 00 00 00 75 ? 41 80 BF B5 00 00 00 00 75 ? \
     49 8B CF E8 ? ? ? ?";

/// Offset of the `call DespawnAll` (5 bytes) from the pattern start.
const DESPAWN_ALL_CALL: usize = 0x23;

pub static SPIRIT_SUMMON_INSTALLER: Once = Once::new();

/// NOP the gating `jz` so a fresh summon with no stone context returns `Use`
/// instead of `Disabled`. Idempotent; refuses to clobber an already-patched
/// site or a site whose opcode no longer matches `jz`.
pub fn install() {
    let Some(gate) = scan::scan_pattern(GATE_PATTERN) else {
        log::log("spirit_summon: ERROR: SummonGoodsState gate signature not found");
        return;
    };
    let jz = gate + GATE_JZ_OFF as u64;
    let live = unsafe { std::slice::from_raw_parts(jz as *const u8, 2) };
    log::log(format!(
        "spirit_summon: SummonGoodsState gate at {jz:#x} bytes={live:02x?}"
    ));
    match live {
        [0x90, 0x90] => {
            log::log("spirit_summon: already patched");
        }
        [0x74, _] => {
            if !unsafe { memory::patch_bytes(jz, &[0x90, 0x90]) } {
                log::log("spirit_summon: ERROR: failed to patch gate JZ");
                return;
            }
            log::log("spirit_summon: enabled (spirit ashes usable anywhere)");
        }
        _ => {
            log::log(format!(
                "spirit_summon: ERROR: gate opcode mismatch ({live:02x?}); skipping to avoid corrupting code"
            ));
        }
    }

    let Some(despawn) = scan::scan_pattern(DESPAWN_ALL_PATTERN) else {
        log::log("spirit_summon: ERROR: out-of-range DespawnAll signature not found");
        return;
    };
    let despawn_call = despawn + DESPAWN_ALL_CALL as u64;
    let live_call = unsafe { std::slice::from_raw_parts(despawn_call as *const u8, 5) };
    log::log(format!(
        "spirit_summon: despawn-all call at {despawn_call:#x} bytes={live_call:02x?}"
    ));
    match live_call {
        [0x90, 0x90, 0x90, 0x90, 0x90] => {
            log::log("spirit_summon: despawn-all already patched");
        }
        [0xE8, ..] => {
            if !unsafe { memory::patch_bytes(despawn_call, &[0x90; 5]) } {
                log::log("spirit_summon: ERROR: failed to patch despawn-all call");
                return;
            }
            log::log("spirit_summon: out-of-range despawn disabled (summons persist away from monuments)");
        }
        _ => {
            log::log(format!(
                "spirit_summon: ERROR: despawn-all call opcode mismatch ({live_call:02x?}); skipping"
            ));
        }
    }
}