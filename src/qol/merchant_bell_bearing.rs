use std::ptr;
use std::sync::Mutex;

use crate::config;
use crate::memory;
use crate::ezstate_menu::{
    is_talk_list_result_transition, make_int_expression, make_talk_list_result_expression,
    register_group_patcher, register_message, slice_of, state_group_id, Command, Event, Span,
    State, StateGroup, Transition, ADD_TALK_LIST_DATA, ADD_TALK_LIST_DATA_ALT, ADD_TALK_LIST_DATA_IF,
    AWARD_ITEM_LOT, CLEAR_TALK_LIST_DATA, OPEN_REGULAR_SHOP, OPEN_SELL_SHOP,
};
use crate::log::log;

// ---- Merchant identity table ----
//
// A merchant is identified by the (range_start, range_end) pair baked into its
// ESD as `OpenRegularShop(range_start, range_end)` (`82 <start> a1 82 <end> a1`).
// The range literal is stored near the end of the loaded ESD blob, so the
// identity scan covers the whole VirtualQuery allocation that owns the state
// group pointer. Kalé is keyed directly by her menu group id (2147483596).

/// Menu group id of Kalé's talk menu (t800006000). Unique to her.
const KALE_GROUP: i32 = 2147483596;
/// Menu group ids used by the t801... ESDs' shop menus. Shared across all of
/// them (and reused by t800006000 forwarding groups), so identity needs the
/// range scan.
const MENU_GROUP_3603: i32 = 2147483603;
const MENU_GROUP_3604: i32 = 2147483604;

const MSG_BUY: i32 = 20000010;
const MSG_SELL: i32 = 20000011;
const MSG_LEAVE: i32 = 20000009;
const MSG_ACQUIRE_BELL_BEARING: i32 = 26000150;

const ACQUIRE_TEXT: &str = "Acquire Bell Bearing";

struct Merchant {
    range_start: i32,
    range_end: i32,
    item_lot: i32,
    event_flag: i32,
}

const fn merchant(start: i32, end: i32, lot: i32, flag: i32) -> Merchant {
    Merchant {
        range_start: start,
        range_end: end,
        item_lot: lot,
        event_flag: flag,
    }
}

/// The 19 purchasable bell-bearing merchants. Cut Nomadic Merchant [11]
/// (range 100850-100874, lot 119065, flag 400914) is deliberately absent.
const MERCHANTS: &[Merchant] = &[
    // Kalé (group 3596; range only used as a cross-check, identity is by group id)
    merchant(100500, 100524, 110410, 400049),
    merchant(100525, 100549, 119000, 400901),
    merchant(100550, 100574, 119005, 400902),
    merchant(100575, 100599, 119010, 400903),
    merchant(100600, 100624, 119015, 400904),
    merchant(100625, 100649, 119020, 400905),
    merchant(100650, 100674, 119025, 400906),
    merchant(100675, 100699, 119030, 400907),
    merchant(100700, 100724, 119035, 400908),
    merchant(100725, 100749, 119040, 400909),
    merchant(100750, 100774, 119045, 400910),
    merchant(100775, 100799, 119050, 400911),
    merchant(100800, 100824, 119055, 400912),
    merchant(100825, 100849, 119060, 400913),
    merchant(100875, 100899, 119070, 400915),
    merchant(100900, 100924, 119075, 400916),
    merchant(100925, 100949, 119080, 400917),
    merchant(100950, 100974, 119085, 400918),
    merchant(100975, 100999, 119090, 400919),
];

// ---- Runtime ESD helpers ----

/// `GetEventFlag(flag) == 0`. Function id 15 is GetEventFlag (byte 0x4f =
/// const 15), 0x85 = call with 1 argument, 0x40 = const 0, 0x95 = `==`,
/// 0xa1 = terminator.
fn make_flag_cleared_expression(flag: i32) -> [u8; 10] {
    [
        0x4f,
        0x82,
        flag as u8,
        (flag >> 8) as u8,
        (flag >> 16) as u8,
        (flag >> 24) as u8,
        0x85,
        0x40,
        0x95,
        0xa1,
    ]
}

/// The baked `OpenRegularShop(range_start, range_end)` range literal:
/// `82 <start u32> a1 82 <end u32> a1`.
fn make_range_pattern(start: i32, end: i32) -> [u8; 12] {
    [
        0x82,
        start as u8,
        (start >> 8) as u8,
        (start >> 16) as u8,
        (start >> 24) as u8,
        0xa1,
        0x82,
        end as u8,
        (end >> 8) as u8,
        (end >> 16) as u8,
        (end >> 24) as u8,
        0xa1,
    ]
}

/// Maximum number of bytes scanned upward from the state group pointer while
/// hunting the baked range literal. The owning ESD blob is small (tens of KB),
/// but several merchant ESDs share one pooled allocation, so a generous bound
/// keeps the scan fast while still finding the owning blob's range.
const MAX_IDENTIFY_SCAN: usize = 2 << 20;

/// Finds the merchant range literal in `bytes` (covering absolute addresses
/// `base..base + len`) at the lowest address strictly above `group_addr`.
/// The owning blob's literal always sits above its own struct pointers (it is
/// baked near the end of the blob, which ends before any later blob begins),
/// so the lowest match above the group pointer is the owning merchant's range.
/// Returns the merchant and the match's absolute address.
fn find_first_match_after(
    bytes: &[u8],
    base: usize,
    group_addr: usize,
) -> Option<(usize, &'static Merchant)> {
    let mut best: Option<(usize, &'static Merchant)> = None;
    for m in MERCHANTS {
        let pat = make_range_pattern(m.range_start, m.range_end);
        if let Some(rel) = bytes.windows(12).position(|w| w == pat) {
            let abs = base + rel;
            if abs > group_addr {
                match best {
                    Some((a, _)) if a <= abs => {}
                    _ => best = Some((abs, m)),
                }
            }
        }
    }
    best
}

/// Scans the memory regions reachable from `state_group` for a baked
/// merchant range literal, bounded to `MAX_IDENTIFY_SCAN` bytes so the scan can
/// never stall the menu. Returns the merchant whose literal is the lowest range
/// found above the group pointer.
unsafe fn identify_merchant(state_group: *mut StateGroup) -> Option<&'static Merchant> {
    let group_addr = state_group as usize;
    let mut scanned = 0usize;
    for region in memory::enum_memory_regions() {
        if region.base.saturating_add(region.size) <= group_addr {
            continue;
        }
        let start = group_addr.saturating_sub(region.base).min(region.size);
        let budget = (region.size - start).min(MAX_IDENTIFY_SCAN - scanned);
        if budget == 0 {
            continue;
        }
        let bytes = unsafe { std::slice::from_raw_parts(region.base as *const u8, region.size) };
        if let Some((abs, m)) =
            find_first_match_after(&bytes[start..start + budget], region.base + start, group_addr)
        {
            log(&format!(
                "merchant_bell_bearing: identified merchant (range {}-{}) at 0x{abs:x} (+0x{:x} above group 0x{group_addr:x})",
                m.range_start, m.range_end, abs - group_addr,
            ));
            return Some(m);
        }
        scanned += budget;
        if scanned >= MAX_IDENTIFY_SCAN {
            break;
        }
    }
    log(&format!(
        "merchant_bell_bearing: identity scan failed for group ptr 0x{group_addr:x}"
    ));
    None
}

// ---- Structural queries ----

/// True if any state in the group runs `command` in its entry/exit events.
unsafe fn group_has_command(state_group: *mut StateGroup, command: Command) -> bool {
    for state in unsafe { slice_of((*state_group).states) } {
        for event in unsafe { slice_of(state.entry_events) } {
            if event.command == command {
                return true;
            }
        }
        for event in unsafe { slice_of(state.exit_events) } {
            if event.command == command {
                return true;
            }
        }
    }
    false
}

/// The menu state: the state whose entry events list the shop rows (Buy/Sell/
/// Leave via `AddTalkListData`). Used as the anchor for row detection.
unsafe fn find_menu_state(state_group: *mut StateGroup) -> Option<*mut State> {
    for state in unsafe { slice_of((*state_group).states) } {
        let state_ptr = state as *const State as *mut State;
        for event in unsafe { slice_of(state.entry_events) } {
            if event.command == ADD_TALK_LIST_DATA
                && unsafe { crate::ezstate_menu::event_arg_int(event, 1) } == MSG_BUY
            {
                return Some(state_ptr);
            }
        }
    }
    None
}

/// The dispatch state: the state with a `SetREG0(GetTalkListEntryResult())`
/// row-dispatch transition.
unsafe fn find_dispatch_state(state_group: *mut StateGroup) -> Option<*mut State> {
    for state in unsafe { slice_of((*state_group).states) } {
        let state_ptr = state as *const State as *mut State;
        for transition in unsafe { slice_of(state.transitions) } {
            if unsafe { is_talk_list_result_transition(*transition) } {
                return Some(state_ptr);
            }
        }
    }
    None
}

/// True if the menu state lists the Sell row.
unsafe fn has_sell_row(menu_state: *mut State) -> bool {
    for event in unsafe { slice_of((*menu_state).entry_events) } {
        if event.command == ADD_TALK_LIST_DATA
            && unsafe { crate::ezstate_menu::event_arg_int(event, 1) } == MSG_SELL
        {
            return true;
        }
    }
    false
}

/// The menu-reset state: the state that clears the talk list when re-entered.
/// Looping the award state back here rebuilds the menu and returns the player
/// to the shop flow.
unsafe fn find_reset_state(state_group: *mut StateGroup) -> Option<*mut State> {
    for state in unsafe { slice_of((*state_group).states) } {
        let state_ptr = state as *const State as *mut State;
        for event in unsafe { slice_of(state.entry_events) } {
            if event.command == CLEAR_TALK_LIST_DATA {
                return Some(state_ptr);
            }
        }
    }
    None
}

/// The index of the Leave row within the menu state's entry events, so the
/// new row can be inserted just before it.
unsafe fn find_leave_row_index(menu_state: *mut State) -> Option<usize> {
    let events = unsafe { slice_of((*menu_state).entry_events) };
    for (i, event) in events.iter().enumerate() {
        if event.command == ADD_TALK_LIST_DATA
            && unsafe { crate::ezstate_menu::event_arg_int(event, 1) } == MSG_LEAVE
        {
            return Some(i);
        }
    }
    None
}

/// The first free menu row index in 3..=8, scanning the existing rows of the
/// menu state. Rows below 3 are reserved (1=Buy, 2=Sell) and the Leave row
/// index is excluded from this range.
unsafe fn first_free_index(menu_state: *mut State) -> Option<i32> {
    let events = unsafe { slice_of((*menu_state).entry_events) };
    let mut used = [false; 10];
    for event in events {
        let idx = match event.command {
            ADD_TALK_LIST_DATA => unsafe { crate::ezstate_menu::event_arg_int(event, 0) },
            ADD_TALK_LIST_DATA_IF | ADD_TALK_LIST_DATA_ALT => {
                unsafe { crate::ezstate_menu::event_arg_int(event, 1) }
            }
            _ => -1,
        };
        if (3..=8).contains(&idx) {
            used[idx as usize] = true;
        }
    }
    (3..=8).find(|&i| !used[i as usize]).map(|i| i as i32)
}

/// True if the menu state already contains a row for our message
/// (idempotence check).
unsafe fn has_acquire_row(menu_state: *mut State) -> bool {
    for event in unsafe { slice_of((*menu_state).entry_events) } {
        let msg = match event.command {
            ADD_TALK_LIST_DATA => unsafe { crate::ezstate_menu::event_arg_int(event, 1) },
            ADD_TALK_LIST_DATA_IF | ADD_TALK_LIST_DATA_ALT => {
                unsafe { crate::ezstate_menu::event_arg_int(event, 2) }
            }
            _ => -1,
        };
        if msg == MSG_ACQUIRE_BELL_BEARING {
            return true;
        }
    }
    false
}

// ---- The spliced-in state machine ----

/// The runtime ESD state machine spliced into a merchant menu:
/// a conditional `5:19` row that shows when the merchant's flag is cleared,
/// dispatching to a one-shot award state that runs `AwardItemLot(lot)` and
/// returns to the menu-reset state. Leaked alongside the row/state it owns.
struct BellBearingSplice {
    cond_expr: [u8; 10],
    index_expr: [u8; 6],
    msg_expr: [u8; 6],
    minus_expr: [u8; 6],
    lot_expr: [u8; 6],
    dispatch_expr: [u8; 9],
    true_expr: [u8; 2],
    row_args: [Span<u8>; 4],
    row_event: Event,
    award_args: [Span<u8>; 1],
    award_event: Event,
    dispatch_transition: Transition,
    return_transition: Transition,
    transition_arr: [*mut Transition; 1],
    state: State,
}

impl BellBearingSplice {
    fn new(index: i32, msg: i32, lot: i32, flag: i32) -> Self {
        Self {
            cond_expr: make_flag_cleared_expression(flag),
            index_expr: make_int_expression(index),
            msg_expr: make_int_expression(msg),
            minus_expr: make_int_expression(-1),
            lot_expr: make_int_expression(lot),
            dispatch_expr: make_talk_list_result_expression(index),
            true_expr: [0x41, 0xa1],
            row_args: [Span::null(); 4],
            row_event: Event {
                command: ADD_TALK_LIST_DATA_IF,
                args: Span::null(),
            },
            award_args: [Span::null(); 1],
            award_event: Event {
                command: AWARD_ITEM_LOT,
                args: Span::null(),
            },
            dispatch_transition: Transition {
                target_state: ptr::null_mut(),
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            return_transition: Transition {
                target_state: ptr::null_mut(),
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

    /// Fixes up all self-referential spans now that the splice lives at a
    /// stable (leaked heap) address. `reset_state` is the award state's return
    /// target.
    fn link(&mut self, reset_state: *mut State) {
        self.row_args = [
            Span {
                ptr: self.cond_expr.as_mut_ptr(),
                len: self.cond_expr.len(),
            },
            Span {
                ptr: self.index_expr.as_mut_ptr(),
                len: self.index_expr.len(),
            },
            Span {
                ptr: self.msg_expr.as_mut_ptr(),
                len: self.msg_expr.len(),
            },
            Span {
                ptr: self.minus_expr.as_mut_ptr(),
                len: self.minus_expr.len(),
            },
        ];
        self.row_event.args = Span {
            ptr: self.row_args.as_mut_ptr(),
            len: self.row_args.len(),
        };

        self.award_args = [Span {
            ptr: self.lot_expr.as_mut_ptr(),
            len: self.lot_expr.len(),
        }];
        self.award_event.args = Span {
            ptr: self.award_args.as_mut_ptr(),
            len: self.award_args.len(),
        };

        self.dispatch_transition.target_state = &mut self.state as *mut State;
        self.dispatch_transition.evaluator = Span {
            ptr: self.dispatch_expr.as_mut_ptr(),
            len: self.dispatch_expr.len(),
        };

        self.return_transition.target_state = reset_state;
        self.return_transition.evaluator = Span {
            ptr: self.true_expr.as_mut_ptr(),
            len: self.true_expr.len(),
        };
        self.transition_arr = [&mut self.return_transition as *mut Transition];
        self.state.transitions = Span {
            ptr: self.transition_arr.as_mut_ptr(),
            len: self.transition_arr.len(),
        };
        self.state.entry_events = Span {
            ptr: &self.award_event as *const Event as *mut Event,
            len: 1,
        };
    }
}

// ---- The patcher ----

static PATCHED: Mutex<Vec<(i32, i32, i32)>> = Mutex::new(Vec::new());

/// Slices the "Acquire Bell Bearing" row into a merchant menu. Identity:
/// group id 2147483596 = Kalé directly; groups 2147483603/2147483604 are
/// identified by scanning the owning ESD allocation for the baked
/// `OpenRegularShop(start, end)` range literal and matching it against the
/// merchant table. Idempotent; returns true on first patch.
unsafe fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.merchant_bell_bearing) {
        return false;
    }

    let Some(id) = (unsafe { state_group_id(state_group) }) else {
        return false;
    };
    if id != KALE_GROUP && id != MENU_GROUP_3603 && id != MENU_GROUP_3604 {
        return false;
    }

    // Structural check: a merchant menu group runs OpenRegularShop +
    // OpenSellShop and lists the Buy/Sell rows. This rules out the t800006000
    // forwarding groups that reuse 3603/3604 but contain no shop commands.
    if !(unsafe { group_has_command(state_group, OPEN_REGULAR_SHOP) })
        || !(unsafe { group_has_command(state_group, OPEN_SELL_SHOP) })
    {
        return false;
    }

    let Some(menu_state) = (unsafe { find_menu_state(state_group) }) else {
        return false;
    };
    if !(unsafe { has_sell_row(menu_state) }) {
        return false;
    }
    if unsafe { has_acquire_row(menu_state) } {
        return false;
    }

    let Some(dispatch_state) = (unsafe { find_dispatch_state(state_group) }) else {
        return false;
    };
    let Some(reset_state) = (unsafe { find_reset_state(state_group) }) else {
        return false;
    };
    let Some(leave_index) = (unsafe { find_leave_row_index(menu_state) }) else {
        return false;
    };
    let Some(index) = (unsafe { first_free_index(menu_state) }) else {
        return false;
    };

    let merchant = if id == KALE_GROUP {
        // Kalé's blob (t800006000) bakes three ranges (her own 100500-100524
        // plus the forwarding groups 3587/3580), so the scan is ambiguous for
        // her. Her menu group id is unique, so resolve her directly.
        &MERCHANTS[0]
    } else {
        let Some(merchant) = (unsafe { identify_merchant(state_group) }) else {
            log(&format!(
                "merchant_bell_bearing: no merchant identity found for group {id}"
            ));
            return false;
        };
        merchant
    };

    let key = (id, merchant.range_start, merchant.range_end);
    {
        let patched = PATCHED.lock().unwrap_or_else(|e| e.into_inner());
        if patched.contains(&key) {
            return false;
        }
    }

    // Build and leak the spliced-in machine.
    let splice = Box::into_raw(Box::new(BellBearingSplice::new(
        index,
        MSG_ACQUIRE_BELL_BEARING,
        merchant.item_lot,
        merchant.event_flag,
    )));
    unsafe {
        (*splice).link(reset_state);
    }

    // Insert the `5:19(GetEventFlag(flag) == 0, index, msg, -1)` row before
    // the Leave row in the menu state's entry events.
    let old_events = unsafe { slice_of((*menu_state).entry_events) };
    let mut new_events: Vec<Event> = old_events.to_vec();
    new_events.insert(leave_index, unsafe { (*splice).row_event });
    let event_count = new_events.len();
    let events_ptr = Box::into_raw(new_events.into_boxed_slice()) as *mut Event;
    unsafe {
        (*menu_state).entry_events = Span {
            ptr: events_ptr,
            len: event_count,
        };
    }

    // Insert the dispatch transition before the `if 1` default fallback so
    // the numeric row checks keep their precedence.
    let old_transitions = unsafe { slice_of((*dispatch_state).transitions) };
    let mut insert_at = old_transitions.len();
    for (i, transition) in old_transitions.iter().enumerate() {
        if unsafe { crate::ezstate_menu::get_ezstate_int_value((**transition).evaluator) } == 1 {
            insert_at = i;
            break;
        }
    }
    let mut new_transitions: Vec<*mut Transition> = Vec::with_capacity(old_transitions.len() + 1);
    new_transitions.extend_from_slice(&old_transitions[..insert_at]);
    new_transitions.push(unsafe { &mut (*splice).dispatch_transition });
    new_transitions.extend_from_slice(&old_transitions[insert_at..]);
    let transition_count = new_transitions.len();
    let transitions_ptr = Box::into_raw(new_transitions.into_boxed_slice()) as *mut *mut Transition;
    unsafe {
        (*dispatch_state).transitions = Span {
            ptr: transitions_ptr,
            len: transition_count,
        };
    }

    PATCHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(key);

    log(&format!(
        "merchant_bell_bearing: patched group {id} (range {}-{}, lot {}, flag {}) row index {index}",
        merchant.range_start, merchant.range_end, merchant.item_lot, merchant.event_flag,
    ));
    true
}

/// Registers this feature's group patcher and the "Acquire Bell Bearing"
/// menu text. Called once, before `ezstate_menu::install()`.
pub(crate) fn init() {
    register_message(MSG_ACQUIRE_BELL_BEARING, ACQUIRE_TEXT);
    register_group_patcher(patch);
}
