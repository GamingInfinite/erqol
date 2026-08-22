use std::sync::Once;

use ilhook::x64::Registers;

use crate::hooks;
use crate::log::log;
use crate::memory;
use crate::scan;

/// Diagnostic hook at the bank-2007 message dispatch. When a
/// DisplayGenericDialog (id 1) instruction is about to run, we read and log its
/// args without modifying anything yet.
const HEAVY_DOOR_MESSAGE_ID: i32 = 4200;
const DISPATCH_PATTERN: &str = "49 8B 80 D0 00 00 00 8B 48 04 FF C9 83 F9 0F";
const DISPATCH_HOOK_OFFSET: usize = 12; // after `FF C9`
const GET_EVENT_ARGS_FROM_EMEVD_FILE_ADDR: u64 = 0x140CE0C00;

pub static HEAVY_DOOR_INSTALLER: Once = Once::new();

pub fn install() {
    let Some(match_va) = scan::scan_pattern(DISPATCH_PATTERN) else {
        log("heavy_door: ERROR: bank 2007 dispatch signature not found");
        return;
    };
    let target = match_va + DISPATCH_HOOK_OFFSET as u64;
    log(&format!(
        "heavy_door: bank 2007 dispatch hook at {target:#x} (pattern {match_va:#x})"
    ));

    hooks::install_jmp_back("heavy_door: bank 2007 dispatch", target, heavy_door_detour);
}

fn heavy_door_detour(regs: *mut Registers) {
    unsafe {
        let rax = (*regs).rax;
        let r8 = (*regs).r8;
        // After `dec ecx`, rcx holds (EMEVD command id - 1).
        let internal_id = (*regs).rcx as u32;

        // Only intercept DisplayGenericDialog (id 1).
        if internal_id != 0 {
            return;
        }

        // Resolve args pointer. Try the cached pointer first; if null use the
        // same helper the handlers call.
        let mut args_ptr = memory::read_qword(r8 + 0xD8);
        if args_ptr == 0 {
            let context = memory::read_qword(r8 + 0xC8);
            let args_offset = memory::read_qword(rax + 16);
            type ComputeArgs = extern "C" fn(u64, u64) -> u64;
            let compute: ComputeArgs = std::mem::transmute(GET_EVENT_ARGS_FROM_EMEVD_FILE_ADDR);
            args_ptr = compute(context, args_offset);
        }

        if args_ptr == 0 {
            return;
        }

        let message_id = memory::read_dword(args_ptr) as i32;
        if message_id == HEAVY_DOOR_MESSAGE_ID {
            // Rewrite the internal command id from 1 to 4 (DisplayBlinkingMessage).
            (*regs).rcx = 3;
            log(&format!(
                "heavy_door: redirected message {message_id} to DisplayBlinkingMessage"
            ));
        }
    }
}


