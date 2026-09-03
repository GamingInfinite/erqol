use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, Once, OnceLock};

use ilhook::x64::Registers;

use crate::hooks;
use crate::log::log;
use crate::memory;
use crate::scan;

// ---- Message ID allocation ----
//
// Menu/list rows show text looked up by an integer message id in the reserved
// `69_990_000..=69_990_100` region of MSGBND_EVENT_TEXT_FOR_TALK. Every module
// that registers such a row must get its ids from here so no two modules ever
// reuse the same id (a real bug before: hardcoded ids collided between the
// settings menu and roundtable_at_home). Allocate once per logical id/block at
// init time with [`alloc_message_id`] / [`alloc_message_block`].

/// First (highest) id handed out; we count down so blocks stay within the
/// reserved region.
const MSG_ID_BASE: i32 = 69_990_100;
static NEXT_MSG_ID: AtomicI32 = AtomicI32::new(MSG_ID_BASE);

/// Allocates the next free message id. Each call returns a distinct id.
pub(crate) fn alloc_message_id() -> i32 {
    NEXT_MSG_ID.fetch_sub(1, Ordering::Relaxed)
}

/// Allocates `count` contiguous message ids, returning the lowest one. The
/// caller uses `base..base+count` (e.g. per-category label ranges).
pub(crate) fn alloc_message_block(count: usize) -> i32 {
    let c = count as i32;
    NEXT_MSG_ID.fetch_sub(c, Ordering::Relaxed) - c + 1
}

// ---- Talk command constants (mirrors elden-x talk_commands.hpp) ----

pub(crate) const ADD_TALK_LIST_DATA: Command = Command { bank: 1, id: 19 };
pub(crate) const ADD_TALK_LIST_DATA_IF: Command = Command { bank: 5, id: 19 };
pub(crate) const ADD_TALK_LIST_DATA_ALT: Command = Command { bank: 5, id: 149 };
const CLOSE_SHOP_MESSAGE: Command = Command { bank: 1, id: 12 };
pub(crate) const CLEAR_TALK_LIST_DATA: Command = Command { bank: 1, id: 20 };
const SHOW_SHOP_MESSAGE: Command = Command { bank: 1, id: 10 };
/// `OpenRegularShop(range_start, range_end)` — the merchant's buy shop.
pub(crate) const OPEN_REGULAR_SHOP: Command = Command { bank: 1, id: 22 };
/// `OpenSellShop(-1, -1)` — the sell-shop sub-menu.
pub(crate) const OPEN_SELL_SHOP: Command = Command { bank: 1, id: 46 };
/// `OpenEnhanceShop(0)` — weapon/armament reinforcement.
pub(crate) const OPEN_ENHANCE_SHOP: Command = Command { bank: 1, id: 24 };
/// `CombineMenuFlagAndEventFlag(menuFlagId, eventFlagId)` — sets up internal
/// state before certain shop menus (e.g. the enhancement shop).
pub(crate) const COMBINE_MENU_FLAG_AND_EVENT_FLAG: Command = Command { bank: 1, id: 49 };
/// `OpenEquipmentChangeOfPurposeShop()` — Ash of War duplication.
pub(crate) const OPEN_EQUIPMENT_CHANGE_OF_PURPOSE_SHOP: Command = Command { bank: 1, id: 48 };
/// `OpenBuddyUpgradeMenu()` — spirit tuning.
pub(crate) const OPEN_BUDDY_UPGRADE_MENU: Command = Command { bank: 1, id: 136 };
/// `AwardItemLot(lot)` — grants the item lot. Used for bell bearings.
pub(crate) const AWARD_ITEM_LOT: Command = Command { bank: 1, id: 104 };
/// `6:2147483647(...)` — the generic `OpenGenericDialog` sub-call.
#[allow(dead_code)]
const OPEN_GENERIC_DIALOG: Command = Command {
    bank: 6,
    id: 2147483647,
};
const MSGBND_EVENT_TEXT_FOR_TALK: u32 = 33;
/// PlaceName message bnd (0x13): the world-map marker label path (text_type 0,
/// textId1) resolves through `LookupEntry(repo, lang, 0x13, id)`. Custom map
/// icon labels are registered here.
const MSGBND_PLACE_NAME: u32 = 0x13;

const MSG_SORT_CHEST: i32 = 15000395;

// ---- AOB signatures (from elden-ring-transmog) ----
// Both end in `e8 $ '` so scan_pattern_call resolves the function the call
// targets rather than the call site itself.

const ENTER_STATE_PATTERN: &str = "80 7e 18 00 74 15 4c 8d 44 24 40 48 8b d6 48 8b 4e 20 e8 $ '";

const LOOKUP_ENTRY_PATTERN: &str = "8b da 44 8b ca 33 d2 48 8b f9 44 8d 42 6f e8 $ '";

// ---- Raw ESD structs (mirrors elden-x er::ezstate/ezstate.hpp) ----

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Span<T> {
    pub(crate) ptr: *mut T,
    pub(crate) len: usize,
}

impl<T> Span<T> {
    pub(crate) fn null() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) bank: i32,
    pub(crate) id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Event {
    pub(crate) command: Command,
    pub(crate) args: Span<Span<u8>>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Transition {
    pub(crate) target_state: *mut State,
    pub(crate) pass_events: Span<Event>,
    pub(crate) sub_transitions: Span<*mut Transition>,
    pub(crate) evaluator: Span<u8>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct State {
    pub(crate) id: i32,
    pub(crate) transitions: Span<*mut Transition>,
    pub(crate) entry_events: Span<Event>,
    pub(crate) exit_events: Span<Event>,
    pub(crate) while_events: Span<Event>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct StateGroup {
    pub(crate) id: i32,
    pub(crate) states: Span<State>,
    pub(crate) initial_state: *mut State,
}

#[repr(C)]
struct Machine {
    vtable: usize,
    unk1: [u8; 0x20],
    state_group: *mut StateGroup,
    unk2: [u8; 0x110],
}

pub(crate) unsafe fn slice_of<T>(span: Span<T>) -> &'static [T] {
    if span.len == 0 || span.ptr.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(span.ptr, span.len) }
    }
}

// ---- ESD expression helpers ----

pub(crate) fn make_int_expression(value: i32) -> [u8; 6] {
    [
        0x82,
        value as u8,
        (value >> 8) as u8,
        (value >> 16) as u8,
        (value >> 24) as u8,
        0xa1,
    ]
}

/// `GetTalkListEntryResult() == value`
pub(crate) fn make_talk_list_result_expression(value: i32) -> [u8; 9] {
    [
        0x57,
        0x84,
        0x82,
        value as u8,
        (value >> 8) as u8,
        (value >> 16) as u8,
        (value >> 24) as u8,
        0x95,
        0xa1,
    ]
}

/// `(CheckSpecificPersonMenuIsOpen(1, 0) == 1 && CheckSpecificPersonGenericDialogIsOpen(0) == 0) == 0`
/// i.e. fires once the talk list menu has closed.
const TALK_MENU_CLOSED_EXPR: [u8; 15] = [
    0x7b, 0x41, 0x40, 0x86, 0x41, 0x95, // menu open (1, 0) == 1
    0x7a, 0x40, 0x85, 0x40, 0x95, // generic dialog open (0) == 0
    0x98, 0x40, 0x95, 0xa1, // && (both) == 0
];

/// Same as [`TALK_MENU_CLOSED_EXPR`] but for menu type 5 (the regular shop
/// menu): `(CheckSpecificPersonMenuIsOpen(5, 0) == 1 && CheckSpecificPersonGenericDialogIsOpen(0) == 0) == 0`.
pub(crate) const SHOP_MENU_CLOSED_EXPR: [u8; 15] = [
    0x7b, 0x45, 0x40, 0x86, 0x41, 0x95, // shop menu open (5, 0) == 1
    0x7a, 0x40, 0x85, 0x40, 0x95, // generic dialog open (0) == 0
    0x98, 0x40, 0x95, 0xa1, // && (both) == 0
];

/// Builds a menu-closed expression for an arbitrary menu type.
/// `(CheckSpecificPersonMenuIsOpen(menu_type, 0) == 1 && CheckSpecificPersonGenericDialogIsOpen(0) == 0) == 0`
pub(crate) fn make_menu_closed_expr(menu_type: i32) -> [u8; 15] {
    [
        0x7b,
        (menu_type + 64) as u8,
        0x40,
        0x86,
        0x41,
        0x95,
        0x7a,
        0x40,
        0x85,
        0x40,
        0x95,
        0x98,
        0x40,
        0x95,
        0xa1,
    ]
}

/// Parses an ESD expression containing only a 1 or 4 byte integer.
pub(crate) unsafe fn get_ezstate_int_value(expr: Span<u8>) -> i32 {
    if expr.len == 2 && !expr.ptr.is_null() {
        return unsafe { *expr.ptr } as i32 - 64;
    }
    if expr.len == 6 && !expr.ptr.is_null() && unsafe { *expr.ptr } == 0x82 {
        return unsafe { i32::from_le_bytes(*(expr.ptr.add(1) as *const [u8; 4])) };
    }
    -1
}

pub(crate) unsafe fn event_arg_int(event: &Event, index: usize) -> i32 {
    if event.args.len <= index || event.args.ptr.is_null() {
        return -1;
    }
    unsafe { get_ezstate_int_value(*event.args.ptr.add(index)) }
}

unsafe fn is_sort_chest_event(event: &Event) -> bool {
    if event.command == ADD_TALK_LIST_DATA {
        return unsafe { event_arg_int(event, 1) } == MSG_SORT_CHEST;
    }
    if event.command == ADD_TALK_LIST_DATA_IF || event.command == ADD_TALK_LIST_DATA_ALT {
        return unsafe { event_arg_int(event, 2) } == MSG_SORT_CHEST;
    }
    false
}

unsafe fn is_grace_state_group(state_group: *mut StateGroup) -> bool {
    if state_group.is_null() {
        return false;
    }
    let states = unsafe { slice_of((*state_group).states) };
    for state in states {
        for event in unsafe { slice_of(state.entry_events) } {
            if unsafe { is_sort_chest_event(event) } {
                return true;
            }
        }
    }
    false
}

/// True if the transition's evaluator is `SetREG0(GetTalkListEntryResult()) == N`,
/// the signature of the row-dispatch transition of a talk menu state.
pub(crate) unsafe fn is_talk_list_result_transition(transition: *mut Transition) -> bool {
    if transition.is_null() {
        return false;
    }
    let evaluator = unsafe { (*transition).evaluator };
    evaluator.len >= 2
        && !evaluator.ptr.is_null()
        && unsafe { *evaluator.ptr == 0x57 && *evaluator.ptr.add(1) == 0x84 }
}

// ---- The injected menu option ----

/// One `AddTalkListData(index, message_id, -1)` row plus the transition that
/// fires when it's selected. Non-default options dispatch on
/// `GetTalkListEntryResult() == index`; the default (cancel) option uses a
/// constant-true evaluator so it also catches backing out of the menu.
struct MenuOption {
    is_default: bool,
    action: Option<SubMenuAction>,
    index_expr: [u8; 6],
    message_expr: [u8; 6],
    placeholder_expr: [u8; 6],
    condition_expr: [u8; 9],
    true_expr: [u8; 2],
    args: [Span<u8>; 3],
    transition: Transition,
}

impl MenuOption {
    fn new(index: i32, message_id: i32, is_default: bool, action: Option<SubMenuAction>) -> Self {
        Self {
            is_default,
            action,
            index_expr: make_int_expression(index),
            message_expr: make_int_expression(message_id),
            placeholder_expr: make_int_expression(-1),
            condition_expr: make_talk_list_result_expression(index),
            true_expr: [0x41, 0xa1],
            args: [Span::null(); 3],
            transition: Transition {
                target_state: ptr::null_mut(),
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
        }
    }

    /// Points the args/evaluator spans at our own byte arrays. Only call this
    /// once the option lives at a stable address (after leaking).
    fn link(&mut self) {
        self.args = [
            Span {
                ptr: self.index_expr.as_mut_ptr(),
                len: self.index_expr.len(),
            },
            Span {
                ptr: self.message_expr.as_mut_ptr(),
                len: self.message_expr.len(),
            },
            Span {
                ptr: self.placeholder_expr.as_mut_ptr(),
                len: self.placeholder_expr.len(),
            },
        ];
        self.transition.evaluator = if self.is_default {
            Span {
                ptr: self.true_expr.as_mut_ptr(),
                len: self.true_expr.len(),
            }
        } else {
            Span {
                ptr: self.condition_expr.as_mut_ptr(),
                len: self.condition_expr.len(),
            }
        };
    }

    fn add_talk_list_data_event(&self) -> Event {
        Event {
            command: ADD_TALK_LIST_DATA,
            args: Span {
                ptr: self.args.as_ptr() as *mut Span<u8>,
                len: self.args.len(),
            },
        }
    }

    fn transition_ptr(&mut self) -> *mut Transition {
        &mut self.transition
    }
}

/// A one-shot state that runs an action callback when the state machine enters
/// it, then immediately returns to the parent menu through a constant-true
/// transition. Leaked alongside the submenu so its address stays valid.
struct ActionTarget {
    true_expr: [u8; 2],
    return_transition: Transition,
    transition_arr: [*mut Transition; 1],
    state: State,
}

impl ActionTarget {
    fn new(return_state: *mut State) -> Self {
        Self {
            true_expr: [0x41, 0xa1],
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

    /// Fixes up the self-referential spans now that the target lives at a
    /// stable (leaked heap) address. Setting these before the move would leave
    /// them pointing at the dead stack copy of the struct.
    fn link(&mut self) {
        self.return_transition.evaluator = Span {
            ptr: self.true_expr.as_mut_ptr(),
            len: self.true_expr.len(),
        };
        self.transition_arr = [&mut self.return_transition as *mut Transition];
        self.state.transitions = Span {
            ptr: self.transition_arr.as_mut_ptr(),
            len: self.transition_arr.len(),
        };
    }
}

// ---- The submenu ----

/// A talk list submenu opened from a parent menu. Mirrors the transmog mod's
/// `talkscript_menu_state`: a menu state whose entry events rebuild the talk
/// list, one transition to a branch state that fires once the menu closes, and
/// a branch state whose transitions dispatch on the selected row. Generic over
/// any number of rows.
pub(crate) struct SubMenu {
    options: Box<[MenuOption]>,
    menu_closed_expr: [u8; 15],
    generic_dialog_msg: [u8; 6],
    events: Box<[Event]>,
    show_msg_args: Box<[Span<u8>]>,
    menu_open_transition: Transition,
    menu_open_transition_arr: [*mut Transition; 1],
    pub(crate) state: State,
    branch_state: State,
    branch_transitions: Box<[*mut Transition]>,
}

impl SubMenu {
    /// Rows are `(index, message_id, is_default, action)`. A row with
    /// `Some(action)` gets its own action state: selecting it runs the
    /// callback (via `ACTIONS` when the state machine enters the state) and
    /// then returns to the parent menu.
    pub(crate) fn new(rows: &[(i32, i32, bool, Option<SubMenuAction>)]) -> Self {
        let options = rows
            .iter()
            .map(|&(index, message_id, is_default, action)| {
                MenuOption::new(index, message_id, is_default, action)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            options,
            menu_closed_expr: TALK_MENU_CLOSED_EXPR,
            generic_dialog_msg: make_int_expression(0),
            events: Box::default(),
            show_msg_args: Box::default(),
            menu_open_transition: Transition {
                target_state: ptr::null_mut(),
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            menu_open_transition_arr: [ptr::null_mut()],
            state: State {
                id: 0,
                transitions: Span::null(),
                entry_events: Span::null(),
                exit_events: Span::null(),
                while_events: Span::null(),
            },
            branch_state: State {
                id: 0,
                transitions: Span::null(),
                entry_events: Span::null(),
                exit_events: Span::null(),
                while_events: Span::null(),
            },
            branch_transitions: Box::default(),
        }
    }

    /// Fixes up every self-referential span now that the submenu lives at a
    /// stable address, then returns a pointer to the submenu state. Rows
    /// without an action return to `return_state`; rows with an action get a
    /// dedicated action state and return to `return_state` after the callback.
    /// `return_state` is where rows without an action go (e.g. Cancel).
    /// `action_return_state` is where action rows return to; pass the submenu
    /// state itself to keep the player inside the submenu after a toggle.
    pub(crate) unsafe fn link(
        ptr: *mut SubMenu,
        return_state: *mut State,
        action_return_state: *mut State,
    ) -> *mut State {
        let this = unsafe { &mut *ptr };

        for opt in &mut this.options {
            opt.link();
            let target_return_state = if opt.action.is_some() && !action_return_state.is_null() {
                action_return_state
            } else {
                return_state
            };
            opt.transition.target_state = match opt.action {
                Some(action) => {
                    let target = Box::into_raw(Box::new(ActionTarget::new(target_return_state)));
                    unsafe { (*target).link() };
                    let action_state = unsafe { &mut (*target).state } as *mut State;
                    register_action(action_state, action);
                    action_state
                }
                None => target_return_state,
            };
        }

        this.show_msg_args = Box::from([Span {
            ptr: this.generic_dialog_msg.as_mut_ptr(),
            len: this.generic_dialog_msg.len(),
        }]);

        let mut events = Vec::with_capacity(this.options.len() + 3);
        events.push(Event {
            command: CLOSE_SHOP_MESSAGE,
            args: Span::null(),
        });
        events.push(Event {
            command: CLEAR_TALK_LIST_DATA,
            args: Span::null(),
        });
        for opt in &mut this.options {
            events.push(Event {
                command: ADD_TALK_LIST_DATA,
                args: Span {
                    ptr: opt.args.as_mut_ptr(),
                    len: 3,
                },
            });
        }
        events.push(Event {
            command: SHOW_SHOP_MESSAGE,
            args: Span {
                ptr: this.show_msg_args.as_ptr() as *mut Span<u8>,
                len: 1,
            },
        });
        this.events = events.into_boxed_slice();

        this.menu_open_transition.target_state = &mut this.branch_state as *mut State;
        this.menu_open_transition.evaluator = Span {
            ptr: this.menu_closed_expr.as_mut_ptr(),
            len: this.menu_closed_expr.len(),
        };
        this.menu_open_transition_arr = [&mut this.menu_open_transition as *mut Transition];

        this.branch_transitions = this
            .options
            .iter_mut()
            .map(|opt| &mut opt.transition as *mut Transition)
            .collect::<Vec<_>>()
            .into_boxed_slice();

        this.state.transitions = Span {
            ptr: this.menu_open_transition_arr.as_mut_ptr(),
            len: this.menu_open_transition_arr.len(),
        };
        this.state.entry_events = Span {
            ptr: this.events.as_ptr() as *mut Event,
            len: this.events.len(),
        };
        this.state.exit_events = Span::null();
        this.state.while_events = Span::null();

        this.branch_state.transitions = Span {
            ptr: this.branch_transitions.as_ptr() as *mut *mut Transition,
            len: this.branch_transitions.len(),
        };
        this.branch_state.entry_events = Span::null();
        this.branch_state.exit_events = Span::null();
        this.branch_state.while_events = Span::null();

        &mut this.state as *mut State
    }

    /// Builds, leaks, and links a [`SubMenu`] from a row description in one step.
    /// This is the common pattern used by grace menu patchers.
    #[allow(dead_code)]
    pub(crate) unsafe fn link_from_rows(
        rows: &[(i32, i32, bool, Option<SubMenuAction>)],
        return_state: *mut State,
        action_return_state: *mut State,
    ) -> *mut State {
        let submenu = Box::into_raw(Box::new(SubMenu::new(rows)));
        unsafe { SubMenu::link(submenu, return_state, action_return_state) }
    }

    /// Same as [`link_from_rows`], but action rows return to the submenu itself
    /// rather than to a separate state. Used by self-contained toggles.
    pub(crate) unsafe fn link_from_rows_self_return(
        rows: &[(i32, i32, bool, Option<SubMenuAction>)],
        return_state: *mut State,
    ) -> *mut State {
        let submenu = Box::into_raw(Box::new(SubMenu::new(rows)));
        let self_state = unsafe { std::ptr::addr_of_mut!((*submenu).state) };
        unsafe { SubMenu::link(submenu, return_state, self_state) }
    }

    /// Like [`link_from_rows_self_return`], but also returns the leaked
    /// `*mut SubMenu` so the caller can re-target rows (e.g. to wire nested
    /// submenus via [`SubMenu::set_option_target`]).
    pub(crate) unsafe fn link_from_rows_self_return_with_ptr(
        rows: &[(i32, i32, bool, Option<SubMenuAction>)],
        return_state: *mut State,
    ) -> (*mut SubMenu, *mut State) {
        let submenu = Box::into_raw(Box::new(SubMenu::new(rows)));
        let self_state = unsafe { std::ptr::addr_of_mut!((*submenu).state) };
        let state = unsafe { SubMenu::link(submenu, return_state, self_state) };
        (submenu, state)
    }

    /// Overrides the transition target of a row that was set during `link`.
    /// Used to wire nested submenus: after linking, point a no-action row at
    /// another submenu state instead of `return_state`.
    pub(crate) unsafe fn set_option_target(
        ptr: *mut SubMenu,
        index: usize,
        target_state: *mut State,
    ) {
        let this = unsafe { &mut *ptr };
        if let Some(opt) = this.options.get_mut(index) {
            opt.transition.target_state = target_state;
        }
    }
}

// ---- Action states ----

/// Builds a one-shot state that runs `action` when the state machine enters it
/// (via the `ACTIONS` registry), then transitions to `return_state`. Leaked;
/// returns a pointer to the state.
#[allow(dead_code)]
pub(crate) unsafe fn make_action_state(
    action: SubMenuAction,
    return_state: *mut State,
) -> *mut State {
    let target = Box::into_raw(Box::new(ActionTarget::new(return_state)));
    unsafe { (*target).link() };
    let action_state = unsafe { &mut (*target).state } as *mut State;
    register_action(action_state, action);
    action_state
}

// ---- The yes/no dialog ----

/// A runtime-constructed yes/no confirmation dialog state, mirroring the game's
/// own dialog states (e.g. the flask upgrade confirmations) byte for byte: the
/// entry calls `6:2147483647` (the generic `OpenGenericDialog` sub-group) with
/// `message_id`; the `#B9 == 0` OK branch leads to `yes_target` and the
/// `#B9 != #BA` cancel branch to `no_target`.
#[allow(dead_code)]
pub(crate) struct YesNoDialog {
    message_expr: [u8; 6],
    message_args: [Span<u8>; 1],
    open_event: Event,
    ok_expr: [u8; 4],     // #B9 == 0
    cancel_expr: [u8; 4], // #B9 != #BA
    true_expr: [u8; 2],   // if 1
    ok_sub: Transition,
    cancel_sub: Transition,
    ok_sub_arr: [*mut Transition; 1],
    cancel_sub_arr: [*mut Transition; 1],
    ok_transition: Transition,
    cancel_transition: Transition,
    transitions: [*mut Transition; 2],
    state: State,
}

#[allow(dead_code)]
impl YesNoDialog {
    pub(crate) fn new(message_id: i32, yes_target: *mut State, no_target: *mut State) -> Self {
        Self {
            message_expr: make_int_expression(message_id),
            message_args: [Span::null(); 1],
            open_event: Event {
                command: OPEN_GENERIC_DIALOG,
                args: Span::null(),
            },
            ok_expr: [0xb9, 0x40, 0x95, 0xa1],
            cancel_expr: [0xb9, 0xba, 0x96, 0xa1],
            true_expr: [0x41, 0xa1],
            ok_sub: Transition {
                target_state: yes_target,
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            cancel_sub: Transition {
                target_state: no_target,
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            ok_sub_arr: [ptr::null_mut()],
            cancel_sub_arr: [ptr::null_mut()],
            ok_transition: Transition {
                target_state: ptr::null_mut(),
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            cancel_transition: Transition {
                target_state: ptr::null_mut(),
                pass_events: Span::null(),
                sub_transitions: Span::null(),
                evaluator: Span::null(),
            },
            transitions: [ptr::null_mut(); 2],
            state: State {
                id: 0,
                transitions: Span::null(),
                entry_events: Span::null(),
                exit_events: Span::null(),
                while_events: Span::null(),
            },
        }
    }

    /// Fixes up every self-referential span now that the dialog lives at a
    /// stable (leaked heap) address. Setting these before the move would leave
    /// them pointing at the dead stack copy of the struct.
    unsafe fn link(&mut self) {
        self.message_args = [Span {
            ptr: self.message_expr.as_mut_ptr(),
            len: self.message_expr.len(),
        }];
        self.open_event.args = Span {
            ptr: self.message_args.as_mut_ptr(),
            len: self.message_args.len(),
        };

        self.ok_sub.evaluator = Span {
            ptr: self.true_expr.as_mut_ptr(),
            len: self.true_expr.len(),
        };
        self.cancel_sub.evaluator = Span {
            ptr: self.true_expr.as_mut_ptr(),
            len: self.true_expr.len(),
        };
        self.ok_sub_arr = [&mut self.ok_sub as *mut Transition];
        self.cancel_sub_arr = [&mut self.cancel_sub as *mut Transition];

        self.ok_transition.evaluator = Span {
            ptr: self.ok_expr.as_mut_ptr(),
            len: self.ok_expr.len(),
        };
        self.ok_transition.sub_transitions = Span {
            ptr: self.ok_sub_arr.as_mut_ptr(),
            len: self.ok_sub_arr.len(),
        };
        self.cancel_transition.evaluator = Span {
            ptr: self.cancel_expr.as_mut_ptr(),
            len: self.cancel_expr.len(),
        };
        self.cancel_transition.sub_transitions = Span {
            ptr: self.cancel_sub_arr.as_mut_ptr(),
            len: self.cancel_sub_arr.len(),
        };

        self.transitions = [
            &mut self.ok_transition as *mut Transition,
            &mut self.cancel_transition as *mut Transition,
        ];
        self.state.transitions = Span {
            ptr: self.transitions.as_mut_ptr(),
            len: self.transitions.len(),
        };
        self.state.entry_events = Span {
            ptr: &self.open_event as *const Event as *mut Event,
            len: 1,
        };
    }

    /// Leaks the dialog and returns a pointer to its state.
    pub(crate) unsafe fn into_state_ptr(dialog: Box<YesNoDialog>) -> *mut State {
        let ptr = Box::into_raw(dialog);
        unsafe { (*ptr).link() };
        (unsafe { &mut (*ptr).state }) as *mut State
    }
}

// ---- Patching ----

/// Adds a menu option row to a grace state group's menu state and inserts its
/// transition into the dispatch state, just before the default (`if 1`)
/// fallback. The dispatch state is identified structurally by the
/// `SetREG0(GetTalkListEntryResult())` evaluator prefix rather than any single
/// menu's dispatch target. The option opens `target_state` when selected.
/// Returns true if the state group was patched; the already-patched check (an
/// existing `AddTalkListData` with `message_id`) makes later calls a no-op.
pub(crate) unsafe fn splice_option(
    state_group: *mut StateGroup,
    option_index: i32,
    message_id: i32,
    target_state: *mut State,
) -> bool {
    fn anchor(event: &Event) -> bool {
        unsafe { is_sort_chest_event(event) }
    }
    unsafe { splice_option_anchored(state_group, option_index, message_id, target_state, anchor) }
}

/// Like [`splice_option`], but anchors on any state whose entry events contain
/// a plain `AddTalkListData` command (e.g. the Roundtable Hold mirror menu),
/// rather than the grace menu's SortChest row.
pub(crate) unsafe fn splice_talk_list_option(
    state_group: *mut StateGroup,
    option_index: i32,
    message_id: i32,
    target_state: *mut State,
) -> bool {
    fn anchor(event: &Event) -> bool {
        event.command == ADD_TALK_LIST_DATA
    }
    unsafe { splice_option_anchored(state_group, option_index, message_id, target_state, anchor) }
}

unsafe fn splice_option_anchored(
    state_group: *mut StateGroup,
    option_index: i32,
    message_id: i32,
    target_state: *mut State,
    anchor: fn(&Event) -> bool,
) -> bool {
    let states = unsafe { slice_of((*state_group).states) };

    let mut add_menu_state: Option<*mut State> = None;
    let mut event_index = -1i32;
    let mut dispatch_state: Option<*mut State> = None;

    for state in states {
        let state_ptr = state as *const State as *mut State;

        for (i, event) in unsafe { slice_of(state.entry_events) }.iter().enumerate() {
            if anchor(event) {
                add_menu_state = Some(state_ptr);
                event_index = i as i32;
            }
        }

        if dispatch_state.is_none() {
            for transition in unsafe { slice_of(state.transitions) } {
                if unsafe { is_talk_list_result_transition(*transition) } {
                    dispatch_state = Some(state_ptr);
                    break;
                }
            }
        }
    }

    let Some(add_menu_state) = add_menu_state else {
        return false;
    };
    let Some(dispatch_state) = dispatch_state else {
        return false;
    };
    if event_index == -1 {
        return false;
    }

    // Build and leak the main menu option that opens the submenu.
    let option = Box::into_raw(Box::new(MenuOption::new(
        option_index,
        message_id,
        false,
        None,
    )));
    unsafe {
        (*option).link();
        (*option).transition.target_state = target_state;
    }

    // Append our AddTalkListData event to the menu state's entry events.
    let old_events = unsafe { slice_of((*add_menu_state).entry_events) };
    let mut new_events: Vec<Event> = old_events.to_vec();
    new_events.push(unsafe { (*option).add_talk_list_data_event() });
    let event_count = new_events.len();
    let events_ptr = Box::into_raw(new_events.into_boxed_slice()) as *mut Event;
    unsafe {
        (*add_menu_state).entry_events = Span {
            ptr: events_ptr,
            len: event_count,
        };
    }

    // Insert our transition into the dispatch state before the `if 1` default
    // fallback so the numeric row checks keep their precedence.
    let old_transitions = unsafe { slice_of((*dispatch_state).transitions) };
    let mut insert_at = old_transitions.len();
    for (i, transition) in old_transitions.iter().enumerate() {
        if unsafe { get_ezstate_int_value((**transition).evaluator) } == 1 {
            insert_at = i;
            break;
        }
    }
    let mut new_transitions: Vec<*mut Transition> = Vec::with_capacity(old_transitions.len() + 1);
    new_transitions.extend_from_slice(&old_transitions[..insert_at]);
    new_transitions.push(unsafe { (*option).transition_ptr() });
    new_transitions.extend_from_slice(&old_transitions[insert_at..]);
    let transition_count = new_transitions.len();
    let transitions_ptr = Box::into_raw(new_transitions.into_boxed_slice()) as *mut *mut Transition;
    unsafe {
        (*dispatch_state).transitions = Span {
            ptr: transitions_ptr,
            len: transition_count,
        };
    }

    true
}

// ---- Transition redirection ----

/// Redirects a transition's target state by rewriting the target pointer in
/// place. The evaluator, pass events and sub-transitions are left untouched.
pub(crate) unsafe fn redirect_transition(
    transition: *mut Transition,
    new_target: *mut State,
) -> bool {
    if transition.is_null() || new_target.is_null() {
        return false;
    }
    unsafe {
        (*transition).target_state = new_target;
    }
    true
}

// ---- Structural state group queries ----

/// The group id read from the ESD file (`state_group.id`).
pub(crate) unsafe fn state_group_id(state_group: *mut StateGroup) -> Option<i32> {
    if state_group.is_null() {
        return None;
    }
    Some(unsafe { (*state_group).id })
}

/// The target of a transition's highest-priority path: the transition's own
/// target if set, otherwise the leaf target reached by following the first
/// sub-transition at each level.
pub(crate) unsafe fn leaf_transition_target(transition: *mut Transition) -> Option<*mut State> {
    if transition.is_null() {
        return None;
    }
    let target = unsafe { (*transition).target_state };
    if !target.is_null() {
        return Some(target);
    }
    let sub_transitions = unsafe { slice_of((*transition).sub_transitions) };
    if sub_transitions.is_empty() {
        return None;
    }
    unsafe { leaf_transition_target(sub_transitions[0]) }
}

/// True if the transition's own target, or any nested sub-transition's sub-tree,
/// reaches `target`.
pub(crate) unsafe fn transition_reaches(transition: *mut Transition, target: *mut State) -> bool {
    if transition.is_null() || target.is_null() {
        return false;
    }
    if unsafe { (*transition).target_state } == target {
        return true;
    }
    for sub in unsafe { slice_of((*transition).sub_transitions) } {
        if unsafe { transition_reaches(*sub, target) } {
            return true;
        }
    }
    false
}

/// The state at array position `index` within the group's state list.
pub(crate) unsafe fn state_at_index(
    state_group: *mut StateGroup,
    index: usize,
) -> Option<*mut State> {
    if state_group.is_null() {
        return None;
    }
    let states = unsafe { slice_of((*state_group).states) };
    states.get(index).map(|s| s as *const State as *mut State)
}

/// True if the state is a plain pass-through: exactly one transition whose
/// evaluator is constant-true, with a direct target and no sub-transitions.
pub(crate) unsafe fn is_plain_true_state(state: *mut State) -> bool {
    if state.is_null() {
        return false;
    }
    let transitions = unsafe { slice_of((*state).transitions) };
    if transitions.len() != 1 {
        return false;
    }
    let transition = transitions[0];
    if transition.is_null() || unsafe { (*transition).target_state.is_null() } {
        return false;
    }
    if !unsafe { slice_of((*transition).sub_transitions) }.is_empty() {
        return false;
    }
    unsafe { get_ezstate_int_value((*transition).evaluator) == 1 }
}

/// The confirm-dialog state of a flask group: the state whose highest-priority
/// branch (`#B9 == 0`) resolves to `ok_target`. Returns the state and its OK
/// transition.
pub(crate) unsafe fn find_dialog_state(
    state_group: *mut StateGroup,
    ok_target: *mut State,
) -> Option<(*mut State, *mut Transition)> {
    if state_group.is_null() || ok_target.is_null() {
        return None;
    }
    for state in unsafe { slice_of((*state_group).states) } {
        for transition in unsafe { slice_of(state.transitions) } {
            if unsafe { leaf_transition_target(*transition) } != Some(ok_target) {
                continue;
            }
            let evaluator = unsafe { (**transition).evaluator };
            if !evaluator.ptr.is_null() && evaluator.len >= 1 && unsafe { *evaluator.ptr == 0xB9 } {
                return Some((state as *const State as *mut State, *transition));
            }
        }
    }
    None
}

/// The first transition owned by a state other than `target` whose sub-tree
/// (own target or nested sub-transitions) reaches `target`. Returns the
/// top-level transition so it can be rewritten in place.
pub(crate) unsafe fn incoming_edge(
    state_group: *mut StateGroup,
    target: *mut State,
) -> Option<*mut Transition> {
    if state_group.is_null() || target.is_null() {
        return None;
    }
    for state in unsafe { slice_of((*state_group).states) } {
        if std::ptr::eq(state, target) {
            continue;
        }
        for transition in unsafe { slice_of(state.transitions) } {
            if unsafe { transition_reaches(*transition, target) } {
                return Some(*transition);
            }
        }
    }
    None
}

/// A compact dump of a state group's runtime layout, used once per flask group
/// to diagnose how commands and transitions are stored in memory.
pub(crate) unsafe fn dump_state_group(state_group: *mut StateGroup) -> String {
    if state_group.is_null() {
        return "null".to_string();
    }
    let id = unsafe { (*state_group).id };
    let states = unsafe { slice_of((*state_group).states) };
    let mut out = format!("group {id} states.len={}", states.len());
    for (i, state) in states.iter().take(30).enumerate() {
        let events = unsafe { slice_of(state.entry_events) }
            .iter()
            .map(|e| format!("{}:{}", e.command.bank, e.command.id))
            .collect::<Vec<_>>()
            .join(",");
        let transitions = unsafe { slice_of(state.transitions) }
            .iter()
            .map(|t| {
                let t = *t;
                if t.is_null() {
                    return "null".to_string();
                }
                let target = unsafe { (*t).target_state };
                let subs = unsafe { slice_of((*t).sub_transitions) }.len();
                let evaluator = unsafe { (*t).evaluator };
                let prefix = if evaluator.ptr.is_null() || evaluator.len == 0 {
                    "[]".to_string()
                } else {
                    let n = evaluator.len.min(8);
                    let bytes = unsafe { std::slice::from_raw_parts(evaluator.ptr, n) };
                    bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                format!("tgt={target:p} subs={subs} eval[{prefix}]")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        out.push_str(&format!(
            "\n  st[{i}] id={} ev=[{events}] trans=[{transitions}]",
            state.id
        ));
    }
    out
}

// ---- Feature registry ----

type Patcher = unsafe fn(*mut StateGroup) -> bool;

static PATCHERS: Mutex<Vec<Patcher>> = Mutex::new(Vec::new());
static GROUP_PATCHERS: Mutex<Vec<Patcher>> = Mutex::new(Vec::new());
static MESSAGES: Mutex<Vec<RegisteredMessage>> = Mutex::new(Vec::new());

const MESSAGE_CAPACITY: usize = 128;

struct RegisteredMessage {
    bnd: u32,
    id: i32,
    text: &'static mut [u16],
}

/// A callback invoked on the game's main thread when the player selects a
/// submenu option wired to an action.
pub(crate) type SubMenuAction = unsafe extern "C" fn();

/// Maps a mod-owned action state's address to the callback to run when the
/// state machine enters it.
static ACTIONS: Mutex<Vec<(usize, SubMenuAction)>> = Mutex::new(Vec::new());

fn register_action(state: *mut State, action: SubMenuAction) {
    ACTIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((state as usize, action));
}

/// Registers a patch routine that is run whenever a grace menu's initial state
/// is entered. The routine must be idempotent (return false once patched).
pub(crate) fn register_patcher(patcher: Patcher) {
    PATCHERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(patcher);
}

/// Registers a patch routine that is run whenever *any* state group's initial
/// state is entered, grace menus or not. The routine must check the group id
/// itself and be idempotent.
pub(crate) fn register_group_patcher(patcher: Patcher) {
    GROUP_PATCHERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(patcher);
}

/// Registers a message text that `MsgRepositoryImp::LookupEntry` returns for
/// `message_id` in message bound `bnd`. The text is UTF-16 encoded and
/// NUL-terminated here, since the game reads it as a C-style wide string.
pub(crate) fn register_message_in_bnd(bnd: u32, message_id: i32, text: &str) {
    let text = encode_message_text(text);
    MESSAGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(RegisteredMessage {
            bnd,
            id: message_id,
            text,
        });
}

/// Updates a previously registered message text in the menu text bound
/// (bound 33). Reuses the existing buffer if the new text fits, otherwise
/// leaks a new one.
pub(crate) fn update_message(message_id: i32, text: &str) {
    update_message_in_bnd(MSGBND_EVENT_TEXT_FOR_TALK, message_id, text);
}

/// Updates a previously registered message text in the given bound.
pub(crate) fn update_message_in_bnd(bnd: u32, message_id: i32, text: &str) {
    let mut chars: Vec<u16> = text.encode_utf16().collect();
    chars.push(0);
    let mut messages = MESSAGES.lock().unwrap_or_else(|e| e.into_inner());
    for entry in messages.iter_mut() {
        if entry.bnd == bnd && entry.id == message_id {
            if chars.len() <= entry.text.len() {
                entry.text[..chars.len()].copy_from_slice(&chars);
                // NUL-terminate at the new length in case the old text was longer.
                entry.text[chars.len() - 1] = 0;
            } else {
                entry.text = Box::leak(chars.into_boxed_slice());
            }
            return;
        }
    }
}

fn encode_message_text(text: &str) -> &'static mut [u16] {
    let mut chars: Vec<u16> = text.encode_utf16().collect();
    chars.push(0);
    if chars.len() < MESSAGE_CAPACITY {
        chars.resize(MESSAGE_CAPACITY, 0);
    }
    Box::leak(chars.into_boxed_slice())
}

/// Registers a message text that `MsgRepositoryImp::LookupEntry` returns for
/// `message_id` (talk message bound 33).
pub(crate) fn register_message(message_id: i32, text: &str) {
    register_message_in_bnd(MSGBND_EVENT_TEXT_FOR_TALK, message_id, text);
}

// ---- Hooks ----

/// Diagnostic logger: logs every talk-script state entry (group id in the
/// 2147483xxx talk range) so merchant conversations can be correlated against
/// runtime machine/group pointers. Set to `false` to disable.
const DIAG_LOG_STATE_ENTRIES: bool = true;

fn ezstate_enter_state_detour(regs: *mut Registers, original: usize) -> usize {
    let state = unsafe { (*regs).rcx } as *mut State;
    let machine = unsafe { (*regs).rdx } as *mut Machine;

    if !machine.is_null() {
        unsafe {
            let state_group = (*machine).state_group;
            if !state_group.is_null() {
                if DIAG_LOG_STATE_ENTRIES && (*state_group).id > 2147480000 {
                    let state_id = if state.is_null() { -1 } else { (*state).id };
                    let initial_state_id = if (*state_group).initial_state.is_null() {
                        -1
                    } else {
                        (*(*state_group).initial_state).id
                    };
                    log(&format!(
                        "ezstate_diag: group_id={} state_id={} init={}",
                        (*state_group).id,
                        state_id,
                        initial_state_id,
                    ));
                    // Log all entry events so we can see OpenRegularShop
                    // ranges, AddTalkListData message IDs, etc.
                    if !state.is_null() {
                        let events = slice_of((*state).entry_events);
                        for (i, event) in events.iter().enumerate() {
                            let mut args_str = String::new();
                            for ai in 0..4 {
                                let v = event_arg_int(event, ai);
                                if v == -1 {
                                    break;
                                }
                                if ai > 0 {
                                    args_str.push(',');
                                }
                                args_str.push_str(&v.to_string());
                            }
                            log(&format!(
                                "  [{}] {}:{}({})",
                                i, event.command.bank, event.command.id, args_str,
                            ));
                        }
                    }
                }

                if state == (*state_group).initial_state {
                    if is_grace_state_group(state_group) {
                        let patchers = PATCHERS.lock().unwrap_or_else(|e| e.into_inner());
                        for patcher in patchers.iter() {
                            if patcher(state_group) {
                                log("ezstate_menu: patched site of grace menu");
                            }
                        }
                    }

                    let group_patchers = GROUP_PATCHERS.lock().unwrap_or_else(|e| e.into_inner());
                    for patcher in group_patchers.iter() {
                        if patcher(state_group) {
                            log("ezstate_menu: patched state group transitions");
                        }
                    }
                }
            }
        }
    }

    let original_fn: extern "C" fn(*mut State, *mut Machine, u64) =
        unsafe { std::mem::transmute(original) };
    unsafe { original_fn(state, machine, (*regs).r8) };

    // Run the registered action for a mod-owned action state, if any.
    if !state.is_null() {
        let actions = ACTIONS.lock().unwrap_or_else(|e| e.into_inner());
        for &(addr, action) in actions.iter() {
            if state as usize == addr {
                unsafe { action() };
                break;
            }
        }
    }
    0
}

/// Address of the relocated LookupEntry prologue (see
/// [`install_lookup_entry_hook_race_safe`]). Set before the entry patch goes
/// live so the detour can always forward misses.
static LOOKUP_TRAMPOLINE: OnceLock<usize> = OnceLock::new();

unsafe extern "system" fn lookup_entry_detour(
    repo: *const core::ffi::c_void,
    language: u32,
    bnd: u32,
    msg_id: i32,
) -> *mut u16 {
    let messages = MESSAGES.lock().unwrap_or_else(|e| e.into_inner());
    for msg in messages.iter() {
        if msg.bnd == bnd && msg.id == msg_id {
            if bnd == MSGBND_EVENT_TEXT_FOR_TALK && (69_990_000..=69_990_100).contains(&msg_id) {
                log(&format!(
                    "ezstate_menu: LookupEntry served bnd={bnd} msg_id={msg_id} text_len={}",
                    msg.text.len()
                ));
            }
            if bnd == MSGBND_PLACE_NAME {
                log(&format!(
                    "map_icons: label served PlaceName bnd={bnd} msg_id={msg_id} text_len={}",
                    msg.text.len()
                ));
            }
            return msg.text.as_ptr() as *mut u16;
        }
    }
    drop(messages);

    if bnd == MSGBND_EVENT_TEXT_FOR_TALK && (69_990_000..=69_990_100).contains(&msg_id) {
        log(&format!(
            "ezstate_menu: LookupEntry fell through bnd={bnd} msg_id={msg_id}"
        ));
    }

    let trampoline = *LOOKUP_TRAMPOLINE.get().unwrap_or(&0) as *const core::ffi::c_void;
    if trampoline.is_null() {
        return ptr::null_mut();
    }
    let original_fn: extern "system" fn(*const core::ffi::c_void, u32, u32, i32) -> *mut u16 =
        unsafe { std::mem::transmute(trampoline) };
    original_fn(repo, language, bnd, msg_id)
}

// ---- Installer ----

pub(crate) static MENU_INSTALLER: Once = Once::new();

/// Installs the two hooks that drive every registered feature. Must be called
/// after the features have registered their patchers and messages.
pub(crate) fn install() {
    if crate::config::with_feature(|cfg| cfg.hook_enter_state) {
        install_enter_state_hook();
    } else {
        log("ezstate_menu: EzState::EnterState hook disabled by config (debug A/B)");
    }
    if crate::config::with_feature(|cfg| cfg.hook_lookup_entry) {
        install_lookup_entry_hook();
    } else {
        log("ezstate_menu: MsgRepositoryImp::LookupEntry hook disabled by config (debug A/B)");
    }
}

fn install_enter_state_hook() {
    let Some(enter_state) = scan::scan_pattern_call(ENTER_STATE_PATTERN) else {
        log("ezstate_menu: ERROR: EzState::EnterState signature not found");
        return;
    };
    log(&format!(
        "ezstate_menu: EzState::EnterState at {enter_state:#x}"
    ));
    hooks::install_retn(
        "ezstate_menu: EzState::EnterState",
        enter_state,
        ezstate_enter_state_detour,
    );
}

fn install_lookup_entry_hook() {
    let Some(lookup_entry) = scan::scan_pattern_call(LOOKUP_ENTRY_PATTERN) else {
        log("ezstate_menu: ERROR: MsgRepositoryImp::LookupEntry signature not found");
        return;
    };
    log(&format!(
        "ezstate_menu: MsgRepositoryImp::LookupEntry at {lookup_entry:#x}"
    ));
    if !install_lookup_entry_hook_race_safe(lookup_entry) {
        log("ezstate_menu: ERROR: LookupEntry hook not installed");
    }
}

/// Installs the LookupEntry hook without ilhook. ilhook places its trampoline
/// on the Rust heap (any distance), forcing a 14-byte `FF 25 <abs8>` entry
/// patch whose non-atomic write races LookupEntry's parallel loading threads —
/// deterministically torn when SeamlessCoop shifts startup timing. Instead we
/// patch with a 5-byte `E9` (opcode written last), a near absolute-jump stub
/// for the detour, and a relocated prologue trampoline. If another hook
/// (e.g. coop's) already owns the entry, we chain behind it so its behaviour
/// is preserved rather than clobbered.
fn install_lookup_entry_hook_race_safe(entry: u64) -> bool {
    // True vanilla prologue, verified with r2 at 0x14266fbd0 in the current
    // 1.17 exe: `cmp edx,[rcx+0x10]; jae; cmp r8d,[rcx+0x14]; jae;
    // mov rax,[rcx+8]`.
    let expected = [
        0x3b, 0x51, 0x10, 0x73, 0x29, 0x44, 0x3b, 0x41, 0x14, 0x73, 0x23, 0x48, 0x8b, 0x41, 0x08,
    ];

    // Pristine trampoline: the 5-byte window (`cmp edx,[rcx+0x10]; jae` at
    // entry+0..5) is relocated verbatim, except the short `jae +0x29` becomes
    // an absolute branch to entry+0x2e (the shared return-null exit); the
    // trampoline then jumps back at entry+5, the start of the untouched
    // second arg check (`cmp r8d,[rcx+0x14]`).
    let build_trampoline = |_orig: &[u8]| -> Option<u64> {
        let jae_target = entry + 0x2e;
        let jump_back = entry + 5;
        let mut trampoline = Vec::with_capacity(32);
        trampoline.extend_from_slice(&[0x3b, 0x51, 0x10]); // cmp edx, [rcx+0x10]
        trampoline.extend_from_slice(&[0x0f, 0x83, 0, 0, 0, 0]); // jae rel32
        trampoline.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]); // jmp [rip+disp32]
        trampoline.extend_from_slice(&jump_back.to_le_bytes());
        let Some(addr) = (unsafe { memory::alloc_executable(entry, trampoline.len()) }) else {
            log("ezstate_menu: ERROR: failed to allocate LookupEntry trampoline");
            return None;
        };
        let rel = (jae_target as i64).wrapping_sub((addr + 9) as i64) as i32;
        trampoline[5..9].copy_from_slice(&rel.to_le_bytes());
        unsafe {
            std::ptr::copy_nonoverlapping(trampoline.as_ptr(), addr as *mut u8, trampoline.len());
        }
        log(&format!(
            "ezstate_menu: LookupEntry pristine; trampoline at {addr:#x} jae={jae_target:#x} bytes={trampoline:02x?}"
        ));
        Some(addr)
    };

    hooks::install_e9_entry_race_safe(
        "ezstate_menu: MsgRepositoryImp::LookupEntry",
        entry,
        &expected,
        build_trampoline,
        lookup_entry_detour as *const () as usize,
        &LOOKUP_TRAMPOLINE,
    )
}
