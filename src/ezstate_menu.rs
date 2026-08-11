use std::ptr;
use std::sync::{Mutex, Once};

use ilhook::x64::{hook_closure_retn, CallbackOption, HookFlags, Registers};

use crate::log::log;
use crate::scan;

// ---- Talk command constants (mirrors elden-x talk_commands.hpp) ----

const ADD_TALK_LIST_DATA: Command = Command { bank: 1, id: 19 };
const ADD_TALK_LIST_DATA_IF: Command = Command { bank: 5, id: 19 };
const ADD_TALK_LIST_DATA_ALT: Command = Command { bank: 5, id: 149 };
const CLOSE_SHOP_MESSAGE: Command = Command { bank: 1, id: 12 };
const CLEAR_TALK_LIST_DATA: Command = Command { bank: 1, id: 20 };
const SHOW_SHOP_MESSAGE: Command = Command { bank: 1, id: 10 };
const OPEN_REPOSITORY: Command = Command { bank: 1, id: 30 };

const MSGBND_EVENT_TEXT_FOR_TALK: u32 = 33;

const MSG_SORT_CHEST: i32 = 15000395;

// ---- AOB signatures (from elden-ring-transmog) ----
// Both end in `e8 $ '` so scan_pattern_call resolves the function the call
// targets rather than the call site itself.

const ENTER_STATE_PATTERN: &str =
    "80 7e 18 00 74 15 4c 8d 44 24 40 48 8b d6 48 8b 4e 20 e8 $ '";

const LOOKUP_ENTRY_PATTERN: &str = "8b da 44 8b ca 33 d2 48 8b f9 44 8d 42 6f e8 $ '";

// ---- Raw ESD structs (mirrors elden-x er::ezstate/ezstate.hpp) ----

#[repr(C)]
#[derive(Clone, Copy)]
struct Span<T> {
    ptr: *mut T,
    len: usize,
}

impl<T> Span<T> {
    fn null() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Command {
    bank: i32,
    id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Event {
    command: Command,
    args: Span<Span<u8>>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Transition {
    target_state: *mut State,
    pass_events: Span<Event>,
    sub_transitions: Span<*mut Transition>,
    evaluator: Span<u8>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct State {
    id: i32,
    transitions: Span<*mut Transition>,
    entry_events: Span<Event>,
    exit_events: Span<Event>,
    while_events: Span<Event>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct StateGroup {
    id: i32,
    states: Span<State>,
    pub(crate) initial_state: *mut State,
}

#[repr(C)]
struct Machine {
    vtable: usize,
    unk1: [u8; 0x20],
    state_group: *mut StateGroup,
    unk2: [u8; 0x110],
}

unsafe fn slice_of<T>(span: Span<T>) -> &'static [T] {
    if span.len == 0 || span.ptr.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(span.ptr, span.len) }
    }
}

// ---- ESD expression helpers ----

fn make_int_expression(value: i32) -> [u8; 6] {
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
fn make_talk_list_result_expression(value: i32) -> [u8; 9] {
    [
        0x57, 0x84, 0x82, value as u8, (value >> 8) as u8, (value >> 16) as u8,
        (value >> 24) as u8, 0x95, 0xa1,
    ]
}

/// `(CheckSpecificPersonMenuIsOpen(1, 0) == 1 && CheckSpecificPersonGenericDialogIsOpen(0) == 0) == 0`
/// i.e. fires once the talk list menu has closed.
const TALK_MENU_CLOSED_EXPR: [u8; 15] = [
    0x7b, 0x41, 0x40, 0x86, 0x41, 0x95, // menu open (1, 0) == 1
    0x7a, 0x40, 0x85, 0x40, 0x95,       // generic dialog open (0) == 0
    0x98, 0x40, 0x95, 0xa1,             // && (both) == 0
];

/// Parses an ESD expression containing only a 1 or 4 byte integer.
unsafe fn get_ezstate_int_value(expr: Span<u8>) -> i32 {
    if expr.len == 2 && !expr.ptr.is_null() {
        return unsafe { *expr.ptr } as i32 - 64;
    }
    if expr.len == 6 && !expr.ptr.is_null() && unsafe { *expr.ptr } == 0x82 {
        return unsafe { i32::from_le_bytes(*(expr.ptr.add(1) as *const [u8; 4])) };
    }
    -1
}

unsafe fn event_arg_int(event: &Event, index: usize) -> i32 {
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

/// True if the transition targets a state that opens the storage chest.
unsafe fn is_sort_chest_transition(transition: *mut Transition) -> bool {
    if transition.is_null() {
        return false;
    }
    let target = unsafe { (*transition).target_state };
    if target.is_null() {
        return false;
    }
    let entry_events = unsafe { slice_of((*target).entry_events) };
    !entry_events.is_empty() && entry_events[0].command == OPEN_REPOSITORY
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
    state: State,
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
    pub(crate) unsafe fn link(ptr: *mut SubMenu, return_state: *mut State) -> *mut State {
        let this = unsafe { &mut *ptr };

        for opt in &mut this.options {
            opt.link();
            opt.transition.target_state = match opt.action {
                Some(action) => {
                    let target = Box::into_raw(Box::new(ActionTarget::new(return_state)));
                    unsafe { (*target).link() };
                    let action_state = unsafe { &mut (*target).state } as *mut State;
                    register_action(action_state, action);
                    action_state
                }
                None => return_state,
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
}

// ---- Patching ----

/// Adds a menu option row to a grace state group's menu state and inserts its
/// transition into the dispatch state. The option opens `target_state` when
/// selected. Returns true if the state group was patched; the already-patched
/// check (an existing `AddTalkListData` with `message_id`) makes later calls
/// a no-op.
pub(crate) unsafe fn patch_grace_menu(
    state_group: *mut StateGroup,
    option_index: i32,
    message_id: i32,
    target_state: *mut State,
) -> bool {
    let states = unsafe { slice_of((*state_group).states) };

    let mut add_menu_state: Option<*mut State> = None;
    let mut event_index = -1i32;
    let mut menu_transition_state: Option<*mut State> = None;
    let mut transition_index = -1i32;

    for state in states {
        let state_ptr = state as *const State as *mut State;

        for (i, event) in unsafe { slice_of(state.entry_events) }
            .iter()
            .enumerate()
        {
            if unsafe { is_sort_chest_event(event) } {
                add_menu_state = Some(state_ptr);
                event_index = i as i32;
            } else if event.command == ADD_TALK_LIST_DATA
                && unsafe { event_arg_int(event, 1) } == message_id
            {
                return false;
            }
        }

        for (i, transition) in unsafe { slice_of(state.transitions) }
            .iter()
            .enumerate()
        {
            if unsafe { is_sort_chest_transition(*transition) } {
                menu_transition_state = Some(state_ptr);
                transition_index = i as i32;
                break;
            }
        }
    }

    let Some(add_menu_state) = add_menu_state else { return false };
    let Some(menu_transition_state) = menu_transition_state else { return false };
    if event_index == -1 || transition_index == -1 {
        return false;
    }

    // Build and leak the main menu option that opens the submenu.
    let option = Box::into_raw(Box::new(MenuOption::new(option_index, message_id, false, None)));
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

    // Insert our transition into the dispatch state's transitions.
    let old_transitions = unsafe { slice_of((*menu_transition_state).transitions) };
    let mut new_transitions: Vec<*mut Transition> = Vec::with_capacity(old_transitions.len() + 1);
    new_transitions.extend_from_slice(&old_transitions[..transition_index as usize]);
    new_transitions.push(unsafe { (*option).transition_ptr() });
    new_transitions.extend_from_slice(&old_transitions[transition_index as usize..]);
    let transition_count = new_transitions.len();
    let transitions_ptr = Box::into_raw(new_transitions.into_boxed_slice()) as *mut *mut Transition;
    unsafe {
        (*menu_transition_state).transitions = Span {
            ptr: transitions_ptr,
            len: transition_count,
        };
    }

    true
}

// ---- Feature registry ----

type Patcher = unsafe fn(*mut StateGroup) -> bool;

static PATCHERS: Mutex<Vec<Patcher>> = Mutex::new(Vec::new());
static MESSAGES: Mutex<Vec<(i32, &'static [u16])>> = Mutex::new(Vec::new());

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

/// Registers a message text that `MsgRepositoryImp::LookupEntry` returns for
/// `message_id` (talk message bound 33).
pub(crate) fn register_message(message_id: i32, text: &'static [u16]) {
    MESSAGES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((message_id, text));
}

// ---- Hooks ----

fn ezstate_enter_state_detour(regs: *mut Registers, original: usize) -> usize {
    let state = unsafe { (*regs).rcx } as *mut State;
    let machine = unsafe { (*regs).rdx } as *mut Machine;

    if !machine.is_null() {
        unsafe {
            let state_group = (*machine).state_group;
            if !state_group.is_null() && is_grace_state_group(state_group) {
                if state == (*state_group).initial_state {
                    let patchers = PATCHERS.lock().unwrap_or_else(|e| e.into_inner());
                    for patcher in patchers.iter() {
                        if patcher(state_group) {
                            log("ezstate_menu: patched site of grace menu");
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

fn lookup_entry_detour(regs: *mut Registers, original: usize) -> usize {
    let bnd = unsafe { (*regs).r8 } as u32;
    let msg_id = unsafe { (*regs).r9 } as i32;

    if bnd == MSGBND_EVENT_TEXT_FOR_TALK {
        let messages = MESSAGES.lock().unwrap_or_else(|e| e.into_inner());
        for &(id, text) in messages.iter() {
            if id == msg_id {
                return text.as_ptr() as usize;
            }
        }
    }

    let original_fn: extern "C" fn(*mut core::ffi::c_void, u32, u32, i32) -> *const u16 =
        unsafe { std::mem::transmute(original) };
    original_fn(
        unsafe { (*regs).rcx } as *mut core::ffi::c_void,
        unsafe { (*regs).rdx } as u32,
        bnd,
        msg_id,
    ) as usize
}

// ---- Installer ----

pub(crate) static MENU_INSTALLER: Once = Once::new();

/// Installs the two hooks that drive every registered feature. Must be called
/// after the features have registered their patchers and messages.
pub(crate) fn install() {
    install_enter_state_hook();
    install_lookup_entry_hook();
}

fn install_enter_state_hook() {
    let Some(enter_state) = scan::scan_pattern_call(ENTER_STATE_PATTERN) else {
        log("ezstate_menu: ERROR: EzState::EnterState signature not found");
        return;
    };
    log(&format!("ezstate_menu: EzState::EnterState at {enter_state:#x}"));

    match unsafe {
        hook_closure_retn(
            enter_state as usize,
            ezstate_enter_state_detour,
            CallbackOption::None,
            HookFlags::empty(),
        )
    } {
        Ok(handle) => {
            std::mem::forget(handle);
            log("ezstate_menu: hooked EzState::EnterState");
        }
        Err(err) => log(&format!(
            "ezstate_menu: ERROR: failed to hook EnterState: {err:?}"
        )),
    }
}

fn install_lookup_entry_hook() {
    let Some(lookup_entry) = scan::scan_pattern_call(LOOKUP_ENTRY_PATTERN) else {
        log("ezstate_menu: ERROR: MsgRepositoryImp::LookupEntry signature not found");
        return;
    };
    log(&format!(
        "ezstate_menu: MsgRepositoryImp::LookupEntry at {lookup_entry:#x}"
    ));

    match unsafe {
        hook_closure_retn(
            lookup_entry as usize,
            lookup_entry_detour,
            CallbackOption::None,
            HookFlags::empty(),
        )
    } {
        Ok(handle) => {
            std::mem::forget(handle);
            log("ezstate_menu: hooked MsgRepositoryImp::LookupEntry");
        }
        Err(err) => log(&format!(
            "ezstate_menu: ERROR: failed to hook LookupEntry: {err:?}"
        )),
    }
}
