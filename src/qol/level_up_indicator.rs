//! Level Up Indicator: shows a talk-list indicator on the Site of Grace
//! "Level Up" row when the player currently holds enough runes to pay for the
//! next level.
//!
//! The vanilla grace menu adds the Level Up row with `5:19` (an
//! `ADD_TALK_LIST_DATA_IF`, no indicator support). We replace that single row
//! with two `5:149` (`ADD_TALK_LIST_DATA_ALT`) rows that are mutually
//! exclusive at runtime:
//!
//!   ON:  `flags && afford`  -> indicator 1
//!   OFF: `flags && !afford` -> indicator 0
//!
//! Both rows keep the vanilla slot and message id, so the existing dispatch
//! (`GetTalkListEntryResult() == 2 -> state 18 -> ... -> OpenSoul`) is
//! untouched. Exactly one row exists per menu open, so the row stays selectable
//! and only the indicator flips.
//!
//! Affordability cannot be computed inside the ESD VM: there is no level-up
//! cost function, and the spendable rune pool is not reliably exposed as a
//! `GetPlayerStat` value. So this module computes `afford` on the Rust side
//! every frame from `PlayerGameData` (`level` + `rune_count`) and writes a
//! 1-byte literal (`0x41` = true, `0x40` = false) into both rows' conditions.
//! The VM then only evaluates `flags && <literal>`. Because the literal is
//! rewritten every frame (not merely on group initial-state entry), it is
//! always current by the time the talk list rows are re-added after a level-up
//! round-trip.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

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

/// Single-byte ESD small-int constants: `0x40 + value` for -64..=63.
const ESD_TRUE: u8 = 0x41; // 1
const ESD_FALSE: u8 = 0x40; // 0
/// Opcode for logical AND.
const OP_AND: u8 = 0x98;
/// End-of-expression terminator.
const OP_TERM: u8 = 0xA1;

/// `GetEventFlag(4680) == 1 || GetEventFlag(4699) == 1`, copied byte-for-byte
/// from the vanilla Level Up row's condition argument (ground truth, group
/// 2147483616 state 2). 19 bytes.
const FLAGS_COND: [u8; 19] = [
    0x4f, 0x82, 0x48, 0x12, 0x00, 0x00, 0x85, 0x41, 0x95, // GetEventFlag(4680) == 1
    0x4f, 0x82, 0x5b, 0x12, 0x00, 0x00, 0x85, 0x41, 0x95, // GetEventFlag(4699) == 1
    0x99, // ||
];

/// Byte offset of the affordability literal inside a condition expression:
/// immediately after [`FLAGS_COND`], before the `&&` opcode and terminator.
const AFFORD_OFFSET: usize = FLAGS_COND.len();

/// Builds a full condition expression (with terminator):
/// `flags && afford`, where `afford` is a 1-byte literal.
fn build_condition(afford: bool) -> Vec<u8> {
    let mut c = Vec::with_capacity(FLAGS_COND.len() + 3);
    c.extend_from_slice(&FLAGS_COND);
    c.push(if afford { ESD_TRUE } else { ESD_FALSE });
    c.push(OP_AND);
    c.push(OP_TERM);
    c
}

/// True for the vanilla `5:19` Level Up row: an `ADD_TALK_LIST_DATA_IF` whose
/// text id (arg 2) is the Level Up message.
fn is_level_up_if(event: &Event) -> bool {
    event.command == ADD_TALK_LIST_DATA_IF
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
    /// Builds both ALT rows with a placeholder affordability literal
    /// (`false`, so neither shows an indicator yet). `slot` is the vanilla
    /// Level Up row's slot so dispatch stays unchanged.
    fn new(slot: i32) -> Self {
        let on_cond = build_condition(false).into_boxed_slice();
        let off_cond = build_condition(false).into_boxed_slice();
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

    /// Rewrites the affordability literal in both rows' conditions. `afford`
    /// true -> ON row shows the indicator; false -> OFF row (no indicator).
    fn set_afford(&mut self, afford: bool) {
        let v = if afford { ESD_TRUE } else { ESD_FALSE };
        self.on_cond[AFFORD_OFFSET] = v;
        self.off_cond[AFFORD_OFFSET] = if afford { ESD_FALSE } else { ESD_TRUE };
    }
}

/// Addresses of the affordability literal byte inside the two leaked rows'
/// condition buffers, once installed. `on` is the ON row (indicator 1), `off`
/// the OFF row (indicator 0). Stored as raw addresses so the cache is
/// Send/Sync; the underlying buffers are leaked and stay valid for the whole
/// process lifetime.
#[derive(Clone, Copy)]
struct AffordWriters {
    on: usize,
    off: usize,
}

/// The installed writers, once installed. None until the first grace menu
/// entry.
static WRITERS: LazyLock<Mutex<Option<AffordWriters>>> =
    LazyLock::new(|| Mutex::new(None));

/// Last affordability value written, to avoid rewriting identical bytes.
static LAST_AFFORD: AtomicBool = AtomicBool::new(false);

/// Cost to level from `level` to `level + 1`, using the verified curve
/// `x = max(0, (level - 11) * 0.02); floor((x + 0.1) * (level + 81)^2) + 1`.
fn level_up_cost(level: u32) -> u32 {
    if level >= MAX_LEVEL {
        return u32::MAX; // sentinel: never affordable
    }
    let l = level as f64;
    let base = (l - 11.0) * 0.02;
    let x = if base < 0.0 { 0.0 } else { base };
    let raw = (x + 0.1) * (l + 81.0).powi(2);
    let cost = raw.floor() as u64 + 1;
    cost.min(u32::MAX as u64) as u32
}

/// The player's current level and held rune count from game data.
fn player_data() -> Option<(u32, u32)> {
    let man = GameDataMan::instance_ptr().ok()?;
    if man.is_null() {
        return None;
    }
    let pgd = unsafe { (*man).main_player_game_data.as_ptr() };
    if pgd.is_null() {
        return None;
    }
    let level = unsafe { (*pgd).level };
    let runes = unsafe { (*pgd).rune_count };
    Some((level, runes))
}

/// Whether the player can currently afford the next level.
fn can_afford() -> bool {
    player_data().map_or(false, |(level, runes)| runes >= level_up_cost(level))
}

/// Re-writes the affordability literal into the installed rows based on the
/// current level/rune count. Runs every frame so the talk list rows are always
/// fresh when the grace menu re-adds them after a level-up round-trip.
fn refresh() {
    if !config::with_feature(|cfg| cfg.level_up_indicator) {
        return;
    }
    let afford = can_afford();
    if LAST_AFFORD.swap(afford, Ordering::Relaxed) == afford {
        return;
    }
    let (v_on, v_off) = if afford {
        (ESD_TRUE, ESD_FALSE)
    } else {
        (ESD_FALSE, ESD_TRUE)
    };
    let guard = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(w) = *guard {
        unsafe {
            *(w.on as *mut u8) = v_on;
            *(w.off as *mut u8) = v_off;
        }
    }
}

/// Installs the two ALT Level Up rows over the vanilla `5:19` row, once.
/// Returns true if a structural change was made. Leaks the rows and caches
/// their affordability-literal addresses for [`refresh`]; idempotent (a later
/// call finds no vanilla row).
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

    // Refuse to install a second copy if writers already exist.
    let mut guard = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return false;
    }

    let rows = Box::leak(Box::new(LevelUpRows::new(slot)));
    rows.link();
    let afford = can_afford();
    rows.set_afford(afford);
    LAST_AFFORD.store(afford, Ordering::Relaxed);

    let events = [rows.on_event, rows.off_event];
    let replaced = unsafe { replace_entry_event(state_group, is_level_up_if, &events) };
    if replaced {
        let (level, runes) = player_data().unwrap_or((0, 0));
        log(&format!(
            "level_up_indicator: installed ALT rows (slot {slot}, level {level}, runes {runes})"
        ));
        *guard = Some(AffordWriters {
            on: rows.on_cond.as_ptr() as usize + AFFORD_OFFSET,
            off: rows.off_cond.as_ptr() as usize + AFFORD_OFFSET,
        });
    }
    replaced
}

/// Runs on every grace menu initial-state entry. Installs the rows once.
pub(crate) fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.level_up_indicator) {
        return false;
    }
    unsafe { install_rows(state_group) }
}

/// Runs once per frame from the frame-begin recurring task, keeping the rows'
/// affordability literal in sync with the player's current rune count.
pub(crate) fn tick() {
    refresh();
}

/// Registers this feature's patch routine. Called once at init.
pub(crate) fn init() {
    register_patcher(patch);
}