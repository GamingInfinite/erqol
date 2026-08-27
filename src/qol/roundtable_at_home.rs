use std::ptr;
use std::sync::LazyLock;

use crate::config;
use crate::ezstate_menu::{
    alloc_message_id, COMBINE_MENU_FLAG_AND_EVENT_FLAG, Event, OPEN_BUDDY_UPGRADE_MENU,
    OPEN_ENHANCE_SHOP, OPEN_EQUIPMENT_CHANGE_OF_PURPOSE_SHOP, OPEN_REGULAR_SHOP, OPEN_SELL_SHOP,
    Span, State, StateGroup, SubMenu, Transition, make_int_expression, make_menu_closed_expr,
    register_message, register_patcher, splice_option,
};
use crate::log::log;

// ---- Message IDs ----
//
// Allocated from the shared allocator so they never collide with another module.

static MSG_ROUNDTABLE: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_HEWG: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_RODERIKA: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_TMH: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_ENHANCE: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_AOW: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_SPIRIT_TUNING: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_BUY: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_SELL: LazyLock<i32> = LazyLock::new(alloc_message_id);
const MSG_CANCEL: i32 = 69_990_003;

const OPTION_INDEX: i32 = 71;

// ---- Menu types for closed expressions ----

const MENU_TYPE_ENHANCE: i32 = 9;
const MENU_TYPE_AOW: i32 = 7;
const MENU_TYPE_BUDDY: i32 = 23;
const MENU_TYPE_SHOP: i32 = 5;
const MENU_TYPE_SELL: i32 = 6;

// ---- Twin Maiden Husks shop range ----

const TMH_SHOP_BEGIN: i32 = 101800;
const TMH_SHOP_END: i32 = 101897;

// ---- Command state ----
//
// A one-shot ESD state that fires prep commands then a talk command
// (e.g. `CombineMenuFlagAndEventFlag` × 4, then `OpenEnhanceShop(0)`)
// on entry, waits for the opened menu to close, then transitions back
// to `return_state`.  Mirrors `ShopState` in `anti_farm_shop.rs`.

struct CommandState {
    prep_arg_counts: Vec<usize>,
    prep_arg_bufs: Vec<[u8; 6]>,
    prep_arg_spans: Vec<Span<u8>>,
    prep_events: Vec<Event>,
    arg_bufs: Vec<[u8; 6]>,
    arg_spans: Vec<Span<u8>>,
    open_event: Event,
    all_events: Vec<Event>,
    closed_expr: [u8; 15],
    return_transition: Transition,
    transition_arr: [*mut Transition; 1],
    state: State,
}

impl CommandState {
    fn new(
        prep: &[(crate::ezstate_menu::Command, &[i32])],
        command: crate::ezstate_menu::Command,
        args: &[i32],
        menu_type: i32,
        return_state: *mut State,
    ) -> Self {
        Self {
            prep_arg_counts: prep.iter().map(|(_, a)| a.len()).collect(),
            prep_arg_bufs: prep
                .iter()
                .flat_map(|(_, a)| a.iter().copied())
                .map(|v| make_int_expression(v))
                .collect(),
            prep_arg_spans: Vec::new(),
            prep_events: prep
                .iter()
                .map(|&(cmd, _)| Event {
                    command: cmd,
                    args: Span::null(),
                })
                .collect(),
            arg_bufs: args.iter().map(|&v| make_int_expression(v)).collect(),
            arg_spans: Vec::new(),
            open_event: Event {
                command,
                args: Span::null(),
            },
            all_events: Vec::new(),
            closed_expr: make_menu_closed_expr(menu_type),
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

    fn link(&mut self) {
        // Build prep event arg spans (contiguous, each prep event points
        // into the right offset).
        self.prep_arg_spans = self
            .prep_arg_bufs
            .iter_mut()
            .map(|buf| Span {
                ptr: buf.as_mut_ptr(),
                len: buf.len(),
            })
            .collect();

        // Wire each prep event's args span to its slice of prep_arg_spans.
        let mut offset = 0usize;
        for (i, &count) in self.prep_arg_counts.iter().enumerate() {
            if count > 0 {
                self.prep_events[i].args = Span {
                    ptr: unsafe { self.prep_arg_spans.as_mut_ptr().add(offset) },
                    len: count,
                };
            }
            offset += count;
        }

        // Build main command arg spans.
        self.arg_spans = self
            .arg_bufs
            .iter_mut()
            .map(|buf| Span {
                ptr: buf.as_mut_ptr(),
                len: buf.len(),
            })
            .collect();

        if !self.arg_spans.is_empty() {
            self.open_event.args = Span {
                ptr: self.arg_spans.as_mut_ptr(),
                len: self.arg_spans.len(),
            };
        }

        // Combine prep + main into all_events.
        self.all_events = Vec::with_capacity(self.prep_events.len() + 1);
        for e in &self.prep_events {
            self.all_events.push(*e);
        }
        self.all_events.push(self.open_event);

        self.return_transition.evaluator = Span {
            ptr: self.closed_expr.as_mut_ptr(),
            len: self.closed_expr.len(),
        };

        self.transition_arr = [&mut self.return_transition as *mut Transition];

        self.state.transitions = Span {
            ptr: self.transition_arr.as_mut_ptr(),
            len: 1,
        };
        self.state.entry_events = Span {
            ptr: self.all_events.as_mut_ptr(),
            len: self.all_events.len(),
        };
    }
}

/// Builds and leaks a `CommandState`; returns a pointer to its `State`.
unsafe fn make_command_state(
    prep: &[(crate::ezstate_menu::Command, &[i32])],
    command: crate::ezstate_menu::Command,
    args: &[i32],
    menu_type: i32,
    return_state: *mut State,
) -> *mut State {
    let cs = Box::into_raw(Box::new(CommandState::new(
        prep, command, args, menu_type, return_state,
    )));
    unsafe { (*cs).link() };
    (unsafe { &mut (*cs).state }) as *mut State
}

// ---- Patcher ----

unsafe fn patch_grace(state_group: *mut StateGroup) -> bool {
    {
        let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        if !cfg.roundtable_at_home {
            return false;
        }
    }

    unsafe {
        let initial_state = (*state_group).initial_state;
        if initial_state.is_null() {
            return false;
        }

        let hewg_rows: &[(i32, i32, bool, Option<crate::ezstate_menu::SubMenuAction>)] = &[
            (1, *MSG_ENHANCE, false, None),
            (2, *MSG_AOW, false, None),
            (99, MSG_CANCEL, true, None),
        ];
        let roderika_rows: &[(i32, i32, bool, Option<crate::ezstate_menu::SubMenuAction>)] = &[
            (1, *MSG_SPIRIT_TUNING, false, None),
            (99, MSG_CANCEL, true, None),
        ];
        let tmh_rows: &[(i32, i32, bool, Option<crate::ezstate_menu::SubMenuAction>)] = &[
            (1, *MSG_BUY, false, None),
            (2, *MSG_SELL, false, None),
            (99, MSG_CANCEL, true, None),
        ];

        let hewg_menu = Box::into_raw(Box::new(SubMenu::new(hewg_rows)));
        let roderika_menu = Box::into_raw(Box::new(SubMenu::new(roderika_rows)));
        let tmh_menu = Box::into_raw(Box::new(SubMenu::new(tmh_rows)));

        let hewg_state = std::ptr::addr_of_mut!((*hewg_menu).state);
        let roderika_state = std::ptr::addr_of_mut!((*roderika_menu).state);
        let tmh_state = std::ptr::addr_of_mut!((*tmh_menu).state);

        let enhance_prep: &[(crate::ezstate_menu::Command, &[i32])] = &[
            (COMBINE_MENU_FLAG_AND_EVENT_FLAG, &[6001, 232]),
            (COMBINE_MENU_FLAG_AND_EVENT_FLAG, &[6001, 233]),
            (COMBINE_MENU_FLAG_AND_EVENT_FLAG, &[6001, 234]),
            (COMBINE_MENU_FLAG_AND_EVENT_FLAG, &[6001, 235]),
        ];
        let enhance_cmd =
            make_command_state(enhance_prep, OPEN_ENHANCE_SHOP, &[0], MENU_TYPE_ENHANCE, hewg_state);
        let aow_cmd = make_command_state(
            &[],
            OPEN_EQUIPMENT_CHANGE_OF_PURPOSE_SHOP,
            &[],
            MENU_TYPE_AOW,
            hewg_state,
        );
        let buddy_cmd = make_command_state(
            &[],
            OPEN_BUDDY_UPGRADE_MENU,
            &[],
            MENU_TYPE_BUDDY,
            roderika_state,
        );
        let buy_cmd = make_command_state(
            &[],
            OPEN_REGULAR_SHOP,
            &[TMH_SHOP_BEGIN, TMH_SHOP_END],
            MENU_TYPE_SHOP,
            tmh_state,
        );
        let sell_cmd =
            make_command_state(&[], OPEN_SELL_SHOP, &[-1, -1], MENU_TYPE_SELL, tmh_state);

        let top_rows: &[(i32, i32, bool, Option<crate::ezstate_menu::SubMenuAction>)] = &[
            (1, *MSG_HEWG, false, None),
            (2, *MSG_RODERIKA, false, None),
            (3, *MSG_TMH, false, None),
            (99, MSG_CANCEL, true, None),
        ];
        let top_menu = Box::into_raw(Box::new(SubMenu::new(top_rows)));
        let top_state = std::ptr::addr_of_mut!((*top_menu).state);

        let hewg_linked = SubMenu::link(hewg_menu, top_state, ptr::null_mut());
        let roderika_linked = SubMenu::link(roderika_menu, top_state, ptr::null_mut());
        let tmh_linked = SubMenu::link(tmh_menu, top_state, ptr::null_mut());

        SubMenu::set_option_target(hewg_menu, 0, enhance_cmd);
        SubMenu::set_option_target(hewg_menu, 1, aow_cmd);
        SubMenu::set_option_target(roderika_menu, 0, buddy_cmd);
        SubMenu::set_option_target(tmh_menu, 0, buy_cmd);
        SubMenu::set_option_target(tmh_menu, 1, sell_cmd);

        SubMenu::link(top_menu, initial_state, ptr::null_mut());

        SubMenu::set_option_target(top_menu, 0, hewg_linked);
        SubMenu::set_option_target(top_menu, 1, roderika_linked);
        SubMenu::set_option_target(top_menu, 2, tmh_linked);

        log("roundtable_at_home: patched grace menu");
        splice_option(state_group, OPTION_INDEX, *MSG_ROUNDTABLE, top_state)
    }
}

// ---- Installer ----

pub(crate) fn init() {
    register_message(*MSG_ROUNDTABLE, "Roundtable at Home");
    register_message(*MSG_HEWG, "Hewg");
    register_message(*MSG_RODERIKA, "Roderika");
    register_message(*MSG_TMH, "Twin Maiden Husks");
    register_message(*MSG_ENHANCE, "Strengthen Armament");
    register_message(*MSG_AOW, "Ash of War Duplication");
    register_message(*MSG_SPIRIT_TUNING, "Spirit Tuning");
    register_message(*MSG_BUY, "Purchase");
    register_message(*MSG_SELL, "Sell");
    register_message(MSG_CANCEL, "Cancel");

    register_patcher(patch_grace);
}
