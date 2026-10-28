//! G4a spec: command matrix for the bubble window (DESIGN.md A7-2 and A7-7 A27; R6-4 capability).
//! The `orb-bubbles` window may call exactly one command, `bubbles_ready`, and no other label may
//! call it. Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::window_commands`, additions):
//!   pub const BUBBLES_WINDOW_LABEL: &str = "orb-bubbles";
//!   window_command_allowed("orb-bubbles", c) is true ONLY for c == "bubbles_ready";
//!   no other label (main, chat, orb, recording_overlay, any other) may call "bubbles_ready".
//!   The orb and chat rules stay unchanged.

use crate::window_commands::{
    window_command_allowed, BUBBLES_WINDOW_LABEL, CHAT_COMMANDS, CHAT_LABEL, ORB_COMMANDS,
    ORB_LABEL,
};

const BUBBLES_READY: &str = "bubbles_ready";

/// Commands that exist for other windows and must all be denied to the bubble window.
const OTHER_COMMANDS: [&str; 8] = [
    "get_app_settings",
    "change_chat_presentation_setting",
    "change_converse_chat_log_setting",
    "set_orb_size",
    "send_chat_message",
    "clear_chat_log",
    "get_chat_snapshot",
    "chat_draft_set",
];

#[test]
fn bubble_window_label_is_orb_bubbles() {
    assert_eq!(BUBBLES_WINDOW_LABEL, "orb-bubbles");
}

#[test]
fn orb_bubbles_may_call_bubbles_ready() {
    assert!(window_command_allowed(BUBBLES_WINDOW_LABEL, BUBBLES_READY));
}

#[test]
fn orb_bubbles_may_call_no_other_command() {
    // A27: the bubble window has no other command, including every orb and chat command.
    for command in ORB_COMMANDS
        .iter()
        .chain(CHAT_COMMANDS.iter())
        .chain(OTHER_COMMANDS.iter())
        .copied()
    {
        assert!(
            !window_command_allowed(BUBBLES_WINDOW_LABEL, command),
            "orb-bubbles must not call {command}"
        );
    }
}

#[test]
fn no_other_label_may_call_bubbles_ready() {
    // A7-2: only the bubble window may call its readiness command.
    let labels = [
        "main",
        CHAT_LABEL,
        ORB_LABEL,
        "recording_overlay",
        "settings",
        "",
        "orb-bubbles-2",
        "Orb-Bubbles",
        "orb_bubbles",
    ];
    for label in labels {
        assert!(
            !window_command_allowed(label, BUBBLES_READY),
            "{label:?} must not call bubbles_ready"
        );
    }
}

#[test]
fn orb_and_chat_rules_are_unchanged() {
    // Re-assert the existing facts the matrix depends on.
    assert!(window_command_allowed(ORB_LABEL, "orb_activate"));
    assert!(window_command_allowed(CHAT_LABEL, "send_chat_message"));
    assert!(!window_command_allowed(ORB_LABEL, "send_chat_message"));
    assert!(!window_command_allowed(CHAT_LABEL, "orb_activate"));
    assert!(window_command_allowed("main", "get_app_settings"));
}
