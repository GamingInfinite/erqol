//! Roundtable Hold mirror "Alter Posture" menu.
//!
//! The vanilla mirror (Clouded Mirror Stand) opens a talk list with two rows:
//! `Apply Cosmetics` (slot 1) and `Leave` (slot 99), built by talk state group
//! `2147483613` (state 5 holds the two `AddTalkListData` events; the initial
//! state 0 is an event-less pass-through hub that flows 2 -> 5 -> 4, so
//! transitioning back into it re-opens the full mirror list).
//!
//! This feature splices a third row, `Alter Posture`, which opens a nested
//! menu tree mirroring AuraFarmingPostures' own talk ESD: category rows
//! (body, right arm, left arm, movement style) open checkbox-style picker
//! lists with one row per posture (`[X]` marks the current selection), plus
//! direct toggles for alternative landing and a clear-all action.

use std::sync::LazyLock;

use crate::config;
use crate::ezstate_menu::{
    alloc_message_block, alloc_message_id, event_arg_int, register_group_patcher, register_message,
    slice_of, splice_talk_list_option, update_message, ADD_TALK_LIST_DATA, StateGroup, SubMenu,
    SubMenuAction,
};
use crate::log::log;

/// The mirror's talk state group id (`m11_10_00_00.talkesdbnd.dcx`).
const MIRROR_GROUP: i32 = 2147483613;

// ---- Message IDs ----
//
// Allocated from the shared allocator (single ids + contiguous picker-row
// blocks) so they never collide with another module.

static MSG_ALTER_POSTURE: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_BODY: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_RIGHT_ARM: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_LEFT_ARM: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_MOVEMENT: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_LANDING: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_CLEAR_ALL: LazyLock<i32> = LazyLock::new(alloc_message_id);
const MSG_CANCEL: i32 = 69_990_003;

/// Picker-row message ranges; `[X]` markers are rewritten on every change.
static MSG_BODY_BASE: LazyLock<i32> = LazyLock::new(|| alloc_message_block(BODY_LABELS.len()));
static MSG_RIGHT_ARM_BASE: LazyLock<i32> = LazyLock::new(|| alloc_message_block(ARM_LABELS.len()));
static MSG_LEFT_ARM_BASE: LazyLock<i32> = LazyLock::new(|| alloc_message_block(ARM_LABELS.len()));
static MSG_MOVE_DEFAULT: LazyLock<i32> = LazyLock::new(alloc_message_id);
static MSG_MOVE_HEAVY: LazyLock<i32> = LazyLock::new(alloc_message_id);

const OPTION_INDEX: i32 = 2;

// Real option lists extracted from AuraFarmingPostures' EventTextForTalk.fmg
// and talk ESD (general postures = msg rows 26080110-26080221, arm/grip
// choices = rows 26080420+). Config values index into these tables.
const BODY_STYLES: i32 = BODY_LABELS.len() as i32;

/// General/body postures in menu order; index 0 ("Normal") disables.
const BODY_LABELS: [&str; 13] = [
    "Normal",
    "Type A",
    "Alternative",
    "Chivalric",
    "Regal",
    "Grave",
    "Courtly",
    "Formidable",
    "Severe",
    "Gentle",
    "Steadfast",
    "Scholarly",
    "Lithe",
];

/// Arm posture choices shared by both arms; index 0 inherits the body
/// posture, index 1 forces vanilla arms.
const ARM_LABELS: [&str; 12] = [
    "Inherit General",
    "Force Normal",
    "Chivalric",
    "Regal",
    "Grave",
    "Courtly",
    "Formidable",
    "Severe",
    "Gentle",
    "Steadfast",
    "Scholarly",
    "Lithe",
];

const MOVEMENT_LABELS: [&str; 2] = ["Default", "Heavy"];

// ---- Label helpers ----

fn named_label(name: &str, labels: &[&str], value: i32) -> String {
    let idx = value.rem_euclid(labels.len() as i32) as usize;
    format!("{name}: {}", labels[idx])
}

fn landing_label(enabled: bool) -> String {
    let state = if enabled { "[ON]" } else { "[OFF]" };
    format!("{state} Alternative Landing")
}

fn marker_label(enabled: bool, label: &str) -> String {
    format!("[{}] {label}", if enabled { "X" } else { " " })
}

fn with_config<T>(f: impl FnOnce(&config::Config) -> T) -> T {
    f(&config::config().lock().unwrap_or_else(|e| e.into_inner()))
}

// ---- Message refreshers ----

fn refresh_main_messages() {
    with_config(|cfg| {
        update_message(
            *MSG_BODY,
            &named_label("Body Posture", &BODY_LABELS, cfg.posture_body),
        );
        update_message(
            *MSG_RIGHT_ARM,
            &named_label("Right Arm Posture", &ARM_LABELS, cfg.posture_right_arm),
        );
        update_message(
            *MSG_LEFT_ARM,
            &named_label("Left Arm Posture", &ARM_LABELS, cfg.posture_left_arm),
        );
        update_message(*MSG_MOVEMENT, &movement_label(cfg.posture_movement));
        update_message(*MSG_LANDING, &landing_label(cfg.posture_alternative_landing));
    });
}

fn movement_label(movement: i32) -> String {
    named_label("Movement Style", &MOVEMENT_LABELS, movement)
}

fn refresh_body_messages() {
    let body = with_config(|cfg| cfg.posture_body.rem_euclid(BODY_STYLES));
    for (i, label) in BODY_LABELS.iter().enumerate() {
        update_message(
            *MSG_BODY_BASE + i as i32,
            &marker_label(i as i32 == body, label),
        );
    }
}

fn refresh_arm_messages(base: i32, labels: &[&str], value: i32) {
    let value = value.rem_euclid(labels.len() as i32);
    for (i, label) in labels.iter().enumerate() {
        update_message(base + i as i32, &marker_label(i as i32 == value, label));
    }
}

fn refresh_right_arm_messages() {
    let v = with_config(|cfg| cfg.posture_right_arm);
    refresh_arm_messages(*MSG_RIGHT_ARM_BASE, &ARM_LABELS, v);
}

fn refresh_left_arm_messages() {
    let v = with_config(|cfg| cfg.posture_left_arm);
    refresh_arm_messages(*MSG_LEFT_ARM_BASE, &ARM_LABELS, v);
}

fn refresh_movement_messages() {
    let heavy = with_config(|cfg| cfg.posture_movement != 0);
    update_message(*MSG_MOVE_DEFAULT, &marker_label(!heavy, MOVEMENT_LABELS[0]));
    update_message(*MSG_MOVE_HEAVY, &marker_label(heavy, MOVEMENT_LABELS[1]));
}

fn refresh_all_messages() {
    refresh_main_messages();
    refresh_body_messages();
    refresh_right_arm_messages();
    refresh_left_arm_messages();
    refresh_movement_messages();
}

fn register_menu_messages() {
    register_message(*MSG_ALTER_POSTURE, "Alter Posture");
    register_message(*MSG_BODY, "Body Posture");
    register_message(*MSG_RIGHT_ARM, "Right Arm Posture");
    register_message(*MSG_LEFT_ARM, "Left Arm Posture");
    register_message(*MSG_MOVEMENT, "Movement Style");
    register_message(*MSG_LANDING, "[OFF] Alternative Landing");
    register_message(*MSG_CLEAR_ALL, "Clear All Postures");
    register_message(MSG_CANCEL, "Cancel");
    for (i, label) in BODY_LABELS.iter().enumerate() {
        register_message(*MSG_BODY_BASE + i as i32, &marker_label(false, label));
    }
    for (i, label) in ARM_LABELS.iter().enumerate() {
        register_message(*MSG_RIGHT_ARM_BASE + i as i32, &marker_label(false, label));
        register_message(*MSG_LEFT_ARM_BASE + i as i32, &marker_label(false, label));
    }
    register_message(*MSG_MOVE_DEFAULT, &marker_label(true, MOVEMENT_LABELS[0]));
    register_message(*MSG_MOVE_HEAVY, &marker_label(false, MOVEMENT_LABELS[1]));
    // Rewrite the registered defaults with the actual current selections.
    refresh_all_messages();
}

// ---- Selection actions ----

/// Persists the current config and refreshes every message that can show a
/// selection marker. Called after any menu mutation.
fn persist() {
    crate::postures::effects::request_sync();
    config::save();
    refresh_all_messages();
}

macro_rules! pick_action {
    ($fn_name:ident, $field:ident, $value:expr) => {
        unsafe extern "C" fn $fn_name() {
            {
                let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
                cfg.$field = $value;
            }
            persist();
            log(&format!(
                concat!("postures: ", stringify!($field), " = {}"),
                $value
            ));
        }
    };
}

pick_action!(set_body_0, posture_body, 0);
pick_action!(set_body_1, posture_body, 1);
pick_action!(set_body_2, posture_body, 2);
pick_action!(set_body_3, posture_body, 3);
pick_action!(set_body_4, posture_body, 4);
pick_action!(set_body_5, posture_body, 5);
pick_action!(set_body_6, posture_body, 6);
pick_action!(set_body_7, posture_body, 7);
pick_action!(set_body_8, posture_body, 8);
pick_action!(set_body_9, posture_body, 9);
pick_action!(set_body_10, posture_body, 10);
pick_action!(set_body_11, posture_body, 11);
pick_action!(set_body_12, posture_body, 12);

pick_action!(set_right_arm_0, posture_right_arm, 0);
pick_action!(set_right_arm_1, posture_right_arm, 1);
pick_action!(set_right_arm_2, posture_right_arm, 2);
pick_action!(set_right_arm_3, posture_right_arm, 3);
pick_action!(set_right_arm_4, posture_right_arm, 4);
pick_action!(set_right_arm_5, posture_right_arm, 5);
pick_action!(set_right_arm_6, posture_right_arm, 6);
pick_action!(set_right_arm_7, posture_right_arm, 7);
pick_action!(set_right_arm_8, posture_right_arm, 8);
pick_action!(set_right_arm_9, posture_right_arm, 9);
pick_action!(set_right_arm_10, posture_right_arm, 10);
pick_action!(set_right_arm_11, posture_right_arm, 11);

pick_action!(set_left_arm_0, posture_left_arm, 0);
pick_action!(set_left_arm_1, posture_left_arm, 1);
pick_action!(set_left_arm_2, posture_left_arm, 2);
pick_action!(set_left_arm_3, posture_left_arm, 3);
pick_action!(set_left_arm_4, posture_left_arm, 4);
pick_action!(set_left_arm_5, posture_left_arm, 5);
pick_action!(set_left_arm_6, posture_left_arm, 6);
pick_action!(set_left_arm_7, posture_left_arm, 7);
pick_action!(set_left_arm_8, posture_left_arm, 8);
pick_action!(set_left_arm_9, posture_left_arm, 9);
pick_action!(set_left_arm_10, posture_left_arm, 10);
pick_action!(set_left_arm_11, posture_left_arm, 11);

pick_action!(set_movement_default, posture_movement, 0);
pick_action!(set_movement_heavy, posture_movement, 1);

unsafe extern "C" fn toggle_landing() {
    {
        let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        cfg.posture_alternative_landing = !cfg.posture_alternative_landing;
    }
    persist();
    log("postures: alternative_landing toggled");
}

unsafe extern "C" fn clear_all() {
    {
        let mut cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
        cfg.posture_body = 0;
        cfg.posture_right_arm = 0;
        cfg.posture_left_arm = 0;
        cfg.posture_movement = 0;
        cfg.posture_alternative_landing = false;
    }
    persist();
    log("postures: cleared all posture settings");
}

// ---- Patcher ----

type Row = (i32, i32, bool, Option<SubMenuAction>);

/// Builds the checkbox list for one category: one row per option (self-return
/// so the player stays in the list and sees the refreshed `[X]`) plus a Back
/// row returning to the main Alter Posture page.
unsafe fn build_picker(rows: &[Row], main_state: *mut crate::ezstate_menu::State) -> *mut crate::ezstate_menu::State {
    unsafe {
        let mut all_rows = rows.to_vec();
        all_rows.push((99, MSG_CANCEL, true, None));
        let picker = Box::into_raw(Box::new(SubMenu::new(&all_rows)));
        let self_state = std::ptr::addr_of_mut!((*picker).state);
        SubMenu::link(picker, main_state, self_state)
    }
}

/// True once this group's talk list already contains our spliced row. The
/// patcher runs on every entry of the group's initial state and Cancel returns
/// to that same initial state, so without this guard each revisit would append
/// a duplicate row.
unsafe fn already_patched(state_group: *mut StateGroup) -> bool {
    unsafe {
        for state in slice_of((*state_group).states) {
            for event in slice_of(state.entry_events) {
                if event.command == ADD_TALK_LIST_DATA
                    && event_arg_int(event, 1) == *MSG_ALTER_POSTURE
                {
                    return true;
                }
            }
        }
    }
    false
}

unsafe fn patch_mirror(state_group: *mut StateGroup) -> bool {
    unsafe {
        if (*state_group).id != MIRROR_GROUP || already_patched(state_group) {
            return false;
        }
        {
            let cfg = config::config().lock().unwrap_or_else(|e| e.into_inner());
            if !cfg.postures_enabled {
                return false;
            }
        }

        let initial_state = (*state_group).initial_state;
        if initial_state.is_null() {
            return false;
        }

        // Main Alter Posture page. Category rows start without actions and are
        // re-pointed at their picker states below.
        let main_rows: &[Row] = &[
            (1, *MSG_BODY, false, None),
            (2, *MSG_RIGHT_ARM, false, None),
            (3, *MSG_LEFT_ARM, false, None),
            (4, *MSG_MOVEMENT, false, None),
            (5, *MSG_LANDING, false, Some(toggle_landing)),
            (6, *MSG_CLEAR_ALL, false, Some(clear_all)),
            (99, MSG_CANCEL, true, None),
        ];
        let main_menu = Box::into_raw(Box::new(SubMenu::new(main_rows)));
        let main_state = std::ptr::addr_of_mut!((*main_menu).state);

        let body_options: Vec<Row> = BODY_LABELS
            .iter()
            .enumerate()
            .map(|(i, _)| {
                (
                    // Slots start at 1: pressing B yields a talk-list result of
                    // 0, which must fall through to the default Back row
                    // instead of matching an action row.
                    i as i32 + 1,
                    *MSG_BODY_BASE + i as i32,
                    false,
                    Some(BODY_ACTIONS[i]),
                )
            })
            .collect();

        let right_options: Vec<Row> = ARM_LABELS
            .iter()
            .enumerate()
            .map(|(i, _)| {
                (
                    i as i32 + 1,
                    *MSG_RIGHT_ARM_BASE + i as i32,
                    false,
                    Some(RIGHT_ARM_ACTIONS[i]),
                )
            })
            .collect();

        let left_options: Vec<Row> = ARM_LABELS
            .iter()
            .enumerate()
            .map(|(i, _)| {
                (
                    i as i32 + 1,
                    *MSG_LEFT_ARM_BASE + i as i32,
                    false,
                    Some(LEFT_ARM_ACTIONS[i]),
                )
            })
            .collect();

        let move_options: Vec<Row> = vec![
            (
                1,
                *MSG_MOVE_DEFAULT,
                false,
                Some(set_movement_default),
            ),
            (2, *MSG_MOVE_HEAVY, false, Some(set_movement_heavy)),
        ];

        let body_state = build_picker(&body_options, main_state);
        let right_state = build_picker(&right_options, main_state);
        let left_state = build_picker(&left_options, main_state);
        let move_state = build_picker(&move_options, main_state);

        // Cancel falls through to the mirror's initial state, which replays its
        // pass-through flow and re-opens the vanilla list; toggle/action rows
        // return to the main page itself.
        let submenu_state = SubMenu::link(main_menu, initial_state, main_state);

        // Re-point the category rows at their pickers AFTER linking: `link`
        // rewrites every row's transition target.
        SubMenu::set_option_target(main_menu, 0, body_state);
        SubMenu::set_option_target(main_menu, 1, right_state);
        SubMenu::set_option_target(main_menu, 2, left_state);
        SubMenu::set_option_target(main_menu, 3, move_state);

        if splice_talk_list_option(state_group, OPTION_INDEX, *MSG_ALTER_POSTURE, submenu_state) {
            log("postures: spliced Alter Posture into mirror menu");
            true
        } else {
            log("postures: failed to find mirror talk-list states");
            false
        }
    }
}

// Action tables indexed like the label tables above.
static BODY_ACTIONS: [SubMenuAction; 13] = [
    set_body_0,
    set_body_1,
    set_body_2,
    set_body_3,
    set_body_4,
    set_body_5,
    set_body_6,
    set_body_7,
    set_body_8,
    set_body_9,
    set_body_10,
    set_body_11,
    set_body_12,
];
static RIGHT_ARM_ACTIONS: [SubMenuAction; 12] = [
    set_right_arm_0,
    set_right_arm_1,
    set_right_arm_2,
    set_right_arm_3,
    set_right_arm_4,
    set_right_arm_5,
    set_right_arm_6,
    set_right_arm_7,
    set_right_arm_8,
    set_right_arm_9,
    set_right_arm_10,
    set_right_arm_11,
];
static LEFT_ARM_ACTIONS: [SubMenuAction; 12] = [
    set_left_arm_0,
    set_left_arm_1,
    set_left_arm_2,
    set_left_arm_3,
    set_left_arm_4,
    set_left_arm_5,
    set_left_arm_6,
    set_left_arm_7,
    set_left_arm_8,
    set_left_arm_9,
    set_left_arm_10,
    set_left_arm_11,
];

// ---- Installer ----

pub(crate) fn init() {
    register_menu_messages();
    register_group_patcher(patch_mirror);
}
