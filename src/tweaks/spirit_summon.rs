//! "Spirit Summons Everywhere" — lets the player use spirit ashes in areas
//! that normally forbid them (caves, dungeons, most boss arenas).
//!
//! Implementation is a line-for-line port of the open-source SummonAnywhere
//! mod (MIT license, originally from soarqin's ER-EzMod). It patches two
//! runtime checks rather than editing params:
//!
//! 1. Summon range check (`SummonBuddyManager::Update`): when the game
//!    decides how far you can be from a Rebirth Monument, it checks a
//!    SpEffect id in a structure. The mod replaces the loaded distance with
//!    a large constant (1000.0f) when the id matches 0x7D0.
//!
//! 2. Area eligibility check (`Invoke`/`CSEzStateTalkEnv`): a small struct
//!    loaded from `[rbp-0x68]` marks the current area as summon-forbidden.
//!    The mod clears two of its fields whenever it is present.
//!
//! In addition, we patch `BuddyStoneParam` every frame to give every
//! Rebirth Monument a huge activation and return range, which covers the
//! case where a monument exists but has a very short/default range.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use eldenring::cs::{BuddyStoneParam, SoloParam, SoloParamRepository};
use fromsoftware_shared::FromStatic;

use crate::config;
use crate::log::log;
use crate::memory;
use crate::scan;

// ---------------------------------------------------------------------------
// Hook A: SummonBuddyManager per-frame range check (SummonAnywhere codecave)
// ---------------------------------------------------------------------------

/// Signature for the range check inside `SummonBuddyManager::Update`.
/// Matches:
/// ```text
/// mov     rax, [rdi+0x28]
/// movss   xmm2, dword ptr [rax+0x84]
/// comiss  xmm2, xmm0
/// ```
const RANGE_CHECK_PATTERN: &str =
    "48 8B 47 ? F3 0F 10 90 ? ? ? ? 0F 2F D0";

const RANGE_HOOK_LEN: usize = 12;

// mov rax,[rdi+0x28]; cmp dword [rax],0x7D0; jne +0x0A;
// movss xmm2,[rip+0x0F]; jmp +0x08;
// movss xmm2,[rax+0x84]; jmp <back>; dd 1000.0f
const RANGE_CAVE_TEMPLATE: &[u8] = &[
    0x48, 0x8B, 0x47, 0x28,
    0x81, 0x38, 0xD0, 0x07, 0x00, 0x00,
    0x0F, 0x85, 0x0A, 0x00, 0x00, 0x00,
    0xF3, 0x0F, 0x10, 0x15, 0x0F, 0x00, 0x00, 0x00,
    0xEB, 0x08,
    0xF3, 0x0F, 0x10, 0x90, 0x84, 0x00, 0x00, 0x00,
    0xE9, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x7A, 0x44,
];

const RANGE_RETURN_JMP_OFFSET: usize = 34;

// ---------------------------------------------------------------------------
// Hook B: Area eligibility struct clear (SummonAnywhere codecave)
// ---------------------------------------------------------------------------

/// Signature for the area eligibility struct load in `Invoke`.
/// Matches:
/// ```text
/// mov   rax, [rbp-0x68]
/// test  rax, rax
/// jz    0x140ea60b4
/// mov   eax, [rax+0x20]
/// ```
const AREA_ELIGIBILITY_PATTERN: &str =
    "48 8B 45 98 48 85 C0 0F 84 ? ? ? ? 8B 40 20";

const AREA_HOOK_LEN: usize = 7;

// mov rax,[rbp-0x68]; test rax,rax; je +0x0D;
// mov dword [rax+0x20],0; mov word [rax+0x1C],0xFFFF;
// test rax,rax; jmp <back>
const AREA_CAVE_TEMPLATE: &[u8] = &[
    0x48, 0x8B, 0x45, 0x98,
    0x48, 0x85, 0xC0,
    0x0F, 0x84, 0x0D, 0x00, 0x00, 0x00,
    0xC7, 0x40, 0x20, 0x00, 0x00, 0x00, 0x00,
    0x66, 0xC7, 0x40, 0x1C, 0xFF, 0xFF,
    0x48, 0x85, 0xC0,
    0xE9, 0x00, 0x00, 0x00, 0x00,
];

const AREA_RETURN_JMP_OFFSET: usize = 29;

// ---------------------------------------------------------------------------
// BuddyStoneParam edit
// ---------------------------------------------------------------------------

const MAX_ACTIVATE_RANGE: u16 = u16::MAX;
const MAX_RETURN_RANGE: i16 = i16::MAX;

static PARAM_PATCH_REPORTED: AtomicBool = AtomicBool::new(false);

/// Called every frame. Gives every Rebirth Monument the maximum possible
/// activation and return ranges, so any existing monument is reachable from
/// anywhere in its area.
pub fn patch_buddy_stone_param() {
    if !config::with_feature(|c| c.spirit_summon_everywhere) {
        return;
    }

    let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else {
        return;
    };

    let Some(holder) = repo.solo_param_holders.get(BuddyStoneParam::INDEX as usize) else {
        return;
    };
    if holder.get_res_cap(0).is_none() {
        return;
    }

    let mut patched = 0u32;
    let mut samples = Vec::new();
    for (id, row) in repo.rows_mut::<BuddyStoneParam>() {
        let activate = row.activate_range();
        let ret_range = row.overwrite_return_range();
        if activate != MAX_ACTIVATE_RANGE || ret_range != MAX_RETURN_RANGE {
            row.set_activate_range(MAX_ACTIVATE_RANGE);
            row.set_overwrite_return_range(MAX_RETURN_RANGE);
            patched += 1;
            if samples.len() < 5 {
                samples.push(id);
            }
        }
    }

    if patched == 0 {
        return;
    }

    if !PARAM_PATCH_REPORTED.swap(true, Ordering::Relaxed) {
        log(format!(
            "spirit_summon: patched {patched} BuddyStoneParam rows; samples: {samples:#x?}"
        ));
    }
}

// ---------------------------------------------------------------------------
// Codecave installation helper
// ---------------------------------------------------------------------------

/// Builds a codecave from `template`, patches in the final jmp-back to
/// `hook_addr + hook_len`, allocates executable memory near `hook_addr`,
/// writes the cave, and finally patches `hook_addr` with a relative jmp into
/// the cave.
unsafe fn install_codecave(
    name: &str,
    hook_addr: u64,
    hook_len: usize,
    template: &[u8],
    return_jmp_offset: usize,
) -> bool {
    let cave_size = template.len();
    let Some(cave_addr) = (unsafe { memory::alloc_executable(hook_addr, cave_size) }) else {
        log(format!("{name}: ERROR: failed to allocate codecave"));
        return false;
    };

    let mut cave = template.to_vec();
    let return_target = hook_addr + hook_len as u64;
    let return_disp = return_target.wrapping_sub(cave_addr + return_jmp_offset as u64 + 5) as i32;
    cave[return_jmp_offset + 1..return_jmp_offset + 5]
        .copy_from_slice(&return_disp.to_le_bytes());

    unsafe { std::ptr::copy_nonoverlapping(cave.as_ptr(), cave_addr as *mut u8, cave_size) };

    let cave_delta = cave_addr.wrapping_sub(hook_addr + 5) as i64;
    if cave_delta < i32::MIN as i64 || cave_delta > i32::MAX as i64 {
        log(format!("{name}: ERROR: codecave too far from hook site"));
        return false;
    }
    let cave_disp = cave_delta as u32;
    let mut patch = vec![0x90u8; hook_len];
    patch[0] = 0xE9;
    patch[1..5].copy_from_slice(&cave_disp.to_le_bytes());

    if !unsafe { memory::patch_bytes(hook_addr, &patch) } {
        log(format!("{name}: ERROR: failed to patch hook site"));
        return false;
    }

    log(format!(
        "{name}: installed codecave at {cave_addr:#x} (hook at {hook_addr:#x})"
    ));
    true
}

pub static SPIRIT_SUMMON_INSTALLER: Once = Once::new();

/// Applies both SummonAnywhere-style hooks. Gated on the config so disabling
/// the feature leaves the game untouched (requires restart to take effect).
pub fn install() {
    if !config::with_feature(|c| c.spirit_summon_everywhere) {
        return;
    }

    // Hook A: replace the range load with the SummonAnywhere codecave.
    let Some(range_addr) = scan::scan_pattern(RANGE_CHECK_PATTERN) else {
        log("spirit_summon: ERROR: range-check signature not found");
        return;
    };
    if !unsafe {
        install_codecave(
            "spirit_summon_range",
            range_addr,
            RANGE_HOOK_LEN,
            RANGE_CAVE_TEMPLATE,
            RANGE_RETURN_JMP_OFFSET,
        )
    } {
        return;
    }

    // Hook B: clear the area-eligibility struct with the SummonAnywhere codecave.
    let Some(eligibility_addr) = scan::scan_pattern(AREA_ELIGIBILITY_PATTERN) else {
        log("spirit_summon: ERROR: area-eligibility signature not found");
        return;
    };
    if !unsafe {
        install_codecave(
            "spirit_summon_eligibility",
            eligibility_addr,
            AREA_HOOK_LEN,
            AREA_CAVE_TEMPLATE,
            AREA_RETURN_JMP_OFFSET,
        )
    } {
        return;
    }

    log("spirit_summon: both hooks installed");
}
