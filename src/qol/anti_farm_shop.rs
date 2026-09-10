//! Adds an "Anti-Farm QoL Shop" option to the site-of-grace menu. Selecting it
//! opens the regular shop UI selling items the player would otherwise have to
//! farm (Gold Firefly, Four-Toed Fowl Foot, Flight Pinion, Aeonian Butterfly)
//! for runes.
//!
//! Like Glorious Merchant, the shop rows live entirely in this mod's memory:
//! `SoloParamRepositoryImp::LookupShopLineupParamParamInRange` and `LookupShopLineupParam` are hooked
//! to serve mod-owned lineup rows for a reserved id range (9500000+), which
//! vanilla, Convergence, Glorious Merchant and the transmog mod all leave
//! untouched.
//!
//! The lineup is filtered lazily at first use against the live `ShopLineupParam`
//! table: any item a loaded mod (e.g. Elden Ring Reforged) already sells in
//! infinite quantity through a real shop row is dropped, so the mod shop only
//! keeps items that are farm-only or sold in limited stock.

use std::ptr;
use std::sync::LazyLock;
use std::sync::OnceLock;

use eldenring::cs::{ShopLineupParam, SoloParam, SoloParamRepository};
use eldenring::param::SHOP_LINEUP_PARAM;
use fromsoftware_shared::FromStatic;
use ilhook::x64::Registers;

use crate::config;
use crate::hooks;
use crate::ezstate_menu::{
    alloc_message_id, Event, OPEN_REGULAR_SHOP, SHOP_MENU_CLOSED_EXPR, Span, State, StateGroup,
    Transition, make_int_expression, register_message, register_message_in_bnd, register_patcher,
    splice_option,
};
use crate::log::log;
use crate::scan;

/// First mod-owned shop lineup id. The shop covers
/// `[SHOP_ID, SHOP_ID + lineups.len())`.
const SHOP_ID: i32 = 9_500_000;

/// Grace menu row for the option. Index 70 is used by Elden Ring Reforged
/// ("Reforged options"), so ERQoL uses 73 to avoid overlapping.
const OPTION_INDEX: i32 = 73;
static MSG_ANTI_FARM_SHOP: LazyLock<i32> = LazyLock::new(alloc_message_id);

/// Shop title message, served from the menu text bound.
static MSG_SHOP_TITLE: LazyLock<i32> = LazyLock::new(alloc_message_id);
const MSGBND_MENU_TEXT: u32 = 200;

const GOLD_FIREFLY: (i32, i32) = (20811, 500);
const FOUR_TOED_FOWL_FOOT: (i32, i32) = (15080, 1000);
const FLIGHT_PINION: (i32, i32) = (15060, 1000);
const AEONIAN_BUTTERFLY: (i32, i32) = (20801, 500);

/// Candidate items in shop order. Any candidate the live game already sells in
/// infinite quantity through a `ShopLineupParam` row is filtered out before the
/// shop is built.
const CANDIDATES: [(i32, i32); 4] = [
    GOLD_FIREFLY,
    FOUR_TOED_FOWL_FOOT,
    FLIGHT_PINION,
    AEONIAN_BUTTERFLY,
];

/// Equips goods like Kalé's wares.
const EQUIP_TYPE_GOODS: u8 = 3;

// AOB signatures from Glorious Merchant / elden-ring-transmog, verified
// against the current eldenring.exe. Note that pelite treats each `?`
// character as one wildcard byte. `e8 $ '` resolves the call target.
const LOOKUP_SHOP_MENU_PATTERN: &str = "? 8b 4e 14 ? 8b 46 10 33 d2 48 8d 4d ? e8 $ '";
const LOOKUP_SHOP_LINEUP_PATTERN: &str =
    "48 8d 15 ? ? ? ? 45 33 c0 ? ? ? e8 ? ? ? ? 48 85 c0 74 ?";
/// Distance from the LookupShopLineupParam pattern match to the function start.
const LOOKUP_SHOP_LINEUP_OFFSET: usize = 129;

/// `SoloParamRepositoryImp::FindShopMenuResult` / `FindShopLineupResult`:
/// `{u8 shop_type; i32 id; const SHOP_LINEUP_PARAM *row}`.
#[repr(C)]
struct ShopResult {
    shop_type: u8,
    id: i32,
    row: *const SHOP_LINEUP_PARAM,
}

static LINEUPS: OnceLock<&'static [SHOP_LINEUP_PARAM]> = OnceLock::new();

/// True if the live game's `ShopLineupParam` already has a goods row selling
/// `equip_id` in infinite quantity. Finite-stock rows (e.g. an item a merchant
/// sells only a handful of) do NOT count, so the anti-farm shop still covers
/// those. Returns false (keeping the item) whenever the param table can't be
/// read yet, so a too-early call degrades to the full lineup instead of an
/// empty shop.
fn sold_by_game_shop(equip_id: i32) -> bool {
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        log("anti_farm_shop: SoloParamRepository unavailable; assuming not sold");
        return false;
    };
    let Some(holder) = repo.solo_param_holders.get(ShopLineupParam::INDEX as usize) else {
        log("anti_farm_shop: ShopLineupParam holder missing; assuming not sold");
        return false;
    };
    if holder.get_res_cap(0).is_none() {
        log("anti_farm_shop: ShopLineupParam not loaded; assuming not sold");
        return false;
    }

    for (row_id, row) in repo.rows::<ShopLineupParam>() {
        if row.equip_id() == equip_id
            && row.equip_type() == EQUIP_TYPE_GOODS
            && row.sell_quantity() == -1
        {
            log(&format!(
                "anti_farm_shop: ShopLineupParam row {row_id} sells {equip_id} with infinite quantity"
            ));
            return true;
        }
    }
    false
}

/// Builds the shop lineup once, dropping any candidate a loaded mod already
/// sells in infinite quantity. Falls back to the full list if every candidate
/// is sold infinitely by the game (menu stays functional, just redundant).
fn lineups() -> &'static [SHOP_LINEUP_PARAM] {
    LINEUPS.get_or_init(|| {
        let kept: Vec<SHOP_LINEUP_PARAM> = CANDIDATES
            .iter()
            .filter(|(equip_id, _)| {
                if sold_by_game_shop(*equip_id) {
                    log(&format!(
                        "anti_farm_shop: skipping {equip_id}: already sold infinitely by a game shop"
                    ));
                    return false;
                }
                true
            })
            .map(|&(equip_id, price)| make_lineup(equip_id, price))
            .collect();

        let shop = if kept.is_empty() {
            log("anti_farm_shop: all candidates sold by game shops; serving full lineup");
            CANDIDATES
                .iter()
                .map(|&(equip_id, price)| make_lineup(equip_id, price))
                .collect::<Vec<_>>()
        } else {
            kept
        };
        log(&format!(
            "anti_farm_shop: lineup built with {} of {} candidates",
            shop.len(),
            CANDIDATES.len()
        ));
        Box::leak(shop.into_boxed_slice())
    })
}

fn make_lineup(equip_id: i32, price: i32) -> SHOP_LINEUP_PARAM {
    let mut row: SHOP_LINEUP_PARAM = unsafe { std::mem::zeroed() };
    row.set_equip_id(equip_id);
    row.set_value(price);
    row.set_mtrl_id(-1);
    row.set_sell_quantity(-1);
    row.set_equip_type(EQUIP_TYPE_GOODS);
    row.set_set_num(1);
    row.set_value_magnification(1.0);
    row.set_icon_id(-1);
    row.set_name_msg_id(-1);
    row.set_menu_title_msg_id(*MSG_SHOP_TITLE);
    row.set_menu_icon_id(-1);
    row
}

// ---- Shop lookup hooks ----

/// Hook for `SoloParamRepositoryImp::LookupShopLineupParamParamInRange(result, shop_type, begin_id, end_id)`.
/// Returns our shop menu row when the requested range starts at our shop id.
fn lookup_shop_menu_detour(regs: *mut Registers, original: usize) -> usize {
    let result = unsafe { (*regs).rcx } as *mut ShopResult;
    let shop_type = unsafe { (*regs).rdx } as u8;
    let begin_id = unsafe { (*regs).r8 } as i32;
    let end_id = unsafe { (*regs).r9 } as i32;

    if begin_id == SHOP_ID {
        log(&format!(
            "anti_farm_shop: LookupShopLineupParamParamInRange called shop_type={shop_type} begin_id={begin_id} end_id={end_id}"
        ));
        unsafe {
            (*result).shop_type = shop_type;
            (*result).id = begin_id;
            (*result).row = lineups().as_ptr();
        }
        log(&format!(
            "anti_farm_shop: LookupShopLineupParamParamInRange served shop menu for begin_id={begin_id}"
        ));
        return result as usize;
    }

    let original_fn: extern "C" fn(*mut ShopResult, u8, i32, i32) -> *mut ShopResult =
        unsafe { std::mem::transmute(original) };
    original_fn(result, shop_type, begin_id, end_id) as usize
}

/// Hook for `SoloParamRepositoryImp::LookupShopLineupParam(result, shop_type, id)`.
/// Returns our lineup rows for ids in `[SHOP_ID, SHOP_ID + lineups.len())`;
/// anything else falls through to the vanilla lookup.
fn lookup_shop_lineup_detour(regs: *mut Registers, original: usize) -> usize {
    let result = unsafe { (*regs).rcx } as *mut ShopResult;
    let shop_type = unsafe { (*regs).rdx } as u8;
    let id = unsafe { (*regs).r8 } as i32;

    let in_range = id >= SHOP_ID && id < SHOP_ID + 10;
    if in_range {
        log(&format!(
            "anti_farm_shop: LookupShopLineupParam called shop_type={shop_type} id={id}"
        ));
    }

    let lineups = lineups();
    if id >= SHOP_ID && (id - SHOP_ID) < lineups.len() as i32 {
        unsafe {
            (*result).shop_type = shop_type;
            (*result).id = id;
            (*result).row = lineups.as_ptr().add((id - SHOP_ID) as usize);
        }
        log(&format!("anti_farm_shop: LookupShopLineupParam served lineup id={id}"));
        return result as usize;
    }

    let original_fn: extern "C" fn(*mut ShopResult, u8, i32) -> *mut ShopResult =
        unsafe { std::mem::transmute(original) };
    original_fn(result, shop_type, id) as usize
}

fn install_lookup_shop_menu_hook() {
    let Some(target) = scan::scan_pattern_call(LOOKUP_SHOP_MENU_PATTERN) else {
        log("anti_farm_shop: ERROR: LookupShopLineupParamParamInRange signature not found");
        return;
    };
    log(&format!("anti_farm_shop: LookupShopLineupParamParamInRange at {target:#x}"));
    hooks::install_retn("anti_farm_shop: LookupShopLineupParamParamInRange", target, lookup_shop_menu_detour);
}

fn install_lookup_shop_lineup_hook() {
    let Some(match_addr) = scan::scan_pattern(LOOKUP_SHOP_LINEUP_PATTERN) else {
        log("anti_farm_shop: ERROR: LookupShopLineupParam signature not found");
        return;
    };
    let target = match_addr - LOOKUP_SHOP_LINEUP_OFFSET as u64;
    log(&format!("anti_farm_shop: LookupShopLineupParam at {target:#x}"));
    hooks::install_retn("anti_farm_shop: LookupShopLineupParam", target, lookup_shop_lineup_detour);
}

// ---- The grace menu option ----

/// State that opens the regular shop for our lineup range and returns to the
/// grace menu once the shop closes. Mirrors the transmog mod's
/// `open_regular_shop_state`.
struct ShopState {
    begin_expr: [u8; 6],
    end_expr: [u8; 6],
    args: [Span<u8>; 2],
    open_event: Event,
    closed_expr: [u8; 15],
    return_transition: Transition,
    transition_arr: [*mut Transition; 1],
    state: State,
}

impl ShopState {
    fn new(return_state: *mut State) -> Self {
        Self {
            begin_expr: make_int_expression(SHOP_ID),
            end_expr: make_int_expression(SHOP_ID + (lineups().len() as i32) - 1),
            args: [Span::null(); 2],
            open_event: Event {
                command: OPEN_REGULAR_SHOP,
                args: Span::null(),
            },
            closed_expr: SHOP_MENU_CLOSED_EXPR,
            return_transition: Transition {
                target_state: return_state,
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            transition_arr: [ptr::null_mut()],
            state: State {
                id: 0,
                transitions: Span::null(),
                entry_events: Span::null(),
                exit_events: Span::null(),
                while_events: Span::null(),
            },
        }
    }

    /// Fixes up the self-referential spans now that the state lives at a
    /// stable (leaked heap) address.
    fn link(&mut self) {
        self.args = [
            Span {
                ptr: self.begin_expr.as_mut_ptr(),
                len: self.begin_expr.len(),
            },
            Span {
                ptr: self.end_expr.as_mut_ptr(),
                len: self.end_expr.len(),
            },
        ];
        self.open_event.args = Span {
            ptr: self.args.as_mut_ptr(),
            len: self.args.len(),
        };
        self.return_transition.evaluator = Span {
            ptr: self.closed_expr.as_mut_ptr(),
            len: self.closed_expr.len(),
        };
        self.transition_arr = [&mut self.return_transition as *mut Transition];
        self.state.transitions = Span {
            ptr: self.transition_arr.as_mut_ptr(),
            len: self.transition_arr.len(),
        };
        self.state.entry_events = Span {
            ptr: &mut self.open_event as *mut Event,
            len: 1,
        };
    }
}

/// Builds and leaks the shop state; returns a pointer to it.
unsafe fn make_shop_state(return_state: *mut State) -> *mut State {
    let shop = Box::into_raw(Box::new(ShopState::new(return_state)));
    unsafe { (*shop).link() };
    (unsafe { &mut (*shop).state }) as *mut State
}

/// Grace menu patcher: adds the shop option row and its dispatch transition.
unsafe fn patch_grace(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.anti_farm_shop) {
        return false;
    }

    let return_state = unsafe { (*state_group).initial_state };
    if return_state.is_null() {
        log("anti_farm_shop: patch_grace: initial_state is null");
        return false;
    }
    let ok = unsafe {
        splice_option(
            state_group,
            OPTION_INDEX,
            *MSG_ANTI_FARM_SHOP,
            make_shop_state(return_state),
        )
    };
    log(&format!(
        "anti_farm_shop: patch_grace: splice_option index={OPTION_INDEX} ok={ok}"
    ));
    ok
}

// ---- Installer ----

pub(crate) fn init() {
    register_message(*MSG_ANTI_FARM_SHOP, "Anti-Farm QoL Shop");
    register_message_in_bnd(MSGBND_MENU_TEXT, *MSG_SHOP_TITLE, "Anti-Farm QoL Shop");
    register_patcher(patch_grace);
    install_lookup_shop_menu_hook();
    install_lookup_shop_lineup_hook();
}
