//! Havok Script (c0000.hks) integration.
//!
//! The game loads every HKS chunk through Havok's
//! `hkbInternal::hksi_luaL_loadbuffer(lua_State, buff, size, chunkname)`
//! (`FUN_1414e2bb0` in 1.16.2, identified by its own assert-string reference).
//! When the requested chunk is `action/script/c0000.hks` we serve the embedded
//! merged posture script instead of whatever bytes the loader found — this
//! replaces the mod's shipped `c0000.hks` file with DLL-owned content. All
//! other chunks pass through untouched (with light diagnostic logging).

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

use ilhook::x64::Registers;

use crate::config;
use crate::hooks;
use crate::log::log;
use crate::scan;

/// `hksi_luaL_loadbuffer` prologue. The trailing `41 B9 72 B4 6A 12`
/// (assert code constant) separates it from a byte-identical sibling wrapper.
/// Verified unique against 1.16.2.
const HKSI_LUAL_LOADBUFFER_PATTERN: &str = "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 \
     57 48 83 EC 30 49 8B F9 49 8B F0 48 8B EA 48 8B D9 48 85 C9 75 ? \
     E8 ? ? ? ? 48 8D 0D ? ? ? ? 41 B9 72 B4 6A 12";

type HksiLuaLLoadbufferFn =
    extern "C" fn(lua_state: usize, buff: *const u8, size: usize, chunkname: *const u8) -> i32;

/// The merged posture script (decompiled vanilla body + appended standalone
/// shim + the five integration call-sites), byte-identical to the file the
/// original mod ships. Served for every `action/script/c0000.hks` load.
static SCRIPT_OVERRIDE: &[u8] = include_bytes!("c0000_postures.hks");

pub(crate) static HKS_INSTALLER: Once = Once::new();

/// True when the chunkname is `.../action/script/c0000.hks` (ASCII
/// case-insensitive suffix match; the game lowercases paths, but be safe).
fn is_c0000_chunk(name_ptr: usize) -> bool {
    const MAX: usize = 300;
    if name_ptr == 0 {
        return false;
    }
    let bytes = unsafe { std::slice::from_raw_parts(name_ptr as *const u8, MAX) };
    let Some(nul) = bytes.iter().position(|&b| b == 0) else {
        return false;
    };
    const NEEDLE: &[u8] = b"/action/script/c0000.hks";
    nul >= NEEDLE.len()
        && bytes[nul - NEEDLE.len()..nul].eq_ignore_ascii_case(NEEDLE)
}

/// Sizes already logged (dedup so hot small chunks don't flood the log).
static SEEN: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
/// Total calls seen; lets us confirm the hook is live even after dedup stops.
static TOTAL_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Hard cap on log lines emitted by the probe.
static LOGGED_LINES: AtomicUsize = AtomicUsize::new(0);

const MAX_LOGGED_LINES: usize = 600;
const MAX_SEEN_SIZES: usize = 512;
/// Chunks at least this large are always logged (c0000.hks is ~1.2 MB).
const ALWAYS_LOG_SIZE: usize = 1_000_000;

fn read_cstr(ptr: usize, max: usize) -> String {
    if ptr == 0 {
        return "<null>".to_string();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, max) };
    match bytes.iter().position(|&b| b == 0) {
        Some(len) => String::from_utf8_lossy(&bytes[..len]).into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// `"bc"` for compiled Lua bytecode (`1b 4c 75 61`), `"src"` otherwise.
fn buffer_kind(buff: *const u8, size: usize) -> &'static str {
    if buff.is_null() || size < 4 {
        return "???";
    }
    if unsafe { buff.read() } == 0x1B
        && unsafe { buff.add(1).read() } == b'L'
        && unsafe { buff.add(2).read() } == b'u'
        && unsafe { buff.add(3).read() } == b'a'
    {
        "bc"
    } else {
        "src"
    }
}

fn probe_log(lua_state: usize, buff: *const u8, size: usize, name_ptr: usize) {
    if buff.is_null() || size == 0 {
        return;
    }
    let calls = TOTAL_CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    let big = size >= ALWAYS_LOG_SIZE;
    if !big {
        let mut slot = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        match slot.as_mut() {
            None => *slot = Some(HashSet::new()),
            Some(seen) => {
                if seen.len() >= MAX_SEEN_SIZES || !seen.insert(size) {
                    return;
                }
            }
        }
    }
    if LOGGED_LINES.load(Ordering::Relaxed) >= MAX_LOGGED_LINES {
        return;
    }
    LOGGED_LINES.fetch_add(1, Ordering::Relaxed);
    let name = read_cstr(name_ptr, 160);
    log(format!(
        "hks_loadbuffer: #{calls} state={lua_state:#x} size={size} kind={} name='{name}'",
        buffer_kind(buff, size)
    ));
}

/// Total c0000 loads served from our copy.
static SWAPS: AtomicUsize = AtomicUsize::new(0);
static SWAP_REPORTED: AtomicBool = AtomicBool::new(false);

fn loadbuffer_detour(regs: *mut Registers, original: usize) -> usize {
    let lua_state = unsafe { (*regs).rcx } as usize;
    let buff = unsafe { (*regs).rdx } as *const u8;
    let size = unsafe { (*regs).r8 } as usize;
    let name = unsafe { (*regs).r9 } as usize;

    let original_fn: HksiLuaLLoadbufferFn = unsafe { std::mem::transmute(original) };

    if is_c0000_chunk(name) {
        let swaps = SWAPS.fetch_add(1, Ordering::Relaxed) + 1;
        if !SWAP_REPORTED.swap(true, Ordering::Relaxed) {
            log(format!(
                "postures_hks: serving embedded posture script ({} bytes) to state={lua_state:#x} \
                 (loader had size={size}, kind={}, swap #{swaps})",
                SCRIPT_OVERRIDE.len(),
                buffer_kind(buff, size),
            ));
        }
        return original_fn(
            lua_state,
            SCRIPT_OVERRIDE.as_ptr(),
            SCRIPT_OVERRIDE.len(),
            name as *const u8,
        ) as usize;
    }

    probe_log(lua_state, buff, size, name);

    original_fn(lua_state, buff, size, name as *const u8) as usize
}

pub(crate) fn install() {
    if !config::with_feature(|cfg| cfg.postures_hks_inject) {
        return;
    }
    let matches = scan::scan_pattern_all(HKSI_LUAL_LOADBUFFER_PATTERN, 4);
    if matches.len() != 1 {
        log(format!(
            "postures_hks: ERROR: hksi_luaL_loadbuffer signature matched {} times ({matches:?})",
            matches.len()
        ));
        return;
    }
    let target = matches[0];
    log(format!("postures_hks: hksi_luaL_loadbuffer at {target:#x}"));
    hooks::install_retn(
        "postures_hks: hksi_luaL_loadbuffer",
        target,
        loadbuffer_detour,
    );
}
