use crate::config;
use crate::ezstate_menu::{
    dump_state_group, find_dialog_state, incoming_edge, is_plain_true_state,
    redirect_transition, register_group_patcher, state_at_index, state_group_id, StateGroup,
};
use crate::log::log;
use std::sync::Mutex;

/// Golden-seed flask allocation. The slot picker (state 28) flows into the
/// confirm dialog (state 21); the dialog's OK branch (`#B9 == 0`) leads to
/// state 6.
const FLASK_ALLOC_GROUP: i32 = 2147483640;

/// Sacred-tear flask upgrade. State 19 flows into the confirm dialog
/// (state 20); the dialog's OK branch leads to state 6.
const FLASK_UPGRADE_GROUP: i32 = 2147483641;

/// In both flask groups the confirm dialog's OK branch (`#B9 == 0`) leads to
/// state 6, a plain pass-through that continues the upgrade flow.
const OK_STATE_INDEX: usize = 6;

/// One-shot diagnostics: log the runtime layout of each flask group the first
/// time its initial state is entered.
static DIAGNOSED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Registers this feature's group patcher. Called once, before
/// `ezstate_menu::install()`.
pub(crate) fn init() {
    register_group_patcher(patch);
}

/// Skips the confirmation dialog in a flask flow by redirecting the transition
/// that enters the dialog state straight to the state the dialog's OK branch
/// (`#B9 == 0`) leads to.
///
/// The dialog state is identified without relying on the `6:2147483647` entry
/// command (which the runtime does not expose in `entry_events`): it is the
/// unique state whose highest-priority branch resolves to the group's OK state
/// at array index [`OK_STATE_INDEX`].
unsafe fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.skip_flask_confirm) {
        return false;
    }

    let Some(id) = (unsafe { state_group_id(state_group) }) else {
        return false;
    };
    if id != FLASK_ALLOC_GROUP && id != FLASK_UPGRADE_GROUP {
        return false;
    }

    let mut diagnosed = DIAGNOSED.lock().unwrap_or_else(|e| e.into_inner());
    if !diagnosed.contains(&id) {
        diagnosed.push(id);
        let dump = unsafe { dump_state_group(state_group) };
        log(format!("skip_flask_confirm: runtime layout {dump}"));
    }
    drop(diagnosed);

    let Some(ok_target) = (unsafe { state_at_index(state_group, OK_STATE_INDEX) }) else {
        log(format!(
            "skip_flask_confirm: no state at OK_STATE_INDEX {OK_STATE_INDEX}"
        ));
        return false;
    };
    if !(unsafe { is_plain_true_state(ok_target) }) {
        log(format!(
            "skip_flask_confirm: state {OK_STATE_INDEX} is not a plain if-1 state; aborting"
        ));
        return false;
    }

    let Some((confirm_state, _ok_branch)) =
        (unsafe { find_dialog_state(state_group, ok_target) })
    else {
        log("skip_flask_confirm: confirm dialog state not found");
        return false;
    };
    let Some(edge) = (unsafe { incoming_edge(state_group, confirm_state) }) else {
        log("skip_flask_confirm: no incoming edge to the confirm dialog state");
        return false;
    };

    if unsafe { redirect_transition(edge, ok_target) } {
        log(format!(
            "skip_flask_confirm: skipped confirmation in flask group {id}"
        ));
        true
    } else {
        false
    }
}
