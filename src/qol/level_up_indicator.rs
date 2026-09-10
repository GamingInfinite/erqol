//! Level Up Indicator: shows a talk-list indicator on the Site of Grace
//! "Level Up" row when the player currently holds enough runes to pay for the
//! next level.
//!
//! The vanilla grace menu adds the Level Up row with `5:19` (an
//! `ADD_TALK_LIST_DATA_IF`, no indicator support). We replace that single row
//! with two `5:149` (`ADD_TALK_LIST_DATA_ALT`) rows that are mutually
//! exclusive at runtime:
//!
//!   ON:  `flags && GetPlayerStat(RunesCollected) >= cost`  -> indicator 1
//!   OFF: `flags && GetPlayerStat(RunesCollected) <  cost`  -> indicator 0
//!
//! Both rows keep the vanilla slot and message id, so the existing dispatch
//! (`GetTalkListEntryResult() == 2 -> state 18 -> ... -> OpenSoul`) is
//! untouched. Exactly one row exists per menu open, so the row stays selectable
//! and only the indicator flips.
//!
//! The cost is not available to the ESD VM as a constant, so the two condition
//! expressions embed a 4-byte literal that this module re-writes on every grace
//! menu open (the state machine re-evaluates the row conditions each time the
//! group's initial state is entered, before our back-fill's effects are read).

use eldenring::cs::GameDataMan;
use fromsoftware_shared::FromStatic;

use crate::config;
use crate::ezstate_menu::{
    ADD_TALK_LIST_DATA_ALT, ADD_TALK_LIST_DATA_IF, Event, Span, StateGroup,
    event_arg_int, make_int_expression, register_patcher, replace_entry_event,
    slice_of,
};
use crate::log::log;

/// Message id of the vanilla "Level Up" row at the Site of Grace.
const MSG_LEVEL_UP: i32 = 15000540;
/// Character level at which no further level ups are possible.
const MAX_LEVEL: u32 = 713;
/// Back-filled cost when the player is at max level: larger than the maximum
/// number of held runes (999,999,999), so `runes < cost` is always true and
/// the indicator stays off while the row remains available.
const COST_MAX_LEVEL: u32 = 1_000_000_000;

/// `GetEventFlag(4680) == 1 || GetEventFlag(4699) == 1`, copied byte-for-byte
/// from the vanilla Level Up row's condition argument (ground truth, group
/// 2147483616 state 2). 19 bytes.
const FLAGS_COND: [u8; 19] = [
    0x4f, 0x82, 0x48, 0x12, 0x00, 0x00, 0x85, 0x41, 0x95, // GetEventFlag(4680) == 1
    0x4f, 0x82, 0x5b, 0x12, 0x00, 0x00, 0x85, 0x41, 0x95, // GetEventFlag(4699) == 1
    0x99, // ||
];

/// `GetPlayerStat(RunesCollected)` = `0x82 <id104> 0x82 <8> 0x85` (11 bytes),
/// using the 4-byte int form for both the function id and the stat arg. The
/// trailing `0x85` is the 1-arg call opcode (0x84 + arity).
const GET_PLAYER_STAT_RUNES: [u8; 11] = [
    0x82, 0x68, 0x00, 0x00, 0x00, // function id 104
    0x82, 0x08, 0x00, 0x00, 0x00, // stat 8 (RunesCollected)
    0x85, // call(1)
];

/// Byte offset of the cost literal inside a built condition expression:
/// the `0x82` prefix that precedes the 4-byte little-endian cost value. The
/// builder and the back-filler share this so they can never drift.
const COST_EXPR_OFFSET: usize = FLAGS_COND.len() + GET_PLAYER_STAT_RUNES.len() + 1;

/// Builds a full condition expression (with terminator):
/// `flags && GetPlayerStat(8) <op> cost`.
/// `ge` selects `>=` (affordable, indicator ON row) vs `<` (NOT affordable,
/// indicator OFF row). Returns the buffer and the written cost (for the
/// caller to re-derive [`COST_EXPR_OFFSET`] is unnecessary; offset is const).
fn build_condition(cost: u32, ge: bool) -> Vec<u8> {
    let mut c = Vec::with_capacity(FLAGS_COND.len() + GET_PLAYER_STAT_RUNES.len() + 9);
    c.extend_from_slice(&FLAGS_COND);
    c.extend_from_slice(&GET_PLAYER_STAT_RUNES);
    c.push(0x82);
    c.extend_from_slice(&cost.to_le_bytes());
    c.push(if ge { 0x92 } else { 0x93 }); // >= / <
    c.push(0x98); // &&
    c.push(0xa1); // terminator
    c
}

/// True for the vanilla `5:19` Level Up row: an `ADD_TALK_LIST_DATA_IF` whose
/// text id (arg 2) is the Level Up message.
fn is_level_up_if(event: &Event) -> bool {
    event.command == ADD_TALK_LIST_DATA_IF
        && unsafe { event_arg_int(event, 2) } == MSG_LEVEL_UP
}

/// True for one of our installed `5:149` Level Up rows.
fn is_level_up_alt(event: &Event) -> bool {
    event.command == ADD_TALK_LIST_DATA_ALT
        && unsafe { event_arg_int(event, 2) } == MSG_LEVEL_UP
}

/// Leaked runtime rows replacing the vanilla Level Up row.
struct LevelUpRows {
    slot: [u8; 6],
    msg: [u8; 6],
    unk: [u8; 6],
    sort: [u8; 6],
    ind_on: [u8; 6],
    ind_off: [u8; 6],
    on_cond: Box<[u8]>,
    off_cond: Box<[u8]>,
    on_args: Box<[Span<u8>; 6]>,
    off_args: Box<[Span<u8>; 6]>,
    on_event: Event,
    off_event: Event,
}

impl LevelUpRows {
    /// Builds both ALT rows with placeholder cost 0 (back-filled on install).
    /// `slot` is the vanilla Level Up row's slot so dispatch stays unchanged.
    fn new(slot: i32, cost: u32) -> Self {
        let on_cond = build_condition(cost, true).into_boxed_slice();
        let off_cond = build_condition(cost, false).into_boxed_slice();
        Self {
            slot: make_int_expression(slot),
            msg: make_int_expression(MSG_LEVEL_UP),
            unk: make_int_expression(-1),
            sort: make_int_expression(0),
            ind_on: make_int_expression(1),
            ind_off: make_int_expression(0),
            on_cond,
            off_cond,
            on_args: Box::new([Span::null(); 6]),
            off_args: Box::new([Span::null(); 6]),
            on_event: Event {
                command: ADD_TALK_LIST_DATA_ALT,
                args: Span::null(),
            },
            off_event: Event {
                command: ADD_TALK_LIST_DATA_ALT,
                args: Span::null(),
            },
        }
    }

    /// Points the two events' arg spans at our own buffers. Must only run once
    /// the rows live at a stable (leaked) address.
    fn link(&mut self) {
        let mk_args = |cond: &[u8], slot: &[u8; 6], msg: &[u8; 6], unk: &[u8; 6], sort: &[u8; 6], ind: &[u8; 6]| {
            Box::new([
                Span { ptr: cond.as_ptr() as *mut u8, len: cond.len() },
                Span { ptr: slot.as_ptr() as *mut u8, len: slot.len() },
                Span { ptr: msg.as_ptr() as *mut u8, len: msg.len() },
                Span { ptr: unk.as_ptr() as *mut u8, len: unk.len() },
                Span { ptr: sort.as_ptr() as *mut u8, len: sort.len() },
                Span { ptr: ind.as_ptr() as *mut u8, len: ind.len() },
            ])
        };
        self.on_args = mk_args(
            &self.on_cond, &self.slot, &self.msg, &self.unk, &self.sort, &self.ind_on,
        );
        self.off_args = mk_args(
            &self.off_cond, &self.slot, &self.msg, &self.unk, &self.sort, &self.ind_off,
        );
        self.on_event.args = Span {
            ptr: self.on_args.as_ptr() as *mut Span<u8>,
            len: self.on_args.len(),
        };
        self.off_event.args = Span {
            ptr: self.off_args.as_ptr() as *mut Span<u8>,
            len: self.off_args.len(),
        };
    }
}

/// Cost to level from `level` to `level + 1`, using the verified curve
/// `x = max(0, (level - 11) * 0.02); floor((x + 0.1) * (level + 81)^2) + 1`.
/// At max level returns a sentinel so the indicator stays off.
fn level_up_cost(level: u32) -> u32 {
    if level >= MAX_LEVEL {
        return COST_MAX_LEVEL;
    }
    let l = level as f64;
    let base = (l - 11.0) * 0.02;
    let x = if base < 0.0 { 0.0 } else { base };
    let raw = (x + 0.1) * (l + 81.0).powi(2);
    let cost = raw.floor() as u64 + 1;
    cost.min(u32::MAX as u64) as u32
}

/// The player's current character level from game data.
fn player_level() -> Option<u32> {
    let man = GameDataMan::instance_ptr().ok()?;
    if man.is_null() {
        return None;
    }
    let pgd = unsafe { (*man).main_player_game_data.as_ptr() };
    if pgd.is_null() {
        return None;
    }
    Some(unsafe { (*pgd).level })
}

/// Re-writes the cost literal inside both installed ALT rows' condition
/// expressions. Runs on every grace menu open so the row reflects the current
/// level (cost rises with level) as well as current held runes (evaluated live
/// by the VM via `GetPlayerStat`). Byte offset is the same for both rows.
fn backfill_cost(state_group: *mut StateGroup, cost: u32) -> bool {
    // Misaligned slot in the event by finding the ALT row's condition arg, then
    // writing the 4 LE bytes at the fixed expression offset.
    let states = unsafe { slice_of((*state_group).states) };
    let mut wrote = false;
    for state in states {
        for event in unsafe { slice_of(state.entry_events) } {
            if !is_level_up_alt(event) || event.args.len < 6 || event.args.ptr.is_null() {
                continue;
            }
            let cond = unsafe { *event.args.ptr };
            if cond.ptr.is_null()
                || cond.len < COST_EXPR_OFFSET + 4
                || unsafe { *cond.ptr.add(COST_EXPR_OFFSET - 1) } != 0x82
            {
                continue;
            }
            unsafe {
                let dst = cond.ptr.add(COST_EXPR_OFFSET) as *mut u8;
                std::ptr::copy_nonoverlapping(&cost.to_le_bytes()[0], dst, 4);
            }
            wrote = true;
        }
    }
    wrote
}

/// Installs the two ALT Level Up rows over the vanilla `5:19` row, once.
/// Returns true if a structural change was made. Leaks the rows so the event
/// must never be rebuilt; idempotent (a later call finds the ALT rows, not the
/// vanilla row, and falls through to [`backfill_cost`] only).
unsafe fn install_rows(state_group: *mut StateGroup) -> bool {
    // Find the vanilla Level Up row and read the slot it used, so dispatch
    // (which fires on that slot number) is preserved exactly.
    let mut slot = 2i32;
    let mut found = false;
    let states = unsafe { slice_of((*state_group).states) };
    'outer: for state in states {
        for event in unsafe { slice_of(state.entry_events) } {
            if is_level_up_if(event) {
                let s = unsafe { event_arg_int(event, 1) };
                if s != -1 {
                    slot = s;
                }
                found = true;
                break 'outer;
            }
        }
    }
    if !found {
        return false;
    }

    let cost = player_level().map(level_up_cost).unwrap_or(0);
    let rows = Box::leak(Box::new(LevelUpRows::new(slot, cost)));
    rows.link();

    let events = [rows.on_event, rows.off_event];
    let replaced = unsafe { replace_entry_event(state_group, is_level_up_if, &events) };
    if replaced {
        log(&format!(
            "level_up_indicator: replaced Level Up row (slot {slot}, cost {cost}) with ALT rows"
        ));
    }
    replaced
}

/// Runs on every grace menu initial-state entry. First call replaces the
/// vanilla Level Up row; every call (including the first) back-fills the cost
/// literal into the installed rows based on the player's current level.
pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.level_up_indicator) {
        return false;
    }

    unsafe {
        // Back-fill cost first so a freshly installed set of rows is correct
        // even when the player already out-leveled the placeholder cost 0.
        if let Some(level) = player_level() {
            let cost = level_up_cost(level);
            backfill_cost(state_group, cost);
        }

        // Structural replacement is a one-time action; returns true only on the
        // first successful swap.
        install_rows(state_group)
    }
}

/// Registers this feature's patch routine. Called once at init.
pub(crate) fn init() {
    register_patcher(patch);
}