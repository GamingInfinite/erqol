use std::sync::OnceLock;

use eldenring::cs::{EquipParamGoods, GameDataMan, SoloParamRepository};
use fromsoftware_shared::FromStatic;

use crate::ezstate_menu::{
    register_message, register_patcher, splice_option, StateGroup, SubMenu, SubMenuAction,
};
use crate::log::log;

// ---- Message constants ----

const MSG_CONSUME_ALL_RUNES: i32 = 69000000;
const MSG_CONSUME_GOLDEN_RUNES: i32 = 69000001;
const MSG_CONSUME_REMEMBRANCES: i32 = 69000002;
const MSG_CANCEL: i32 = 69000003;
const OPTION_INDEX: i32 = 69;

static CONSUME_ALL_RUNES_TEXT: OnceLock<Vec<u16>> = OnceLock::new();
static GOLDEN_RUNES_TEXT: OnceLock<Vec<u16>> = OnceLock::new();
static REMEMBRANCES_TEXT: OnceLock<Vec<u16>> = OnceLock::new();
static CANCEL_TEXT: OnceLock<Vec<u16>> = OnceLock::new();

/// Returns a leaked, NUL-terminated UTF-16 copy of `text`.
fn static_utf16(buffer: &'static OnceLock<Vec<u16>>, text: &str) -> &'static [u16] {
    buffer
        .get_or_init(|| {
            let mut chars: Vec<u16> = text.encode_utf16().collect();
            chars.push(0);
            chars
        })
        .as_slice()
}

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

// ---- Feature wiring ----

/// Registers this feature's message texts and patch routine. Called once,
/// before `ezstate_menu::install()`.
pub(crate) fn init() {
    register_patcher(patch);
    register_message(
        MSG_CONSUME_ALL_RUNES,
        static_utf16(&CONSUME_ALL_RUNES_TEXT, "Consume all Runes"),
    );
    register_message(
        MSG_CONSUME_GOLDEN_RUNES,
        static_utf16(&GOLDEN_RUNES_TEXT, "Consume Golden Runes"),
    );
    register_message(
        MSG_CONSUME_REMEMBRANCES,
        static_utf16(&REMEMBRANCES_TEXT, "Consume Remembrances"),
    );
    register_message(MSG_CANCEL, static_utf16(&CANCEL_TEXT, "Cancel"));
}

/// Adds the "Consume all Runes" option to a grace menu state group along with
/// the submenu it opens. Safe to call on every grace menu open; the library's
/// already-patched check makes it a no-op afterwards.
pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    unsafe {
        let initial_state = (*state_group).initial_state;

        let submenu = Box::into_raw(Box::new(SubMenu::new(&[
            (
                1,
                MSG_CONSUME_GOLDEN_RUNES,
                false,
                Some(consume_golden_runes_action as SubMenuAction),
            ),
            (2, MSG_CONSUME_REMEMBRANCES, false, Some(consume_remembrances_action as SubMenuAction)),
            (99, MSG_CANCEL, true, None),
        ])));
        let submenu_state = SubMenu::link(submenu, initial_state);

        splice_option(
            state_group,
            OPTION_INDEX,
            MSG_CONSUME_ALL_RUNES,
            submenu_state,
        )
    }
}
