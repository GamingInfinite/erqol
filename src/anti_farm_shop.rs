//! Adds an "Anti-Farm QoL Shop" option to the site-of-grace menu. Selecting it
//! opens the regular shop UI selling a Gold Firefly and a Four-Toed Fowl Foot
//! for runes, so the player can skip farming for them.
//!
//! Like Glorious Merchant, the shop rows live entirely in this mod's memory:
//! `SoloParamRepositoryImp::LookupShopMenu` and `LookupShopLineup` are hooked
//! to serve mod-owned lineup rows for a reserved id range (9500000+), which
//! vanilla, Convergence, Glorious Merchant and the transmog mod all leave
//! untouched.

use std::ptr;
use std::sync::OnceLock;

use eldenring::param::SHOP_LINEUP_PARAM;
use ilhook::x64::Registers;

use crate::config;
use crate::hooks;
use crate::ezstate_menu::{
    Event, OPEN_REGULAR_SHOP, SHOP_MENU_CLOSED_EXPR, Span, State, StateGroup, Transition,
    make_int_expression, register_message, register_message_in_bnd, register_patcher,
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
const MSG_ANTI_FARM_SHOP: i32 = 69_990_004;

/// Shop title message, served from the menu text bound.
const MSG_SHOP_TITLE: i32 = 69_990_005;
const MSGBND_MENU_TEXT: u32 = 200;

const GOLD_FIREFLY: (i32, i32) = (20811, 500);
const FOUR_TOED_FOWL_FOOT: (i32, i32) = (15080, 1000);

/// Equips goods like Kalé's wares.
const EQUIP_TYPE_GOODS: u8 = 3;

// AOB signatures from Glorious Merchant / elden-ring-transmog, verified
// against the current eldenring.exe. Note that pelite treats each `?`
// character as one wildcard byte. `e8 $ '` resolves the call target.
const LOOKUP_SHOP_MENU_PATTERN: &str = "? 8b 4e 14 ? 8b 46 10 33 d2 48 8d 4d ? e8 $ '";
const LOOKUP_SHOP_LINEUP_PATTERN: &str =
    "48 8d 15 ? ? ? ? 45 33 c0 ? ? ? e8 ? ? ? ? 48 85 c0 74 ?";
/// Distance from the LookupShopLineup pattern match to the function start.
const LOOKUP_SHOP_LINEUP_OFFSET: usize = 129;

/// `SoloParamRepositoryImp::FindShopMenuResult` / `FindShopLineupResult`:
/// `{u8 shop_type; i32 id; const SHOP_LINEUP_PARAM *row}`.
#[repr(C)]
struct ShopResult {
    shop_type: u8,
    id: i32,
    row: *const SHOP_LINEUP_PARAM,
}

static LINEUPS: OnceLock<&'static [SHOP_LINEUP_PARAM; 2]> = OnceLock::new();

fn lineups() -> &'static [SHOP_LINEUP_PARAM; 2] {
    LINEUPS.get_or_init(|| {
        let rows = [
            make_lineup(GOLD_FIREFLY.0, GOLD_FIREFLY.1),
            make_lineup(FOUR_TOED_FOWL_FOOT.0, FOUR_TOED_FOWL_FOOT.1),
        ];
        Box::leak(Box::new(rows))
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
    row.set_menu_title_msg_id(MSG_SHOP_TITLE);
    row.set_menu_icon_id(-1);
    row
}

// ---- Shop lookup hooks ----

/// Hook for `SoloParamRepositoryImp::LookupShopMenu(result, shop_type, begin_id, end_id)`.
/// Returns our shop menu row when the requested range starts at our shop id.
fn lookup_shop_menu_detour(regs: *mut Registers, original: usize) -> usize {
    let result = unsafe { (*regs).rcx } as *mut ShopResult;
    let shop_type = unsafe { (*regs).rdx } as u8;
    let begin_id = unsafe { (*regs).r8 } as i32;
    let end_id = unsafe { (*regs).r9 } as i32;

    if begin_id == SHOP_ID {
        log(&format!(
            "anti_farm_shop: LookupShopMenu called shop_type={shop_type} begin_id={begin_id} end_id={end_id}"
        ));
        unsafe {
            (*result).shop_type = shop_type;
            (*result).id = begin_id;
            (*result).row = lineups().as_ptr();
        }
        log(&format!(
            "anti_farm_shop: LookupShopMenu served shop menu for begin_id={begin_id}"
        ));
        return result as usize;
    }

    let original_fn: extern "C" fn(*mut ShopResult, u8, i32, i32) -> *mut ShopResult =
        unsafe { std::mem::transmute(original) };
    original_fn(result, shop_type, begin_id, end_id) as usize
}

/// Hook for `SoloParamRepositoryImp::LookupShopLineup(result, shop_type, id)`.
/// Returns our lineup rows for ids in `[SHOP_ID, SHOP_ID + lineups.len())`;
/// anything else falls through to the vanilla lookup.
fn lookup_shop_lineup_detour(regs: *mut Registers, original: usize) -> usize {
    let result = unsafe { (*regs).rcx } as *mut ShopResult;
    let shop_type = unsafe { (*regs).rdx } as u8;
    let id = unsafe { (*regs).r8 } as i32;

    let in_range = id >= SHOP_ID && id < SHOP_ID + 10;
    if in_range {
        log(&format!(
            "anti_farm_shop: LookupShopLineup called shop_type={shop_type} id={id}"
        ));
    }

    let lineups = lineups();
    if id >= SHOP_ID && (id - SHOP_ID) < lineups.len() as i32 {
        unsafe {
            (*result).shop_type = shop_type;
            (*result).id = id;
            (*result).row = lineups.as_ptr().add((id - SHOP_ID) as usize);
        }
        log(&format!("anti_farm_shop: LookupShopLineup served lineup id={id}"));
        return result as usize;
    }

    let original_fn: extern "C" fn(*mut ShopResult, u8, i32) -> *mut ShopResult =
        unsafe { std::mem::transmute(original) };
    original_fn(result, shop_type, id) as usize
}

fn install_lookup_shop_menu_hook() {
    let Some(target) = scan::scan_pattern_call(LOOKUP_SHOP_MENU_PATTERN) else {
        log("anti_farm_shop: ERROR: LookupShopMenu signature not found");
        return;
    };
    log(&format!("anti_farm_shop: LookupShopMenu at {target:#x}"));
    hooks::install_retn("anti_farm_shop: LookupShopMenu", target, lookup_shop_menu_detour);
}

fn install_lookup_shop_lineup_hook() {
    let Some(match_addr) = scan::scan_pattern(LOOKUP_SHOP_LINEUP_PATTERN) else {
        log("anti_farm_shop: ERROR: LookupShopLineup signature not found");
        return;
    };
    let target = match_addr - LOOKUP_SHOP_LINEUP_OFFSET as u64;
    log(&format!("anti_farm_shop: LookupShopLineup at {target:#x}"));
    hooks::install_retn("anti_farm_shop: LookupShopLineup", target, lookup_shop_lineup_detour);
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
            MSG_ANTI_FARM_SHOP,
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
    register_message(MSG_ANTI_FARM_SHOP, "Anti-Farm QoL Shop");
    register_message_in_bnd(MSGBND_MENU_TEXT, MSG_SHOP_TITLE, "Anti-Farm QoL Shop");
    register_patcher(patch_grace);
    install_lookup_shop_menu_hook();
    install_lookup_shop_lineup_hook();
}
