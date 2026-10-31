//! G1 spec: activation-policy owner and the window command matrix (DESIGN.md Rev 4:
//! 'Activation policy owner', 'Rust modules and commands'). Authoritative: do not weaken an
//! assertion; if a test is wrong, report it.
//!
//! Contract:
//!   crate::activation_policy::{Policy {Regular, Accessory}, Inputs {main_visible, classic_chat_visible,
//!     panel_needs_focus, tray_present: bool}, OpenKind {User, Auto, Peek}, desired(&Inputs) -> Policy,
//!     open_may_focus(OpenKind) -> bool, open_may_change_policy(OpenKind) -> bool}
//!   crate::window_commands::{ORB_LABEL: &str = "orb", ORB_COMMANDS: [&str; 5], CHAT_COMMANDS: [&str; 11],
//!     window_command_allowed(window_label: &str, command: &str) -> bool}
//!   The existing crate::chat_window::chat_command_allowed stays UNCHANGED in G1 and is still the gate in
//!   lib.rs (the new function replaces it in G3 when the commands exist).
use crate::activation_policy::*;
use crate::window_commands::*;

fn inputs(main: bool, classic: bool, panel: bool, tray: bool) -> Inputs {
    Inputs {
        main_visible: main,
        classic_chat_visible: classic,
        panel_needs_focus: panel,
        tray_present: tray,
    }
}

#[test]
fn accessory_only_when_nothing_needs_the_dock_and_a_tray_exists() {
    for bits in 0u8..16 {
        let main = bits & 1 != 0;
        let classic = bits & 2 != 0;
        let panel = bits & 4 != 0;
        let tray = bits & 8 != 0;
        let expected = if !main && !classic && !panel && tray {
            Policy::Accessory
        } else {
            Policy::Regular
        };
        assert_eq!(
            desired(&inputs(main, classic, panel, tray)),
            expected,
            "main={main} classic={classic} panel={panel} tray={tray}"
        );
    }
}

#[test]
fn only_a_user_open_may_focus_or_change_the_policy() {
    assert!(open_may_focus(OpenKind::User));
    assert!(!open_may_focus(OpenKind::Auto));
    assert!(!open_may_focus(OpenKind::Peek));
    assert!(open_may_change_policy(OpenKind::User));
    assert!(!open_may_change_policy(OpenKind::Auto));
    assert!(!open_may_change_policy(OpenKind::Peek));
}

const OLD_CHAT: [&str; 3] = ["get_chat_snapshot", "clear_chat_log", "send_chat_message"];
const NEW_CHAT: [&str; 7] = [
    "chat_panel_ready",
    "chat_panel_resize",
    "chat_panel_close",
    "chat_draft_set",
    "chat_draft_get",
    "chat_draft_clear",
    "chat_draft_flush_ack",
];
/// Added by the orb-click chrome step (D1b): the chat window hides itself on Escape.
const HIDE_CHAT: [&str; 1] = ["chat_hide_window"];
/// T1a (DESIGN Rev 4, B1/B2b): the chat window's session-id Stop, chat-only by the live gate.
const STOP_CHAT: [&str; 1] = ["chat_stop_conversation"];
const ORB: [&str; 5] = [
    "orb_pointer_down",
    "orb_activate",
    "orb_menu",
    "orb_get_state",
    "orb_set_reduced_motion",
];

#[test]
fn the_constants_list_exactly_the_documented_commands() {
    assert_eq!(ORB_LABEL, "orb");
    let mut orb: Vec<&str> = ORB_COMMANDS.to_vec();
    orb.sort_unstable();
    let mut want: Vec<&str> = ORB.to_vec();
    want.sort_unstable();
    assert_eq!(orb, want);

    let mut chat: Vec<&str> = CHAT_COMMANDS.to_vec();
    chat.sort_unstable();
    let mut want: Vec<&str> = OLD_CHAT
        .iter()
        .chain(NEW_CHAT.iter())
        .chain(HIDE_CHAT.iter())
        .chain(STOP_CHAT.iter())
        .copied()
        .collect();
    want.sort_unstable();
    assert_eq!(chat, want);
}

#[test]
fn the_orb_window_may_call_only_the_orb_commands() {
    for c in ORB {
        assert!(window_command_allowed("orb", c), "{c}");
    }
    for c in OLD_CHAT.iter().chain(NEW_CHAT.iter()) {
        assert!(!window_command_allowed("orb", c), "{c}");
    }
    for c in [
        "get_app_settings",
        "change_ptt_setting",
        "toggle_chat_window_command",
        "brain_test_connection",
        "",
    ] {
        assert!(!window_command_allowed("orb", c), "{c}");
    }
}

#[test]
fn the_chat_window_may_call_only_its_eleven_commands() {
    for c in OLD_CHAT
        .iter()
        .chain(NEW_CHAT.iter())
        .chain(HIDE_CHAT.iter())
    {
        assert!(window_command_allowed("chat", c), "{c}");
    }
    for c in ORB {
        assert!(!window_command_allowed("chat", c), "{c}");
    }
    for c in [
        "get_app_settings",
        "toggle_chat_window_command",
        "first_run_finish_step",
        "",
    ] {
        assert!(!window_command_allowed("chat", c), "{c}");
    }
}

#[test]
fn every_other_window_is_refused_the_orb_and_chat_commands() {
    for label in ["main", "recording_overlay", "settings", "", "ORB", "Chat"] {
        for c in ORB
            .iter()
            .chain(OLD_CHAT.iter())
            .chain(NEW_CHAT.iter())
            .chain(HIDE_CHAT.iter())
        {
            assert!(!window_command_allowed(label, c), "{label}: {c}");
        }
        assert!(
            window_command_allowed(label, "get_app_settings"),
            "{label} keeps normal commands"
        );
    }
}

#[test]
fn the_new_gate_is_a_superset_of_the_old_chat_gate() {
    for c in OLD_CHAT {
        assert_eq!(
            crate::chat_window::chat_command_allowed("chat", c),
            window_command_allowed("chat", c)
        );
    }
    // The old gate is untouched in G1: it does not know the new commands yet.
    assert!(!crate::chat_window::chat_command_allowed(
        "chat",
        "chat_panel_ready"
    ));
}

#[test]
fn the_orb_capability_file_grants_events_only_once_it_exists() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/orb.json");
    if !path.exists() {
        return; // added in G3 together with the orb window
    }
    let text = std::fs::read_to_string(path).expect("readable");
    let json: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(json["windows"], serde_json::json!(["orb"]));
    let perms: Vec<String> = json["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .map(|p| p.as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        perms,
        ["core:event:allow-listen", "core:event:allow-unlisten"]
    );
}
