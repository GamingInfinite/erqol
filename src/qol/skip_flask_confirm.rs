use crate::config;
use crate::ezstate_menu::{
    dump_state_group, find_dialog_state, incoming_edge, is_plain_true_state,
    redirect_transition, register_group_patcher, state_at_index, state_group_id, StateGroup,
};
use crate::log::log;
use std::sync::Mutex;

/// A state group whose upgrade-confirmation dialog should be skipped.
struct ConfirmSpec {
    /// Human-readable label used in log messages.
    name: &'static str,
    /// The ESD state-group id to patch.
    group_id: i32,
    /// Array index of the group's OK pass-through state: the state the confirm
    /// dialog's OK branch (`#B9 == 0`) leads to.
    ok_index: usize,
}

/// Confirmation-skip specs. The base-game flask groups (golden seed, sacred
/// tear) flow from their confirm dialog to the index-6 pass-through; the DLC
/// upgrade groups (scadutree blessing, revered spirit ash) flow to index 9.
const CONFIRM_SPECS: [ConfirmSpec; 4] = [
    ConfirmSpec {
        name: "golden seed",
        group_id: 2147483640,
        ok_index: 6,
    },
    ConfirmSpec {
        name: "sacred tear",
        group_id: 2147483641,
        ok_index: 6,
    },
    ConfirmSpec {
        name: "scadutree blessing",
        group_id: 2147483572,
        ok_index: 9,
    },
    ConfirmSpec {
        name: "revered spirit ash",
        group_id: 2147483571,
        ok_index: 9,
    },
];

/// One-shot diagnostics: log the runtime layout of each group the first time
/// its initial state is entered.
static DIAGNOSED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Registers this feature's group patcher. Called once, before
/// `ezstate_menu::install()`.
pub(crate) fn init() {
    register_group_patcher(patch);
}

/// Skips the confirmation dialog in a flask/upgrade flow by redirecting the
/// transition that enters the dialog state straight to the state the dialog's
/// OK branch (`#B9 == 0`) leads to.
///
/// The dialog state is identified without relying on the `6:2147483647` entry
/// command (which the runtime does not expose in `entry_events`): it is the
/// unique state whose highest-priority branch resolves to the group's OK state
/// at array index `ok_index` for the group's spec.
unsafe fn patch(state_group: *mut StateGroup) -> bool {
    if !config::with_feature(|cfg| cfg.skip_flask_confirm) {
        return false;
    }

    let Some(id) = (unsafe { state_group_id(state_group) }) else {
        return false;
    };
    let Some(spec) = CONFIRM_SPECS.iter().find(|s| s.group_id == id) else {
        return false;
    };

    let mut diagnosed = DIAGNOSED.lock().unwrap_or_else(|e| e.into_inner());
    if !diagnosed.contains(&id) {
        diagnosed.push(id);
        let dump = unsafe { dump_state_group(state_group) };
        log(format!("skip_flask_confirm: runtime layout {dump}"));
    }
    drop(diagnosed);

    let Some(ok_target) = (unsafe { state_at_index(state_group, spec.ok_index) }) else {
        log(format!(
            "skip_flask_confirm: {}: no state at OK index {}",
            spec.name, spec.ok_index
        ));
        return false;
    };
    if !(unsafe { is_plain_true_state(ok_target) }) {
        log(format!(
            "skip_flask_confirm: {}: state {} is not a plain if-1 state; aborting",
            spec.name, spec.ok_index
        ));
        return false;
    }

    let Some((confirm_state, _ok_branch)) =
        (unsafe { find_dialog_state(state_group, ok_target) })
    else {
        log(format!(
            "skip_flask_confirm: {}: confirm dialog state not found",
            spec.name
        ));
        return false;
    };
    let Some(edge) = (unsafe { incoming_edge(state_group, confirm_state) }) else {
        log(format!(
            "skip_flask_confirm: {}: no incoming edge to the confirm dialog state",
            spec.name
        ));
        return false;
    };

    if unsafe { redirect_transition(edge, ok_target) } {
        log(format!(
            "skip_flask_confirm: skipped confirmation in group {} ({})",
            id, spec.name
        ));
        true
    } else {
        false
    }
}