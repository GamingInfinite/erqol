use std::sync::Once;

use windows::Win32::System::Memory::{
    VirtualProtect, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
};

use crate::{log, scan};

/// The map-open guard. Match starts at a `74 ??` (`je`) that, when the map is
/// *not* allowed (i.e. in combat), falls through into the "map blocked" code.
/// Rewriting the `74` to `EB` (`jmp`) skips that branch so the map always
/// opens. Signature taken from Erd-Tools-CPP (ErdHook.cpp).
const OPEN_MAP_GUARD_PATTERN: &str =
    "74 ? C7 45 ? ? ? ? ? C7 45 ? ? ? ? ? C7 45 ? ? ? ? ? 48 8D 05 ? ? ? ? 48 89 45 ? 48 8D 4D ? E8 ? ? ? ? E9 ? ? ? ? 48 83 BF";

/// A call site that decides "close the map because combat started". The call is
/// replaced with `xor eax, eax; nop; nop` so the caller always sees 0 and never
/// closes the map. Signature taken from Erd-Tools-CPP (ErdHook.cpp).
const CLOSE_MAP_CALL_PATTERN: &str =
    "E8 ? ? ? ? 84 C0 75 ? 38 83 ? ? ? ? 75 ? 83 E6 FE";

/// Replacement for the close-map call site: `xor eax, eax; nop; nop`.
const CLOSE_MAP_PATCH: [u8; 5] = [0x48, 0x31, 0xC0, 0x90, 0x90];

/// Makes the page containing `addr` writable, writes `bytes`, then restores the
/// original protection. Mirrors the code-page write pattern ilhook uses
/// internally.
unsafe fn write_code(addr: u64, bytes: &[u8]) -> bool {
    let mut old_prot = PAGE_PROTECTION_FLAGS(0);
    if unsafe {
        VirtualProtect(
            addr as *const core::ffi::c_void,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old_prot,
        )
    }
    .is_err()
    {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
    }
    let _ = unsafe {
        VirtualProtect(addr as *const core::ffi::c_void, bytes.len(), old_prot, &mut old_prot)
    };
    true
}

pub static MAP_IN_COMBAT_INSTALLER: Once = Once::new();

/// Applies both patches from Erd-Tools' `enable_map_in_combat`: opens the map
/// during combat and stops it from auto-closing when combat starts.
pub fn install() {
    let guard = scan::scan_pattern(OPEN_MAP_GUARD_PATTERN)
        .expect("open-map-in-combat guard signature not found");
    log::log(format!("map_in_combat: open-map guard at {guard:#x}"));

    let close_map = scan::scan_pattern(CLOSE_MAP_CALL_PATTERN)
        .expect("close-map-in-combat call signature not found");
    log::log(format!("map_in_combat: close-map call at {close_map:#x}"));

    if !unsafe { write_code(guard, &[0xEB]) } {
        log::log("map_in_combat: ERROR: failed to patch open-map guard");
        return;
    }
    if !unsafe { write_code(close_map, &CLOSE_MAP_PATCH) } {
        log::log("map_in_combat: ERROR: failed to patch close-map call");
        return;
    }
    log::log("map_in_combat: enabled");
}
