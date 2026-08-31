//! Shared inline-hook installation helpers.

use std::sync::OnceLock;

use ilhook::x64::{
    hook_closure_jmp_back, hook_closure_retn, CallbackOption, HookFlags, Registers,
};

use crate::log::log;
use crate::memory;

/// Type of a `hook_closure_retn` callback.
pub type RetnFn = fn(*mut Registers, usize) -> usize;

/// Type of a `hook_closure_jmp_back` callback.
pub type JmpBackFn = fn(*mut Registers);

/// Installs a function-replacement hook at `address` using `hook_closure_retn`.
/// Logs success/failure and leaks the handle so the hook stays active.
pub fn install_retn(name: &'static str, address: u64, detour: RetnFn) {
    match unsafe {
        hook_closure_retn(
            address as usize,
            detour,
            CallbackOption::None,
            HookFlags::empty(),
        )
    } {
        Ok(handle) => {
            std::mem::forget(handle);
            log(format!("{name}: hooked"));
        }
        Err(err) => log(format!("{name}: ERROR: failed to hook: {err:?}")),
    }
}

/// Installs a mid-function jump-back hook at `address` using
/// `hook_closure_jmp_back`. Logs success/failure and leaks the handle.
pub fn install_jmp_back(name: &'static str, address: u64, detour: JmpBackFn) {
    match unsafe {
        hook_closure_jmp_back(
            address as usize,
            detour,
            CallbackOption::None,
            HookFlags::empty(),
        )
    } {
        Ok(handle) => {
            std::mem::forget(handle);
            log(format!("{name}: hooked"));
        }
        Err(err) => log(format!("{name}: ERROR: failed to hook: {err:?}")),
    }
}

/// Decodes the destination of an already-installed hook so callers can chain
/// behind it rather than clobber it. Supports the two forms hook libraries use
/// on x64: a 5-byte `E9 rel32` relative jump and a 6-byte `FF 25 disp32`
/// indirect jump (destination cell read from memory).
pub fn existing_hook_target(entry: u64, bytes: &[u8]) -> Option<u64> {
    match bytes[0] {
        0xE9 => {
            let disp = i32::from_le_bytes(bytes[1..5].try_into().ok()?);
            Some(entry.wrapping_add(5).wrapping_add((disp as i64) as u64))
        }
        0xFF if bytes.get(1) == Some(&0x25) => {
            let disp = i32::from_le_bytes(bytes[2..6].try_into().ok()?);
            let cell = entry.wrapping_add(6).wrapping_add((disp as i64) as u64);
            memory::safe_read_u64(cell as usize)
        }
        _ => None,
    }
}

/// Race-safe entry-patch finalisation shared by every hook that must tolerate
/// concurrent fetchers and/or other hooking libraries: allocates a near
/// absolute-jump stub (`FF 25 <abs>` -> `detour`) within ±2 GiB of `entry`,
/// stores `forward_target` (a relocated prologue trampoline or a chained
/// hook's destination) in `forward`, then writes the 5-byte `E9` entry patch
/// with the opcode byte last so no observing thread decodes a half-written
/// branch.
pub fn finalize_e9_entry(
    name: &str,
    entry: u64,
    detour: usize,
    forward_target: usize,
    forward: &OnceLock<usize>,
) -> bool {
    let mut stub = Vec::with_capacity(12);
    stub.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
    stub.extend_from_slice(&(detour as u64).to_le_bytes());
    let Some(stub_addr) = (unsafe { memory::alloc_executable(entry, 12) }) else {
        log(format!("{name}: ERROR: failed to allocate hook stub"));
        return false;
    };
    unsafe {
        std::ptr::copy_nonoverlapping(stub.as_ptr(), stub_addr as *mut u8, stub.len());
    }
    let _ = forward.set(forward_target);
    let delta = stub_addr.wrapping_sub(entry + 5) as i64;
    if delta < i32::MIN as i64 || delta > i32::MAX as i64 {
        log(format!("{name}: ERROR: stub out of range for E9"));
        return false;
    }
    let mut patch = [0x90u8; 5];
    patch[1..5].copy_from_slice(&(delta as i32).to_le_bytes());
    patch[0] = 0xE9;
    if !unsafe { memory::patch_bytes_ordered(entry, &patch, 0) } {
        log(format!("{name}: ERROR: failed to patch entry"));
        return false;
    }
    log(format!(
        "{name}: hooked (race-safe; stub={stub_addr:#x} bytes={stub:02x?} forward={forward_target:#x})"
    ));
    true
}
