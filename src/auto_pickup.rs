use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Once, OnceLock};

use ilhook::x64::Registers;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W, TH32CS_SNAPMODULE,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW};
use windows::core::{w, PCSTR, PCWSTR};

use crate::config;
use crate::hooks;
use crate::log::log;
use crate::scan;

/// Base game action-button IDs that correspond to ground-item / item-lot pickups.
/// When the player walks near one of these, `ExecActionButton` (the proxy at
/// the RVA target) returns 1 to auto-interact.
const BASE_AUTO_PICKUP_ITEM_IDS: &[i32] = &[
    1000, 4000, 4110, 4200, 4201, 4202, 4250, 4251, 4252, 4253, 4260, 4270, 4280, 4300, 4350,
    6361, 9532, 4100, 7800, 7810, 7811, 7812, 7813, 7814, 7815, 7816, 7817, 7818, 7819, 7820,
    7821, 7822, 7823, 7824, 7825, 7826, 7827, 7828, 7850, 7860, 7861, 7862, 7863, 7864, 7865,
    7866, 7867, 7868, 7869, 7870, 7871, 7872, 7873, 7874, 7875, 7876, 7877, 7878, 207800,
    207810, 207811, 207812, 207813, 207814, 207815, 207816, 207817, 207818, 207819, 207820,
    207821, 207822, 207823, 207824, 207825, 207826, 207827, 207828, 207829, 207830, 207831,
    207832, 207833, 207834, 207835, 207836, 207837, 207838, 207839, 207840, 207841, 207842,
    207843, 207844,
];

/// Elden Ring Reforged adds "Rune Pieces" as world collectibles. They use the
/// standard item-pickup action-button category but have their own IDs, so
/// auto-pickup does not collect them unless we add those IDs.
const REFORGED_AUTO_PICKUP_ITEM_IDS: &[i32] = &[7829, 7879];

const EXEC_ACTION_BUTTON_PATTERN: &str =
    "48 89 5C 24 08 57 48 81 EC 90 00 00 00 48 8B 84 24 E0 00 00 00 41 0F B6 D9 48 8B 0D ? ? ? ? 8B FA 0F 29 B4 24 80 00 00 00";

/// Lazily-built sorted ID list. Reforged IDs are appended only when
/// `reforged.dll` is loaded, so vanilla is unaffected.
static AUTO_PICKUP_ITEM_IDS: OnceLock<&'static [i32]> = OnceLock::new();

fn auto_pickup_ids() -> &'static [i32] {
    AUTO_PICKUP_ITEM_IDS.get_or_init(|| {
        let mut ids = BASE_AUTO_PICKUP_ITEM_IDS.to_vec();
        log("auto_pickup: building pickup ID list");
        if is_reforged_mod_loaded() {
            log("auto_pickup: Reforged detected, adding Rune Piece pickup IDs");
            ids.extend_from_slice(REFORGED_AUTO_PICKUP_ITEM_IDS);
        } else {
            log("auto_pickup: Reforged NOT detected");
        }
        ids.sort_unstable();
        ids.dedup();
        Box::leak(ids.into_boxed_slice())
    })
}

/// True if Elden Ring Reforged's main DLL is loaded by ModEngine. Logs the
/// result of every probe and the matching loaded modules for diagnostics.
fn is_reforged_mod_loaded() -> bool {
    let ansi_names: [&[u8]; 3] = [b"reforged.dll\0", b"Reforged.dll\0", b"REFORGED.DLL\0"];
    for &name in &ansi_names {
        let res = unsafe { GetModuleHandleA(PCSTR(name.as_ptr())) };
        let s = std::str::from_utf8(&name[..name.len() - 1]).unwrap_or("?");
        log(&format!(
            "auto_pickup: GetModuleHandleA({s}) -> ok={}",
            res.is_ok()
        ));
        if res.is_ok() {
            return true;
        }
    }

    let wide = w!("reforged.dll");
    let res = unsafe { GetModuleHandleW(PCWSTR(wide.as_ptr())) };
    log(&format!(
        "auto_pickup: GetModuleHandleW(reforged.dll) -> ok={}",
        res.is_ok()
    ));
    if res.is_ok() {
        return true;
    }

    log_loaded_modules_matching("reforged");
    false
}

/// Logs every loaded module whose name contains `substr` (case-insensitive).
fn log_loaded_modules_matching(substr: &str) {
    let substr_lower = substr.to_lowercase();
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, 0) {
            Ok(s) => s,
            Err(e) => {
                log(&format!("auto_pickup: CreateToolhelp32Snapshot failed: {e:?}"));
                return;
            }
        };

        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;

        if Module32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry
                    .szModule
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szModule.len());
                let module_name = String::from_utf16_lossy(&entry.szModule[..len]);
                if module_name.to_lowercase().contains(&substr_lower) {
                    log(&format!("auto_pickup: loaded module: {module_name}"));
                }
                if Module32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
    }
}

fn find_exec_action_button() -> Option<u64> {
    scan::scan_pattern(EXEC_ACTION_BUTTON_PATTERN)
}

fn is_auto_pickup_id(entry_id: i32) -> bool {
    auto_pickup_ids().binary_search(&entry_id).is_ok()
}

pub static AUTO_PICKUP_INSTALLER: Once = Once::new();

/// Last action-button ID logged by the detour. Used to de-duplicate the log
/// so we only record each newly-seen ID once instead of every frame.
static LAST_LOGGED_ID: AtomicI32 = AtomicI32::new(-1);

fn auto_pickup_detour(regs: *mut Registers, original: usize) -> usize {
    let entry_id = unsafe { (*regs).rdx } as i32;
    let enabled = config::with_feature(|cfg| cfg.auto_pickup);
    let is_reforged_id = REFORGED_AUTO_PICKUP_ITEM_IDS.contains(&entry_id);

    // Log each distinct ID the first time it is seen so we can identify what
    // action-button ID Reforged uses for rune pieces.
    let last = LAST_LOGGED_ID.load(Ordering::Relaxed);
    if entry_id != last {
        LAST_LOGGED_ID.store(entry_id, Ordering::Relaxed);
        log(&format!(
            "auto_pickup: saw entry_id={entry_id}, enabled={enabled}, in_list={}",
            is_auto_pickup_id(entry_id)
        ));
    }

    if is_reforged_id && !is_auto_pickup_id(entry_id) {
        log(&format!(
            "auto_pickup: rune piece ID {entry_id} not in pickup list"
        ));
    }

    if !enabled {
        return call_original(regs, original);
    }
    if is_auto_pickup_id(entry_id) {
        return 1;
    }
    call_original(regs, original)
}

fn call_original(regs: *mut Registers, original: usize) -> usize {
    // Pass through ALL register params and stack params from the original
    // call. ExecActionButton reads r9b (4th param) and [rsp+0x48] (9th
    // param via get_stack(9) from original RSP). Without forwarding all
    // params, the original function gets garbage in R8/R9/stack → crashes.
    let original_fn: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32 =
        unsafe { std::mem::transmute(original) };
    unsafe {
        original_fn(
            (*regs).rcx,
            (*regs).rdx,
            (*regs).r8,
            (*regs).r9,
            (*regs).get_stack(5),
            (*regs).get_stack(6),
            (*regs).get_stack(7),
            (*regs).get_stack(8),
            (*regs).get_stack(9),
        ) as usize
    }
}

pub fn install_auto_pickup_hook() {
    let exec_addr = find_exec_action_button()
        .expect("ExecActionButton not found");
    hooks::install_retn(
        "auto_pickup: ExecActionButton",
        exec_addr,
        auto_pickup_detour,
    );

    // Build the ID list and log detection state immediately so the log always
    // shows whether Reforged was detected, regardless of whether the player
    // has walked near a pickup yet.
    let ids = auto_pickup_ids();
    let enabled = config::with_feature(|cfg| cfg.auto_pickup);
    log(&format!(
        "auto_pickup: installed; enabled={enabled}; total_ids={}; has_reforged_ids={}",
        ids.len(),
        REFORGED_AUTO_PICKUP_ITEM_IDS
            .iter()
            .all(|id| ids.binary_search(id).is_ok())
    ));
}
