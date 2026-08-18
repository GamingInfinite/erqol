use std::sync::Once;

use ilhook::x64::Registers;

use crate::config;
use crate::hooks;
use crate::scan;

const AUTO_PICKUP_ITEM_IDS: &[i32] = &[
    1000, 4000, 4110, 4200, 4201, 4202, 4250, 4251, 4252, 4253, 4260, 4270, 4280, 4300, 4350,
    6361, 9532, 4100, 7800, 7810, 7811, 7812, 7813, 7814, 7815, 7816, 7817, 7818, 7819, 7820,
    7821, 7822, 7823, 7824, 7825, 7826, 7827, 7828, 7850, 7860, 7861, 7862, 7863, 7864, 7865,
    7866, 7867, 7868, 7869, 7870, 7871, 7872, 7873, 7874, 7875, 7876, 7877, 7878, 207800,
    207810, 207811, 207812, 207813, 207814, 207815, 207816, 207817, 207818, 207819, 207820,
    207821, 207822, 207823, 207824, 207825, 207826, 207827, 207828, 207829, 207830, 207831,
    207832, 207833, 207834, 207835, 207836, 207837, 207838, 207839, 207840, 207841, 207842,
    207843, 207844,
];

const PROXY_PATTERN: &str =
    "48 89 5C 24 08 57 48 81 EC 90 00 00 00 48 8B 84 24 E0 00 00 00 41 0F B6 D9 48 8B 0D ? ? ? ? 8B FA 0F 29 B4 24 80 00 00 00";

fn find_execute_action_button_proxy() -> Option<u64> {
    scan::scan_pattern(PROXY_PATTERN)
}

fn is_auto_pickup_id(entry_id: i32) -> bool {
    AUTO_PICKUP_ITEM_IDS.binary_search(&entry_id).is_ok()
}

pub static AUTO_PICKUP_INSTALLER: Once = Once::new();

fn auto_pickup_detour(regs: *mut Registers, original: usize) -> usize {
    {
        let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        if !cfg.auto_pickup {
            return call_original_proxy(regs, original);
        }
    }
    let entry_id = unsafe { (*regs).rdx } as i32;
    if is_auto_pickup_id(entry_id) {
        return 1;
    }
    call_original_proxy(regs, original)
}

fn call_original_proxy(regs: *mut Registers, original: usize) -> usize {
    // Pass through ALL register params and stack params from the original
    // call. The proxy function reads r9b (4th param) and [rsp+0x48] (9th
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
    let proxy_addr = find_execute_action_button_proxy()
        .expect("ExecuteActionButtonParamProxy not found");
    hooks::install_retn("auto_pickup: ExecuteActionButtonParamProxy", proxy_addr, auto_pickup_detour);
}
