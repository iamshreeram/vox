//! Stage 4 settings for specialist agents and the planner (DESIGN-PR4-PLANNER.md Revision 4,
//! sections 2.2, A6 and A8): defaults and serde of `acp_step_agents` and `conversation_planner`,
//! existing-store salvage, `apply_step_agent_setting` validation, `preserve_launch_fields`
//! (the launch-field guard of the central writer) and the A8 interleaving model.
//!
//! This file is a child module of `settings` (a `#[path]` line at the end of `settings.rs`),
//! so the private `salvage_settings` is reachable. The new fields are read and written through
//! serde JSON, so this file does not name them as Rust fields. Workers must not edit this file.

use super::*;
use serde_json::{json, Value};

/// Every field that `preserve_launch_fields` must take from the committed settings.
const LAUNCH_KEYS: [&str; 8] = [
    "acp_agent_enabled",
    "acp_agent_command",
    "acp_agent_args",
    "acp_agent_turn_timeout_secs",
    "acp_permission_mode",
    "acp_step_agents",
    "conversation_agent_handoff",
    "conversation_planner",
];

fn snapshot(s: &AppSettings) -> Value {
    serde_json::to_value(s).expect("settings serialize")
}

/// Defaults with the given top-level keys replaced.
fn settings(patch: Value) -> AppSettings {
    let mut value = snapshot(&get_default_settings());
    for (key, val) in patch.as_object().expect("patch object") {
        value[key.as_str()] = val.clone();
    }
    serde_json::from_value(value).expect("settings deserialize")
}

// ---------------- defaults and serde ----------------

#[test]
fn defaults_turn_the_planner_on_and_have_no_specialists() {
    let value = snapshot(&get_default_settings());
    assert_eq!(value["conversation_planner"], json!(true));
    for kind in ["research", "browser", "code", "files"] {
        assert!(
            value["acp_step_agents"][kind].is_null(),
            "{kind} has no profile by default"
        );
    }
}

#[test]
fn missing_keys_in_a_stored_object_default_to_planner_on_and_no_specialists() {
    let mut stored = snapshot(&get_default_settings());
    let map = stored.as_object_mut().expect("object");
    map.remove("conversation_planner");
    map.remove("acp_step_agents");
    let parsed: AppSettings =
        serde_json::from_value(stored).expect("a store without the keys parses");
    let value = snapshot(&parsed);
    assert_eq!(value["conversation_planner"], json!(true));
    assert!(value["acp_step_agents"]["research"].is_null());
    assert!(value["acp_step_agents"]["files"].is_null());
}

#[test]
fn specialist_profiles_and_planner_flag_round_trip() {
    let original = settings(json!({
        "conversation_planner": false,
        "acp_step_agents": {"research": {"command": "/opt/r", "args": "--x"}},
    }));
    let value = snapshot(&original);
    let again: AppSettings = serde_json::from_value(value.clone()).expect("round trip parses");
    let value_again = snapshot(&again);
    assert_eq!(value_again, value);
    assert_eq!(value_again["conversation_planner"], json!(false));
    assert_eq!(
        value_again["acp_step_agents"]["research"],
        json!({"command": "/opt/r", "args": "--x"})
    );
}

// ---------------- existing-store salvage ----------------

#[test]
fn salvage_keeps_saved_profiles_and_defaults_the_missing_keys() {
    let mut stored = snapshot(&get_default_settings());
    let map = stored.as_object_mut().expect("object");
    map.remove("conversation_planner");
    map.insert(
        "acp_step_agents".into(),
        json!({"research": {"command": "/opt/r", "args": "--x"}}),
    );
    map.insert("selected_model".into(), json!("parakeet-tdt-0.6b-v3"));
    // An enum value this build does not know: only that field falls back.
    map.insert("sound_theme".into(), json!("theremin"));

    let salvaged = snapshot(&salvage_settings(&stored));
    assert_eq!(salvaged["acp_step_agents"]["research"]["command"], "/opt/r");
    assert_eq!(salvaged["acp_step_agents"]["research"]["args"], "--x");
    assert!(salvaged["acp_step_agents"]["browser"].is_null());
    assert!(salvaged["acp_step_agents"]["code"].is_null());
    assert_eq!(salvaged["conversation_planner"], json!(true));
    assert_eq!(salvaged["selected_model"], "parakeet-tdt-0.6b-v3");
}

#[test]
fn salvage_of_a_wrong_typed_profile_drops_only_that_field() {
    let mut stored = snapshot(&get_default_settings());
    let map = stored.as_object_mut().expect("object");
    map.insert(
        "acp_step_agents".into(),
        json!({"research": {"command": 5, "args": ""}}),
    );
    map.insert("selected_model".into(), json!("parakeet-tdt-0.6b-v3"));

    let salvaged = snapshot(&salvage_settings(&stored));
    assert!(salvaged["acp_step_agents"]["research"].is_null());
    assert_eq!(salvaged["selected_model"], "parakeet-tdt-0.6b-v3");
}

// ---------------- apply_step_agent_setting (A6) ----------------

#[test]
fn a_valid_profile_is_stored_trimmed() {
    let mut s = get_default_settings();
    let result = apply_step_agent_setting(
        &mut s,
        "research",
        Some("  /opt/research-agent  "),
        "  --fast --x  ",
    );
    assert!(result.is_ok(), "{result:?}");
    let value = snapshot(&s);
    assert_eq!(
        value["acp_step_agents"]["research"]["command"],
        "/opt/research-agent"
    );
    assert_eq!(value["acp_step_agents"]["research"]["args"], "--fast --x");
}

#[test]
fn every_step_type_accepts_a_profile() {
    for kind in ["research", "browser", "code", "files"] {
        let mut s = get_default_settings();
        assert!(
            apply_step_agent_setting(&mut s, kind, Some("/opt/x"), "").is_ok(),
            "{kind}"
        );
        assert_eq!(
            snapshot(&s)["acp_step_agents"][kind]["command"],
            "/opt/x",
            "{kind}"
        );
    }
}

#[test]
fn a_blank_or_missing_command_clears_only_that_profile() {
    let mut s = get_default_settings();
    apply_step_agent_setting(&mut s, "research", Some("/opt/r"), "").expect("set");
    apply_step_agent_setting(&mut s, "code", Some("/opt/c"), "").expect("set");

    apply_step_agent_setting(&mut s, "research", None, "").expect("clear with none");
    assert!(snapshot(&s)["acp_step_agents"]["research"].is_null());
    assert_eq!(snapshot(&s)["acp_step_agents"]["code"]["command"], "/opt/c");

    apply_step_agent_setting(&mut s, "code", Some(""), "").expect("clear with empty");
    assert!(snapshot(&s)["acp_step_agents"]["code"].is_null());
    apply_step_agent_setting(&mut s, "files", Some("/opt/f"), "").expect("set");
    apply_step_agent_setting(&mut s, "files", Some("   "), "").expect("clear with blank");
    assert!(snapshot(&s)["acp_step_agents"]["files"].is_null());
}

#[test]
fn rejected_values_leave_the_settings_untouched() {
    let mut s = get_default_settings();
    apply_step_agent_setting(&mut s, "research", Some("/opt/old"), "").expect("set");
    let before = snapshot(&s);

    let long_arg = "z".repeat(257);
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("shell", Some("/opt/x"), ""),
        ("Research", Some("/opt/x"), ""),
        ("", Some("/opt/x"), ""),
        ("general", Some("/opt/x"), ""),
        ("research", Some("bin/agent"), ""),
        ("research", Some("./agent"), ""),
        ("research", Some("/opt/x\n--evil"), ""),
        ("research", Some("/opt/\u{7}x"), ""),
        ("research", Some("/opt/x"), "--a\u{7}"),
        ("research", Some("/opt/x"), "a a a a a a a a a"),
        ("research", Some("/opt/x"), long_arg.as_str()),
    ];
    for (kind, command, args) in cases {
        let result = apply_step_agent_setting(&mut s, kind, command, args);
        assert!(
            result.is_err(),
            "{kind} {command:?} {args:?} must be rejected"
        );
        assert_eq!(snapshot(&s), before, "{kind} {command:?} must not write");
    }
}

// ---------------- preserve_launch_fields (A8) ----------------

#[test]
fn committed_launch_fields_win_over_the_incoming_ones() {
    let committed = settings(json!({
        "acp_agent_enabled": true,
        "acp_agent_command": "/usr/bin/new-agent",
        "acp_agent_args": "--new",
        "acp_agent_turn_timeout_secs": 200,
        "acp_permission_mode": "ask_all",
        "acp_step_agents": {"research": {"command": "/opt/new-r", "args": ""}},
        "conversation_agent_handoff": true,
        "conversation_planner": true,
    }));
    let incoming = settings(json!({
        "acp_agent_enabled": false,
        "acp_agent_command": "/usr/bin/old-agent",
        "acp_agent_args": "--old",
        "acp_agent_turn_timeout_secs": 60,
        "acp_permission_mode": "deny",
        "acp_step_agents": {"research": {"command": "/opt/old-r", "args": ""}},
        "conversation_agent_handoff": false,
        "conversation_planner": false,
    }));
    let committed_value = snapshot(&committed);
    let out = snapshot(&preserve_launch_fields(incoming, &committed));
    for key in LAUNCH_KEYS {
        assert_eq!(out[key], committed_value[key], "launch field {key}");
    }
}

#[test]
fn incoming_unrelated_fields_win_over_the_committed_ones() {
    let committed = settings(json!({"selected_model": "committed-model"}));
    let incoming = settings(json!({"selected_model": "incoming-model"}));
    let out = snapshot(&preserve_launch_fields(incoming, &committed));
    assert_eq!(out["selected_model"], "incoming-model");
}

#[test]
fn a_stale_whole_object_write_keeps_a_gated_launch_change() {
    // The unrelated setter reads A, a gated change commits B, then the unrelated setter writes
    // A with its own unrelated change through preserve_launch_fields.
    let before = settings(json!({
        "acp_agent_command": "/usr/bin/old-agent",
        "selected_model": "model-a",
    }));
    let read_a = before.clone();
    let committed_b = settings(json!({
        "acp_agent_command": "/usr/bin/new-agent",
        "acp_agent_enabled": true,
    }));
    let mut incoming = read_a;
    incoming.selected_model = "model-b".to_string();

    let out = snapshot(&preserve_launch_fields(incoming, &committed_b));
    assert_eq!(
        out["acp_agent_command"], "/usr/bin/new-agent",
        "the gated change is kept"
    );
    assert_eq!(out["acp_agent_enabled"], json!(true));
    assert_eq!(
        out["selected_model"], "model-b",
        "the unrelated change is kept"
    );
}
