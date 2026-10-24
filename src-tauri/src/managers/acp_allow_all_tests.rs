//! Contract tests for `PermissionMode::AllowAll` ("Allow everything", DESIGN-PR2-STAGE2
//! Revision 3, section 2.7, C11). Red until the production step adds the variant.
//!
//! Pure tests: no IO, no Tauri. Requests are built through
//! `parse_permission_request`, so they go through the real parse path.
//! Workers must not edit this file.

use crate::managers::acp_permission::{
    decide_auto, outcome_for, parse_permission_request, AutoDecision, Outcome, PermissionMode,
    PermissionRequest, UserChoice,
};
use serde_json::{json, Value};

fn request(kind: &str, options: Value, raw_input: Option<Value>) -> PermissionRequest {
    let mut tool_call = json!({"toolCallId": "t1", "title": "Do a thing", "kind": kind});
    if let Some(raw) = raw_input {
        tool_call["rawInput"] = raw;
    }
    let params = json!({"sessionId": "s1", "toolCall": tool_call, "options": options});
    parse_permission_request(&params).expect("well-formed request must parse")
}

fn allow_once() -> Value {
    json!({"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"})
}

fn allow_always() -> Value {
    json!({"optionId": "allow-always", "name": "Always", "kind": "allow_always"})
}

fn reject_once() -> Value {
    json!({"optionId": "reject-once", "name": "Reject", "kind": "reject_once"})
}

fn reject_always() -> Value {
    json!({"optionId": "reject-always", "name": "Never", "kind": "reject_always"})
}

// ---- wire names (serde) ----

#[test]
fn allow_all_serializes_as_allow_all() {
    assert_eq!(
        serde_json::to_value(PermissionMode::AllowAll).unwrap(),
        json!("allow_all")
    );
}

#[test]
fn allow_all_deserializes_from_allow_all() {
    let mode: PermissionMode = serde_json::from_value(json!("allow_all")).unwrap();
    assert_eq!(mode, PermissionMode::AllowAll);
}

#[test]
fn existing_modes_keep_their_wire_names_and_round_trip() {
    let cases = [
        (PermissionMode::AskWrites, "ask_writes"),
        (PermissionMode::AskAll, "ask_all"),
        (PermissionMode::Deny, "deny"),
        (PermissionMode::AllowAll, "allow_all"),
    ];
    for (mode, wire) in cases {
        let value = serde_json::to_value(mode).unwrap();
        assert_eq!(value, json!(wire), "wire name for {mode:?}");
        let back: PermissionMode = serde_json::from_value(value).unwrap();
        assert_eq!(back, mode, "round trip for {mode:?}");
    }
}

#[test]
fn unknown_mode_string_is_rejected() {
    assert!(serde_json::from_value::<PermissionMode>(json!("allow_everything")).is_err());
}

// ---- decide_auto(AllowAll): allow_once selection ----

#[test]
fn allow_all_selects_allow_once_for_a_write() {
    let req = request("edit", json!([allow_once(), reject_once()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Allow {
            option_id: "allow-once".to_string()
        }
    );
}

#[test]
fn allow_all_allows_every_tool_kind_when_allow_once_is_offered() {
    for kind in [
        "read",
        "edit",
        "delete",
        "move",
        "search",
        "execute",
        "think",
        "fetch",
        "switch_mode",
        "other",
        "weird_kind_from_agent",
    ] {
        let req = request(kind, json!([allow_once()]), None);
        assert_eq!(
            decide_auto(PermissionMode::AllowAll, &req),
            AutoDecision::Allow {
                option_id: "allow-once".to_string()
            },
            "kind {kind}"
        );
    }
}

#[test]
fn allow_all_never_selects_allow_always_even_when_listed_first() {
    let req = request("execute", json!([allow_always(), allow_once()]), None);
    match decide_auto(PermissionMode::AllowAll, &req) {
        AutoDecision::Allow { option_id } => {
            assert_eq!(option_id, "allow-once");
            assert_ne!(option_id, "allow-always");
        }
        other => panic!("expected the allow_once option, got {other:?}"),
    }
}

// ---- decide_auto(AllowAll): fall back to asking, never reject ----

#[test]
fn allow_all_with_only_allow_always_falls_back_to_asking() {
    let req = request("edit", json!([allow_always()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Ask
    );
}

#[test]
fn allow_all_with_only_reject_once_asks_and_never_rejects() {
    let req = request("edit", json!([reject_once()]), None);
    let decision = decide_auto(PermissionMode::AllowAll, &req);
    assert!(
        !matches!(decision, AutoDecision::Reject { .. }),
        "AllowAll must never auto-reject: {decision:?}"
    );
    assert_eq!(decision, AutoDecision::Ask);
}

#[test]
fn allow_all_with_reject_listed_first_still_selects_allow_once() {
    let req = request("edit", json!([reject_once(), allow_once()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Allow {
            option_id: "allow-once".to_string()
        }
    );
}

#[test]
fn allow_all_with_only_reject_always_asks() {
    let req = request("delete", json!([reject_always()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Ask
    );
}

#[test]
fn allow_all_with_no_options_asks() {
    let req = request("edit", json!([]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Ask
    );
}

#[test]
fn allow_all_with_only_unknown_option_kinds_asks() {
    let odd = json!({"optionId": "odd-1", "name": "Odd", "kind": "something_new"});
    let missing_kind = json!({"optionId": "odd-2", "name": "No kind"});
    let req = request("edit", json!([odd, missing_kind]), None);
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Ask
    );
}

// ---- AllowAll and the other fail-closed rules ----

#[test]
fn contradicting_raw_input_is_still_flagged_and_follows_the_allow_once_rule() {
    // A write-like rawInput key on an edit request. The flag is parsed as before.
    let req = request(
        "edit",
        json!([allow_once()]),
        Some(json!({"command": "rm -rf build"})),
    );
    assert!(
        req.raw_input_contradicts,
        "the contradiction flag must still be set"
    );
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Allow {
            option_id: "allow-once".to_string()
        }
    );
}

#[test]
fn unknown_details_do_not_change_the_allow_all_decision() {
    let req = request("edit", json!([allow_once()]), Some(json!({"mystery": 1})));
    assert!(
        req.details.is_none(),
        "unrecognised rawInput yields no details"
    );
    assert_eq!(
        decide_auto(PermissionMode::AllowAll, &req),
        AutoDecision::Allow {
            option_id: "allow-once".to_string()
        }
    );
}

#[test]
fn malformed_requests_still_fail_to_parse_so_they_are_cancelled() {
    let bad = [
        json!("not an object"),
        json!({"toolCall": {"toolCallId": "t1", "kind": "edit"}, "options": []}),
        json!({"sessionId": "s1", "options": []}),
        json!({"sessionId": "s1", "toolCall": {"kind": "edit"}, "options": "nope"}),
        json!({"sessionId": "s1", "toolCall": {"kind": "edit"}}),
    ];
    for params in bad {
        assert!(
            parse_permission_request(&params).is_err(),
            "malformed payload must not parse: {params}"
        );
    }
}

#[test]
fn timeout_and_cancel_still_choose_reject_for_an_allow_all_request() {
    let req = request("edit", json!([allow_once(), reject_once()]), None);
    assert_eq!(
        outcome_for(UserChoice::TimedOut, &req),
        Outcome::Selected("reject-once".to_string())
    );
    assert_eq!(
        outcome_for(UserChoice::Cancelled, &req),
        Outcome::Selected("reject-once".to_string())
    );
}

#[test]
fn user_allow_never_selects_allow_always() {
    let req = request("edit", json!([allow_always()]), None);
    assert_eq!(outcome_for(UserChoice::Allow, &req), Outcome::Cancelled);
}

// ---- the other modes are unchanged by AllowAll ----

#[test]
fn ask_writes_still_asks_for_a_contradicting_write() {
    let req = request(
        "edit",
        json!([allow_once(), reject_once()]),
        Some(json!({"command": "rm -rf build"})),
    );
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Ask
    );
}

#[test]
fn ask_writes_still_asks_for_a_plain_write() {
    let req = request("edit", json!([allow_once(), reject_once()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Ask
    );
}

#[test]
fn ask_writes_still_allows_a_read_with_allow_once() {
    let req = request("read", json!([allow_once(), reject_once()]), None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Allow {
            option_id: "allow-once".to_string()
        }
    );
}

#[test]
fn ask_all_still_asks_for_a_read() {
    let req = request("read", json!([allow_once(), reject_once()]), None);
    assert_eq!(decide_auto(PermissionMode::AskAll, &req), AutoDecision::Ask);
}

#[test]
fn deny_still_rejects_with_the_reject_once_option() {
    let req = request("edit", json!([allow_once(), reject_once()]), None);
    assert_eq!(
        decide_auto(PermissionMode::Deny, &req),
        AutoDecision::Reject {
            option_id: Some("reject-once".to_string())
        }
    );
}
