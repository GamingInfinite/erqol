use std::sync::atomic::{AtomicBool, Ordering};

use eldenring::cs::{MapDefaultInfoParam, SoloParam, SoloParamRepository};
use fromsoftware_shared::FromStatic;

use crate::config::DUNGEON_WARP_ENABLED;
use crate::log::log;

/// Event flag `6001`, defined in the ER event flag table as the "always ON"
/// flag (常時ONにしておくイベントフラグ). Areas gate fast travel behind a
/// per-area `EnableFastTravelEventFlagId`: when that flag is non-zero and not
/// set, Sites of Grace are shown crossed out. Pointing every area at an
/// always-set flag unlocks fast travel everywhere, which is exactly what
/// Convergence's regulation.bin does (it sets `6001` for all 82 dungeon rows).
const ALWAYS_ON_FAST_TRAVEL_FLAG: u32 = 6001;

/// Area ID prefixes that belong to small/interior dungeons. Vanilla leaves the
/// fast-travel flag at 0 for some of these (Divine Towers, Shunning-Grounds,
/// Ruin-Strewn Precipice), but they still block fast travel just like catacombs
/// and caves. Convergence's regulation.bin also forces these rows to 6001.
const DUNGEON_AREA_PREFIXES: &[u32] = &[
    30, // catacombs / hero graves
    31, // caves
    32, // tunnels
    34, // divine towers (includes Sealed Tunnel / Divine Tower of West Altus)
    35, // Subterranean Shunning-Grounds
    39, // Ruin-Strewn Precipice
    40, // Shadow of the Erdtree dungeons
    41, // Shadow of the Erdtree dungeons
    42, // Shadow of the Erdtree dungeons
    43, // Shadow of the Erdtree dungeons
];

/// Number of area IDs to include in the log as examples.
const SAMPLE_IDS: usize = 5;

/// Set once the first successful patch has been reported in the log.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// Returns true for area IDs that belong to small/interior dungeons whose
/// MapDefaultInfoParam row should always allow fast travel.
fn is_dungeon_area(area_id: u32) -> bool {
    DUNGEON_AREA_PREFIXES.contains(&(area_id / 1_000_000))
}

/// Rewrites `MapDefaultInfoParam.EnableFastTravelEventFlagId` to the always-ON
/// flag for every area that currently gates fast travel behind a per-area
/// event flag (catacombs, caves, tunnels, hero's graves, ...).
///
/// This is called every frame from the recurring FrameBegin task. It is
/// idempotent and cheap, so it tolerates the game (re)loading `regulation.bin`
/// into [`SoloParamRepository`] after startup: if the repository is not
/// populated yet the call is a no-op and the next frame retries, and if a later
/// reg load resets the flags they are fixed again within a frame.
pub fn patch() {
    if !DUNGEON_WARP_ENABLED.load(Ordering::Relaxed) {
        return;
    }

    let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else {
        return;
    };

    // Params are not loaded until the game parses `regulation.bin`, which can
    // happen after the first frames. Calling `rows_mut` before this param's
    // holder has a res cap would panic (an `expect` inside fromsoftware-rs'
    // `get_param_file`), so bail out and retry next frame until the data exists.
    let Some(holder) = repo
        .solo_param_holders
        .get(MapDefaultInfoParam::INDEX as usize)
    else {
        return;
    };
    if holder.get_res_cap(0).is_none() {
        return;
    }

    let mut unlocked = 0u32;
    let mut samples = Vec::new();
    for (area_id, row) in repo.rows_mut::<MapDefaultInfoParam>() {
        let flag = row.enable_fast_travel_event_flag_id();
        // Patch rows that gate fast travel behind a per-area event flag, plus
        // dungeon rows that vanilla leaves at 0 but still restrict fast travel.
        if flag != ALWAYS_ON_FAST_TRAVEL_FLAG && (flag != 0 || is_dungeon_area(area_id)) {
            row.set_enable_fast_travel_event_flag_id(ALWAYS_ON_FAST_TRAVEL_FLAG);
            unlocked += 1;
            if samples.len() < SAMPLE_IDS {
                samples.push(area_id);
            }
        }
    }

    if unlocked == 0 {
        return;
    }

    if !REPORTED.swap(true, Ordering::Relaxed) {
        log(format!(
            "dungeon_warp: unlocked {unlocked} maps by setting fast-travel flag to {ALWAYS_ON_FAST_TRAVEL_FLAG}; samples: {samples:#x?}"
        ));
    }
}
