use std::time::Duration;

use eldenring::cs::CSTaskGroupIndex;
use eldenring::cs::CSTaskImp;
use eldenring::fd4::FD4TaskData;
use fromsoftware_shared::SharedTaskImpExt;

mod auto_pickup;
mod consume_all_runes;
mod ezstate_menu;
mod log;
mod scan;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn DllMain(hmodule: usize, reason: u32) -> bool {
    if reason != 1 {
        return true;
    }

    log::init_dll_path(hmodule);

    std::thread::spawn(|| {
        let cs_task = CSTaskImp::wait_for_instance(Duration::MAX).unwrap();
        cs_task.run_recurring(
            |_: &FD4TaskData| {
                auto_pickup::AUTO_PICKUP_INSTALLER.call_once(auto_pickup::install_auto_pickup_hook);
                ezstate_menu::MENU_INSTALLER.call_once(|| {
                    consume_all_runes::init();
                    ezstate_menu::install();
                });
            },
            CSTaskGroupIndex::FrameBegin,
        );
    });

    true
}
