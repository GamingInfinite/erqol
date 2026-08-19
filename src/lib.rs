use std::time::Duration;

use eldenring::cs::CSTaskGroupIndex;
use eldenring::cs::CSTaskImp;
use eldenring::fd4::FD4TaskData;
use fromsoftware_shared::SharedTaskImpExt;
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};

mod anti_farm_shop;
mod auto_pickup;
mod config;
mod consume_all_runes;
mod dungeon_warp;
mod ezstate_menu;
mod grace_settings;
mod heavy_door;
mod hooks;
mod log;
mod map_in_combat;
mod memory;
mod merchant_bell_bearing;
mod no_time_on_death;
mod roundtable_at_home;
mod scan;
mod skip_flask_confirm;

// ---- Crash logging (VEH) ----

fn hex_words(words: &[u64]) -> String {
    words.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>().join(" ")
}

/// Dumps the ESD machine context around a crash: the machine's current state,
/// that state's transition array, and the first transition's fields. All reads
/// go through [`safe_read_u64`]. `machine` is taken from the crash register
/// `rdx` (the convention this codebase's machine functions use).
fn dump_machine_context(machine: usize) {
    if machine == 0 {
        return;
    }
    let Some(state) = memory::safe_read_u64(machine + 0x20) else {
        log::crash_log(&format!("VEH:   machine={machine:#x} state=<unreadable>"));
        return;
    };
    let state_group = memory::safe_read_u64(machine + 0x28);
    log::crash_log(&format!(
        "VEH:   machine={machine:#x} state={state:#x} state_group={:#x}",
        state_group.unwrap_or(0)
    ));

    let state_addr = state as usize;
    let Some(tptr) = memory::safe_read_u64(state_addr + 0x08) else {
        log::crash_log("VEH:   state->transitions <unreadable>");
        return;
    };
    let tlen = memory::safe_read_u64(state_addr + 0x10).unwrap_or(0) as usize;
    log::crash_log(&format!("VEH:   state->transitions ptr={tptr:#x} len={tlen}"));

    let mut entries: Vec<u64> = Vec::new();
    for i in 0..tlen.min(4) {
        match memory::safe_read_u64(tptr as usize + i * 8) {
            Some(v) => entries.push(v),
            None => {
                entries.push(0);
                log::crash_log(&format!("VEH:   transitions[{i}] <unreadable>"));
            }
        }
    }
    log::crash_log(&format!("VEH:   transitions[0..{}] = {}", entries.len(), hex_words(&entries)));

    for (i, &t) in entries.iter().enumerate() {
        if t == 0 {
            continue;
        }
        let t = t as usize;
        let target = memory::safe_read_u64(t).unwrap_or(0);
        let sub_ptr = memory::safe_read_u64(t + 0x18).unwrap_or(0);
        let sub_len = memory::safe_read_u64(t + 0x20).unwrap_or(0);
        let eval_ptr = memory::safe_read_u64(t + 0x28).unwrap_or(0);
        let eval_len = memory::safe_read_u64(t + 0x30).unwrap_or(0);
        log::crash_log(&format!(
            "VEH:   t[{i}]={t:#x} target={target:#x} sub=[{sub_ptr:#x};{sub_len}] eval=[{eval_ptr:#x};{eval_len}]"
        ));
    }
}

/// Installed once from `DllMain`. Any unhandled exception (crash) is logged to
/// `logs/crash.log` with the exception code, the faulting instruction address,
/// the faulting data address, the register context and a stack trace, then
/// passed on to the normal handler so the crash behaviour is unchanged.
unsafe extern "system" fn on_exception(exception_info: *mut EXCEPTION_POINTERS) -> i32 {
    if exception_info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = unsafe { (*exception_info).ExceptionRecord };
    let context = unsafe { (*exception_info).ContextRecord };
    if record.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = unsafe { (*record).ExceptionCode }.0 as u32;
    let address = unsafe { (*record).ExceptionAddress };
    let params = unsafe { (*record).NumberParameters };

    // Benign debugger/CRT exceptions (OutputDebugString prologue etc.) flood the
    // VEH on this target; skip them entirely.
    if code == 0x406d1388 || code == 0x4001000a || code == 0x40010006 {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    let is_av = code == 0xc0000005;
    if !is_av {
        log::crash_log(&format!("VEH: exception code={code:#x} at {address:p} nparams={params}"));
        return EXCEPTION_CONTINUE_SEARCH;
    }

    // Access violation — log everything.
    log::crash_log(&format!("VEH: AV code={code:#x} at {address:p} nparams={params}"));
    if params >= 2 {
        let op = unsafe { (*record).ExceptionInformation[0] };
        let target = unsafe { (*record).ExceptionInformation[1] };
        log::crash_log(&format!("VEH:   AV op={op:#x} accessed={target:#x}"));
    }
    if !context.is_null() {
        let ctx = unsafe { &*context };
        log::crash_log(&format!(
            "VEH:   rax={:#x} rcx={:#x} rdx={:#x} rbx={:#x}",
            ctx.Rax, ctx.Rcx, ctx.Rdx, ctx.Rbx
        ));
        log::crash_log(&format!(
            "VEH:   rsp={:#x} rbp={:#x} rsi={:#x} rdi={:#x} rip={:#x}",
            ctx.Rsp, ctx.Rbp, ctx.Rsi, ctx.Rdi, ctx.Rip
        ));
        log::crash_log(&format!(
            "VEH:   r8={:#x} r9={:#x} r10={:#x} r11={:#x}",
            ctx.R8, ctx.R9, ctx.R10, ctx.R11
        ));
        log::crash_log(&format!(
            "VEH:   r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
            ctx.R12, ctx.R13, ctx.R14, ctx.R15
        ));
        let stack = unsafe { std::slice::from_raw_parts(ctx.Rsp as *const u64, 20) };
        log::crash_log(&format!("VEH:   stack[0..20] = {}", hex_words(stack)));
        dump_machine_context(ctx.Rdx as usize);
    }
    EXCEPTION_CONTINUE_SEARCH
}

fn install_veh() {
    let handler: unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> i32 = on_exception;
    unsafe {
        AddVectoredExceptionHandler(1, Some(handler));
    }
    log::log("VEH: exception handler installed");
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(hmodule: usize, reason: u32) -> bool {
    if reason != 1 {
        return true;
    }

    log::init_dll_path(hmodule);
    install_veh();

    std::thread::spawn(|| {
        config::load();
        config::apply_to_runtime();

        let cs_task = CSTaskImp::wait_for_instance(Duration::MAX).unwrap();
        cs_task.run_recurring(
            |_: &FD4TaskData| {
                auto_pickup::AUTO_PICKUP_INSTALLER.call_once(auto_pickup::install_auto_pickup_hook);
                map_in_combat::MAP_IN_COMBAT_INSTALLER.call_once(map_in_combat::install);
                heavy_door::HEAVY_DOOR_INSTALLER.call_once(heavy_door::install);
                no_time_on_death::NO_TIME_ON_DEATH_INSTALLER.call_once(no_time_on_death::install);
                dungeon_warp::patch();
                ezstate_menu::MENU_INSTALLER.call_once(|| {
                    anti_farm_shop::init();
                    consume_all_runes::init();
                    skip_flask_confirm::init();
                    merchant_bell_bearing::init();
                    roundtable_at_home::init();
                    grace_settings::init();
                    ezstate_menu::install();
                });
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });

    true
}
