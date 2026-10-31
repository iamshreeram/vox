//! T1a (DESIGN Rev 4, B1/B2b/B8): live-gate matrix for the chat Stop and the chat snapshot /
//! state-event identity fields. The live gate is `window_commands::window_command_allowed`
//! (lib.rs invoke handler); `chat_window::chat_command_allowed` is test-only.

use crate::commands::chat::{displayed_session, StateWire};
use crate::managers::chat_log::{ChatEvent, ChatEventKind, ChatSnapshot};
use crate::window_commands::window_command_allowed;
use std::collections::BTreeSet;

const LABELS: [&str; 6] = [
    "main",
    "chat",
    "orb",
    "orb-bubbles",
    "recording_overlay",
    "no_such_window",
];

/// Expected live-gate result for `label` x `command`, from the DESIGN Rev 4 rules.
fn expected_allowed(label: &str, command: &str) -> bool {
    match command {
        "chat_stop_conversation" | "get_chat_snapshot" | "clear_chat_log" | "chat_hide_window" => {
            label == "chat"
        }
        // Unchanged legacy stop: every label except the chat, orb and bubble windows.
        "converse_stop" => !matches!(label, "chat" | "orb" | "orb-bubbles"),
        other => panic!("no expectation written for {other}"),
    }
}

#[test]
fn live_gate_matrix_labels_by_stop_and_chat_commands() {
    let commands = [
        "chat_stop_conversation",
        "converse_stop",
        "get_chat_snapshot",
        "clear_chat_log",
        "chat_hide_window",
    ];
    let mut failures = Vec::new();
    for label in LABELS {
        for command in commands {
            let got = window_command_allowed(label, command);
            let want = expected_allowed(label, command);
            if got != want {
                failures.push(format!(
                    "label={label} command={command} got={got} want={want}"
                ));
            }
        }
    }
    assert!(failures.is_empty(), "gate matrix:\n{}", failures.join("\n"));
}

#[test]
fn chat_stop_is_allowed_only_for_the_chat_label() {
    assert!(window_command_allowed("chat", "chat_stop_conversation"));
    for label in [
        "main",
        "orb",
        "orb-bubbles",
        "recording_overlay",
        "no_such_window",
    ] {
        assert!(
            !window_command_allowed(label, "chat_stop_conversation"),
            "{label} must not stop a conversation by id"
        );
    }
}

#[test]
fn converse_stop_gate_is_unchanged_main_allowed_chat_denied() {
    assert!(window_command_allowed("main", "converse_stop"));
    assert!(!window_command_allowed("chat", "converse_stop"));
}

#[test]
fn displayed_session_is_none_when_idle_and_the_active_id_otherwise() {
    assert_eq!(displayed_session("idle", Some(7)), None);
    assert_eq!(displayed_session("listening", Some(7)), Some(7));
    assert_eq!(displayed_session("speaking", None), None);
}

fn keys_of(value: &serde_json::Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("object payload")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn snapshot_keys_are_pinned_and_carry_no_transcript_text() {
    let snapshot = ChatSnapshot {
        seq: 3,
        entries: vec![],
        state: "idle".into(),
        end: None,
        recording: false,
        active_session: Some(9),
    };
    let value = serde_json::to_value(&snapshot).expect("serializes");
    let expected: BTreeSet<String> = [
        "seq",
        "entries",
        "state",
        "end",
        "recording",
        "activeSession",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(keys_of(&value), expected, "ChatSnapshot exact keys");
    assert_eq!(value["activeSession"], serde_json::json!(9));
}

#[test]
fn state_event_wire_carries_only_the_session_identity_beside_the_event() {
    let event = ChatEvent {
        seq: 4,
        kind: ChatEventKind::State {
            state: "thinking".into(),
            end: None,
        },
        stamp: None,
    };
    let wire = StateWire {
        event: &event,
        active_session: Some(5),
    };
    let value = serde_json::to_value(&wire).expect("serializes");
    let expected: BTreeSet<String> = ["seq", "kind", "activeSession"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        keys_of(&value),
        expected,
        "state event exact top-level keys"
    );
    assert_eq!(value["activeSession"], serde_json::json!(5));
    assert_eq!(
        keys_of(&value["kind"]).len(),
        3,
        "kind keeps type, state, end"
    );
}
