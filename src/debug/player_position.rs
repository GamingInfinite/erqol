//! Debug helper: logs the player's current world-map position to a JSON file
//! so custom `WorldMapPointParam` rows can be recorded from inside the game.
//!
//! The WorldMapPoint row fields (`areaNo`/`gridXNo`/`gridZNo`) are the
//! `mAA_BB_CC_DD` map-id components — i.e. exactly the player's `BlockId`
//! bitfield: `area`=AA -> `area_no`, `block`=BB -> `grid_x_no`, `region`=CC ->
//! `grid_z_no`. The position (`posX/posZ`, `posY` is unused by the game) is
//! written down alongside the raw diagnostic positions so the exact mapping
//! (block-relative vs global chunk) can be confirmed empirically and then baked
//! into the `CustomPoint` list.
//!
//! Enabled when `map_icon_points = true` in `erqol_settings.toml`. Standing
//! somewhere and pressing the hotkey (F9) appends one JSON record to
//! `erqol_points.json` next to the DLL. The `note` field is auto-generated as
//! `"{label} {n}"`, where `label` comes from the `map_icon_point_label` config
//! key and `n` is the number of points recorded with that label during the
//! current session (one DLL load / one game run).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once, OnceLock};

use eldenring::cs::{BlockId, PlayerIns};
use eldenring::position::BlockPosition;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::log::{self, log};

/// Hotkey that records the current point. F9 (VK_F9 = 0x78).
const RECORD_KEY: i32 = 0x78;

#[derive(Serialize, Deserialize)]
struct Record {
    note: String,
    area_no: u8,
    grid_x_no: u8,
    grid_z_no: u8,
    pos: [f32; 3],
    block_id: String,
    block_relative_pos: [f32; 3],
    chunk_pos: [f32; 3],
}

/// Per-session counter of how many points have been recorded with the current
/// label, so each record's note reads `"{label} {n}"`.
static SESSION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Cached output file path (needs the DLL path, resolved lazily on first tick).
static OUTPUT_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Serialize the writes so two ticks can't interleave a line.
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn output_path() -> Option<PathBuf> {
    OUTPUT_PATH
        .get_or_init(|| log::dll_parent().map(|dir| dir.join("erqol_points.json")))
        .clone()
}

/// Maps a `BlockId` to the `area_no/grid_x_no/grid_z_no` triple used by
/// `WorldMapPointParam` rows (the `mAA_BB_CC_DD` components).
fn block_to_grid(id: BlockId) -> (u8, u8, u8) {
    (id.area(), id.block(), id.region())
}

/// Clears the debug record file once per mod launch. The debug
/// `erqol_points.json` is just a recording scratchpad — it never survives
/// between launches. The base set of icons is the embedded one baked into the
/// DLL (see `map_icons_points.json`), which is what actually ships.
///
/// The file is truncated *in place* (not deleted) so it stays continuously
/// existing on disk like `erqol.log`, letting editors see the live contents.
static CLEARED: Once = Once::new();

fn clear_records() {
    let Some(path) = output_path() else {
        log("player_position: could not resolve DLL path for erqol_points.json");
        return;
    };
    CLEARED.call_once(|| {
        let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Open with create+truncate so the file always exists (empty JSON
        // array); `record_point` later reads it back and appends.
        match fs::write(&path, "[\n]\n") {
            Ok(()) => log(&format!(
                "player_position: cleared debug records ({})",
                path.display()
            )),
            Err(e) => log(&format!(
                "player_position: could not clear {}: {e}",
                path.display()
            )),
        }
    });
}

/// Appends one JSON record for the player's current position. Returns whether a
/// record was written (false when the player isn't loaded / config disabled).
pub fn record_point() -> bool {
    if !config::with_feature(|c| c.map_icon_points) {
        return false;
    }

    let Ok(player) = (unsafe { PlayerIns::local_player() }) else {
        log("player_position: no local player");
        return false;
    };

    let label = config::config().lock().unwrap_or_else(|e| e.into_inner()).map_icon_point_label.clone();
    let n = SESSION_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let note = format!("{label} {n}");

    let block_id = player.current_block_id;
    let (area_no, grid_x_no, grid_z_no) = block_to_grid(block_id);

    // Priority candidate for the WMP `posX/posZ`: the position within the
    // current block. Captured verbatim alongside the global chunk position so
    // the true mapping can be confirmed and then hardcoded into the rows.
    let bp: &BlockPosition = &player.block_position;
    let chunk = player.chr_ins.chunk_position;

    let record = Record {
        note,
        area_no,
        grid_x_no,
        grid_z_no,
        pos: [bp.x, bp.y, bp.z],
        block_id: block_id.to_string(),
        block_relative_pos: [bp.x, bp.y, bp.z],
        chunk_pos: [chunk.0, chunk.1, chunk.2],
    };

    let Some(path) = output_path() else {
        log("player_position: cannot resolve DLL path for erqol_points.json");
        return false;
    };

    let _guard = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Read back the existing records (a valid JSON array, possibly hand-edited),
    // append this one, and rewrite the whole file as a valid array so the
    // result always has outer `[...]` and commas between entries.
    let mut all: Vec<Record> = match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).unwrap_or_else(|e| {
                log(&format!(
                    "player_position: could not parse existing {} ({e}); starting fresh",
                    path.display()
                ));
                Vec::new()
            })
        }
        _ => Vec::new(),
    };
    all.push(record);

    // Pretty-print (2-space indent) so each record is readable and hand-editable.
    let json = match serde_json::to_string_pretty(&all) {
        Ok(json) => json,
        Err(e) => {
            log(&format!(
                "player_position: JSON serialisation failed: {e}"
            ));
            return false;
        }
    };

    match fs::write(&path, json) {
        Ok(()) => {
            let written_note = all.last().map(|r| r.note.as_str()).unwrap_or("");
            log(&format!(
                "player_position: recorded {} at {} ({area_no},{grid_x_no},{grid_z_no}) pos=({:.2},{:.2},{:.2})",
                written_note, block_id, bp.x, bp.y, bp.z
            ))
        }
        Err(e) => log(&format!(
            "player_position: failed to write {}: {e}",
            path.display()
        )),
    }
    true
}

/// Called every frame from the recurring `FrameBegin` task. Debounce (250ms)
/// is handled by `input::is_key_pressed`, so holding the key won't spam.
pub fn tick() {
    if !config::with_feature(|c| c.map_icon_points) {
        return;
    }
    clear_records();
    if eldenring::util::input::is_key_pressed(RECORD_KEY) {
        let _ = record_point();
    }
}
