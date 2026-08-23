//! Serves the SpEffectParam rows added by AuraFarmingPostures from mod-owned
//! memory, replacing that mod's `regulation.bin` edits.
//!
//! `SoloParamRepositoryImp::GetSpEffectParam(result, id)` is the central
//! lookup every speffect consumer goes through (apply, damage calc, network
//! sync). It binary-searches the loaded regulation and returns a null row for
//! unknown ids — which is exactly what happens today for the mod's 671
//! posture effect ids (6805000+), making [`crate::postures::effects`] a
//! no-op. The hook below intercepts those ids and serves verbatim row blobs
//! extracted from the mod's regulation.bin; everything else falls through to
//! vanilla. Like the anti-farm shop lineups, the rows live entirely in this
//! DLL's memory and no vanilla row is touched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};

use ilhook::x64::Registers;

use crate::hooks;
use crate::log::log;
use crate::scan;
use crate::postures::speffect_ids::SPEFFECT_IDS;

/// Raw SP_EFFECT_PARAM_ST row size in the 1.16 regulation.
const ROW_SIZE: usize = 912;

/// GetSpEffectParam itself: prologue shared with sibling param getters, made
/// unique by the trailing `lea edx,[r8+0xF]` (the SpEffectParam solo-param
/// type id passed to GetParamResCap). Verified unique against 1.16.2.
const GET_SP_EFFECT_PARAM_PATTERN: &str = "41 56 48 83 EC 40 48 C7 44 24 ? FE FF FF FF \
     48 89 5C 24 ? 48 89 6C 24 ? 48 89 74 24 ? 48 89 7C 24 ? 8B FA 4C 8B F1 33 DB 40 32 F6 \
     85 D2 0F 88 ? ? ? ? 48 8B 0D ? ? ? ? 48 85 C9 75 ? 48 8D 0D ? ? ? ? E8 ? ? ? ? \
     4C 8B C8 4C 8D 05 ? ? ? ? BA B4 00 00 00 48 8D 0D ? ? ? ? E8 ? ? ? ? \
     48 8B 0D ? ? ? ? 45 33 C0 41 8D 50 0F";

/// `SoloParamRepositoryImp`'s lookup result:
/// `{ const SP_EFFECT_PARAM_ST *row; u32 param_id; u8 data_version; }`.
#[repr(C)]
struct SpEffectLookupResult {
    row: *const SpeffectRow,
    param_id: u32,
    _pad: u32,
    data_version: u8,
}

/// One opaque param row; contents are copied verbatim from the mod's
/// regulation so field layout matches whatever the game expects.
#[repr(C, align(16))]
pub struct SpeffectRow([u8; ROW_SIZE]);

static ROWS: OnceLock<&'static [SpeffectRow]> = OnceLock::new();
static REPORTED: AtomicBool = AtomicBool::new(false);
pub(crate) static SPEFFECTS_INSTALLER: Once = Once::new();

fn rows() -> &'static [SpeffectRow] {
    ROWS.get_or_init(|| {
        let blob = include_bytes!("speffect_rows.bin");
        debug_assert_eq!(blob.len(), SPEFFECT_IDS.len() * ROW_SIZE);
        let mut out = Vec::with_capacity(blob.len() / ROW_SIZE);
        for chunk in blob.chunks_exact(ROW_SIZE) {
            let mut row: SpeffectRow = unsafe { std::mem::zeroed() };
            row.0.copy_from_slice(chunk);
            out.push(row);
        }
        Box::leak(out.into_boxed_slice())
    })
}

/// Index into [`SPEFFECT_IDS`]/[`rows`] for `id`, if it is one of ours.
fn find_index(id: i32) -> Option<usize> {
    let id = u32::try_from(id).ok()?;
    SPEFFECT_IDS.binary_search(&id).ok()
}

type GetSpEffectParamFn =
    extern "C" fn(*mut SpEffectLookupResult, u32) -> *mut SpEffectLookupResult;

fn get_sp_effect_param_detour(regs: *mut Registers, original: usize) -> usize {
    let result = unsafe { (*regs).rcx } as *mut SpEffectLookupResult;
    let id = unsafe { (*regs).rdx } as u32;
    let original_fn: GetSpEffectParamFn = unsafe { std::mem::transmute(original) };

    if let Some(index) = find_index(id as i32) {
        // The original fills param_id + data_version (and row=null, since our
        // ids don't exist in vanilla regulations); we then swap in our row.
        original_fn(result, id);
        unsafe { (*result).row = &rows()[index] };
        if !REPORTED.swap(true, Ordering::Relaxed) {
            log(format!(
                "postures_speffects: served first posture speffect id={id}"
            ));
        }
        return result as usize;
    }

    original_fn(result, id) as usize
}

pub(crate) fn install() {
    let Some(target) = scan::scan_pattern(GET_SP_EFFECT_PARAM_PATTERN) else {
        log("postures_speffects: ERROR: GetSpEffectParam signature not found");
        return;
    };
    log(format!(
        "postures_speffects: GetSpEffectParam at {target:#x}; serving {} rows",
        SPEFFECT_IDS.len()
    ));
    hooks::install_retn(
        "postures_speffects: GetSpEffectParam",
        target,
        get_sp_effect_param_detour,
    );
}
