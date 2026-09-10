use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};

use eldenring::cs::{EquipParamGoods, GameDataMan, SoloParamRepository};
use fromsoftware_shared::FromStatic;

use crate::config;
use crate::ezstate_menu::{
    alloc_message_id, event_arg_int, register_message, register_patcher, slice_of, splice_alt_option,
    ADD_TALK_LIST_DATA_ALT, State, StateGroup, SubMenu, SubMenuAction,
};
use crate::log::log;

// ---- Message ids ----
//
// Allocated from the shared allocator so they never collide with another module.

static MSG_CONSUME_ALL_RUNES: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CONSUME_GOLDEN_RUNES: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CONSUME_REMEMBRANCES: LazyLock<i32> = LazyLock::new(alloc_message_id);
const MSG_CANCEL: i32 = 69_990_003;
const OPTION_INDEX: i32 = 69;

// ---- Rune sources ----

/// Known param IDs for every rune-source goods item. The runes granted per
/// item are always read live from the game's `EquipParamGoods` param
/// (`sellValue`), so value changes from other mods are respected. The IDs
/// themselves come from the player's own `regulation.bin` (v1.16.1 / DLC02).
const RUNE_IDS: &[u32] = &[
    // Golden Rune [1-13]
    2900, 2901, 2902, 2903, 2904, 2905, 2906, 2907, 2908, 2909, 2910, 2911, 2912,
    // Numen's Rune, Hero's Rune [1-5], Lord's Rune
    2913, 2914, 2915, 2916, 2917, 2918, 2919,
    // Lands Between Rune
    2990,
    // DLC: Leda's Rune, Broken Rune, Shadow Realm Rune [1-7]
    2002950, 2002951, 2002952, 2002953, 2002954, 2002955, 2002956, 2002957, 2002958,
    // Rune of an Unsung Hero, Marika's Rune
    2002959, 2002960,
];

fn is_rune(param_id: u32) -> bool {
    RUNE_IDS.contains(&param_id)
}

/// Known param IDs for every remembrance goods item (boss memories). The runes
/// granted per item are always read live from the game's `EquipParamGoods`
/// param (`sellValue`), same as rune items. IDs come from the player's own
/// `regulation.bin` (v1.16.1 / DLC02).
const REMEMBRANCE_IDS: &[u32] = &[
    // Base game
    2950, 2951, 2952, 2953, 2954, 2955, 2956, 2957, 2958, 2959, 2960, 2961, 2962, 2963, 2964,
    // DLC
    2002900, 2002901, 2002902, 2002903, 2002904, 2002905, 2002907, 2002908, 2002909, 2002910,
];

fn is_remembrance(param_id: u32) -> bool {
    REMEMBRANCE_IDS.contains(&param_id)
}

/// Runes granted by a single item, read live from the game's param so mods
/// that retune `sellValue` are respected.
fn rune_value(param_id: u32) -> u64 {
    if let Ok(repo) = unsafe { SoloParamRepository::instance() }
        && let Some(row) = repo.get::<EquipParamGoods>(param_id)
    {
        return row.sell_value().max(0) as u64;
    }
    0
}

// ---- Actions ----

/// Runs on the game's main thread when the player picks a consume option. Sums
/// the value of every matching stack into the rune count and zeroes the
/// consumed stacks.
unsafe fn consume_matching(is_target: impl Fn(u32) -> bool, label: &str) {
    let Some(game_data_man) = GameDataMan::instance_ptr().ok() else {
        log("consume_all_runes: GameDataMan unavailable");
        return;
    };
    if game_data_man.is_null() {
        log("consume_all_runes: GameDataMan is null");
        return;
    }
    let pgd_ptr = unsafe { (*game_data_man).main_player_game_data.as_ptr() };
    if pgd_ptr.is_null() {
        log("consume_all_runes: player game data is null");
        return;
    }

    let mut total_value: u64 = 0;
    let mut consumed_count: u64 = 0;

    {
        let pgd = unsafe { &*pgd_ptr };
        let items_data = &pgd.equipment.equip_inventory_data.items_data;
        for entry in items_data.items() {
            let id = entry.item_id.param_id();
            if is_target(id) {
                total_value += entry.quantity as u64 * rune_value(id);
                consumed_count += entry.quantity as u64;
            }
        }
    }

    if consumed_count == 0 {
        log(format!("consume_all_runes: no {label}s in inventory"));
        return;
    }

    let pgd = unsafe { &mut *pgd_ptr };
    pgd.rune_count = pgd.rune_count.saturating_add(total_value as u32);

    let items_data = &pgd.equipment.equip_inventory_data.items_data;
    for entry in items_data.items_mut() {
        if is_target(entry.item_id.param_id()) {
            entry.quantity = 0;
        }
    }

    log(format!(
        "consume_all_runes: consumed {consumed_count} {label} items for {total_value} runes"
    ));
}

/// Runs on the game's main thread when the player picks "Consume Golden Runes".
unsafe extern "C" fn consume_golden_runes_action() {
    unsafe { consume_matching(is_rune, "rune") };
}

/// Runs on the game's main thread when the player picks "Consume Remembrances".
unsafe extern "C" fn consume_remembrances_action() {
    unsafe { consume_matching(is_remembrance, "remembrance") };
}

// ---- Talk-list indicators ----
//
// Each of the three rows we add carries a writable indicator literal (`0x40+v,
// 0xa1`); the per-frame `tick` drives the glow: the two consume buttons light
// when an item of the matching kind sits in the inventory, and the top-level
// row forwards that state (either lit).
//
// Metadata is re-derived from the live ESD structures on every patch instead of
// being cached in Rust: the top row's indicator address comes straight back
// from `splice_alt_option`, and the submenu rows are located by scanning the
// leaked submenu state's entry events.

/// Indicator literal byte addresses `[top, golden, remembrance]`. Zero = not
/// patched into the live group yet (never write through a zero address).
static WRITER_ADDRS: LazyLock<Mutex<[usize; 3]>> = LazyLock::new(|| Mutex::new([0usize; 3]));

/// Cached last-written indicator mask (`bit0 = rune, bit1 = remembrance,
/// bit2 = top`) to avoid pointless memory writes every frame.
static LAST_MASK: AtomicU8 = AtomicU8::new(0);

fn store_writers(addrs: [usize; 3]) {
    if let Ok(mut writers) = WRITER_ADDRS.lock() {
        *writers = addrs;
    }
}

/// True when any inventory item matches `is_target` with a nonzero stack.
fn has_matching(is_target: impl Fn(u32) -> bool) -> bool {
    let Ok(game_data_man) = GameDataMan::instance_ptr() else {
        return false;
    };
    if game_data_man.is_null() {
        return false;
    }
    let pgd_ptr = unsafe { (*game_data_man).main_player_game_data.as_ptr() };
    if pgd_ptr.is_null() {
        return false;
    }
    let pgd = unsafe { &*pgd_ptr };
    let items_data = &pgd.equipment.equip_inventory_data.items_data;
    items_data
        .items()
        .any(|entry| entry.quantity > 0 && is_target(entry.item_id.param_id()))
}

/// Scans one state's entry events for the given ALT row and returns the
/// address of its indicator value byte (0 if not found).
unsafe fn find_row_indicator(state: *mut State, message_id: i32) -> usize {
    for event in unsafe { slice_of((*state).entry_events) } {
        if event.command != ADD_TALK_LIST_DATA_ALT {
            continue;
        }
        if unsafe { event_arg_int(event, 2) } != message_id {
            continue;
        }
        if event.args.ptr.is_null() || event.args.len <= 5 {
            continue;
        }
        let indicator_expr = unsafe { *event.args.ptr.add(5) };
        if indicator_expr.ptr.is_null() {
            continue;
        }
        return indicator_expr.ptr as usize;
    }
    0
}

/// True when the grace group already carries our top-level row (i.e. this live
/// group was patched on an earlier grace open). Lets us skip re-splicing while
/// keeping the writer addresses from the original splice valid.
unsafe fn group_has_alt_row(state_group: *mut StateGroup, message_id: i32) -> bool {
    for state in unsafe { slice_of((*state_group).states) } {
        let state_ptr = state as *const State as *mut State;
        if unsafe { find_row_indicator(state_ptr, message_id) } != 0 {
            return true;
        }
    }
    false
}

// ---- Feature wiring ----

/// Registers this feature's message texts and patch routine. Called once,
/// before `ezstate_menu::install()`.
pub(crate) fn init() {
    register_patcher(patch);
    register_message(*MSG_CONSUME_ALL_RUNES, "Consume all Runes");
    register_message(*MSG_CONSUME_GOLDEN_RUNES, "Consume Golden Runes");
    register_message(*MSG_CONSUME_REMEMBRANCES, "Consume Remembrances");
    register_message(MSG_CANCEL, "Cancel");
}

/// Adds the "Consume all Runes" option to a grace menu state group along with
/// the submenu it opens. Each row carries a talk-list indicator driving the
/// button glow; the submenu indicators light when matching items are in the
/// inventory, and the top-level row forwards that state.
pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.consume_all_runes) {
        return false;
    }

    unsafe {
        let already = group_has_alt_row(state_group, *MSG_CONSUME_ALL_RUNES);
        if already {
            return false;
        }

        let initial_state = (*state_group).initial_state;
        let has_rune = has_matching(is_rune);
        let has_remembrance = has_matching(is_remembrance);

        let submenu_state = SubMenu::link_from_rows_self_return_with_indicators(
            &[
                (
                    1,
                    *MSG_CONSUME_GOLDEN_RUNES,
                    false,
                    Some(consume_golden_runes_action as SubMenuAction),
                ),
                (
                    2,
                    *MSG_CONSUME_REMEMBRANCES,
                    false,
                    Some(consume_remembrances_action as SubMenuAction),
                ),
                (99, MSG_CANCEL, true, None),
            ],
            &[has_rune as i32, has_remembrance as i32],
            initial_state,
        );

        let golden_addr = find_row_indicator(submenu_state, *MSG_CONSUME_GOLDEN_RUNES);
        let remembrance_addr = find_row_indicator(submenu_state, *MSG_CONSUME_REMEMBRANCES);

        let top_addr = splice_alt_option(
            state_group,
            OPTION_INDEX,
            *MSG_CONSUME_ALL_RUNES,
            submenu_state,
            (has_rune || has_remembrance) as i32,
        );
        let Some(top_addr) = top_addr else {
            return false;
        };

        store_writers([top_addr as usize, golden_addr, remembrance_addr]);
        true
    }
}

/// Per-frame upkeep: writes the current indicator values into the three
/// installed rows. Called by the recurring frame task.
pub(crate) fn tick() {
    let addrs = {
        let Ok(writers) = WRITER_ADDRS.lock() else {
            return;
        };
        *writers
    };
    if addrs[0] == 0 && addrs[1] == 0 && addrs[2] == 0 {
        return;
    }

    let can_rune = has_matching(is_rune);
    let can_remembrance = has_matching(is_remembrance);
    let mask = (can_rune as u8)
        | (can_remembrance as u8) << 1
        | ((can_rune || can_remembrance) as u8) << 2;
    if mask == LAST_MASK.swap(mask, Ordering::Relaxed) {
        return;
    }

    unsafe {
        let top_on = (can_rune || can_remembrance) as u8;
        (addrs[0] as *mut u8).write(0x40 + top_on);
        (addrs[1] as *mut u8).write(0x40 + can_rune as u8);
        (addrs[2] as *mut u8).write(0x40 + can_remembrance as u8);
    }
}
