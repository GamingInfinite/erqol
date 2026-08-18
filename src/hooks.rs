//! Shared inline-hook installation helpers.

use ilhook::x64::{
    hook_closure_jmp_back, hook_closure_retn, CallbackOption, HookFlags, Registers,
};

use crate::log::log;

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
