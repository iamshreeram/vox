//! Contract tests for `acp_permission` (DESIGN.md Revision 3: M2 "Types (S1)",
//! "Trust boundary" and C6). Red until step S1 implements the module.
//! Workers must not edit this file.

use crate::managers::acp_permission::*;
use serde_json::json;

fn full_params() -> serde_json::Value {
    json!({
        "sessionId": "s1",
        "toolCall": {
            "toolCallId": "t1",
            "title": "Run tests",
            "kind": "execute",
            "rawInput": {"command": "cargo test"}
        },
        "options": [
            {"optionId": "a1", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "a2", "name": "Always", "kind": "allow_always"},
            {"optionId": "r1", "name": "Reject", "kind": "reject_once"}
        ]
    })
}

fn params_with_kind(kind: serde_json::Value) -> serde_json::Value {
    json!({
        "sessionId": "s1",
        "toolCall": {"toolCallId": "t1", "title": "Do thing", "kind": kind},
        "options": [{"optionId": "a1", "name": "Allow once", "kind": "allow_once"}]
    })
}

fn req_with(
    kind: &str,
    options: serde_json::Value,
    raw_input: Option<serde_json::Value>,
) -> PermissionRequest {
    let mut tool_call = json!({"toolCallId": "t1", "title": "Do thing", "kind": kind});
    if let Some(raw) = raw_input {
        tool_call["rawInput"] = raw;
    }
    let params = json!({"sessionId": "s1", "toolCall": tool_call, "options": options});
    parse_permission_request(&params)
        .ok()
        .expect("fixture must parse")
}

fn allow_once_opts() -> serde_json::Value {
    json!([
        {"optionId": "a1", "name": "Allow once", "kind": "allow_once"},
        {"optionId": "r1", "name": "Reject", "kind": "reject_once"}
    ])
}

fn allow_always_first_opts() -> serde_json::Value {
    json!([
        {"optionId": "a2", "name": "Always", "kind": "allow_always"},
        {"optionId": "a1", "name": "Allow once", "kind": "allow_once"},
        {"optionId": "r1", "name": "Reject", "kind": "reject_once"}
    ])
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

// ---- parsing -------------------------------------------------------------

#[test]
fn parse_ok_full_request() {
    let req = parse_permission_request(&full_params())
        .ok()
        .expect("must parse");
    assert_eq!(req.session_id, "s1");
    assert_eq!(req.tool_call_id, Some("t1".to_string()));
    assert_eq!(req.title, "Run tests");
    assert_eq!(req.kind, ToolKind::Execute);
    assert_eq!(req.options.len(), 3);
    assert_eq!(req.options[0].option_id, "a1");
    assert_eq!(req.options[0].kind, PermOptionKind::AllowOnce);
    assert_eq!(req.options[1].kind, PermOptionKind::AllowAlways);
    assert_eq!(req.options[2].kind, PermOptionKind::RejectOnce);
    // rawInput carries "command", a contradiction key.
    assert!(req.raw_input_contradicts);
    let details = req.details.expect("command rawInput yields details");
    assert!(details.summary.contains("command: cargo test"));
    assert!(!details.truncated);
}

#[test]
fn parse_rejects_non_object() {
    assert!(parse_permission_request(&json!("x")).is_err());
    assert!(parse_permission_request(&json!([])).is_err());
    assert!(parse_permission_request(&json!(null)).is_err());
    assert!(parse_permission_request(&json!(42)).is_err());
}

#[test]
fn parse_rejects_missing_tool_call() {
    let mut p = full_params();
    p.as_object_mut().expect("object").remove("toolCall");
    assert!(parse_permission_request(&p).is_err());

    let mut p = full_params();
    p["toolCall"] = json!("not-an-object");
    assert!(parse_permission_request(&p).is_err());
}

#[test]
fn parse_rejects_missing_session() {
    let mut p = full_params();
    p.as_object_mut().expect("object").remove("sessionId");
    assert!(parse_permission_request(&p).is_err());

    let mut p = full_params();
    p["sessionId"] = json!(5);
    assert!(parse_permission_request(&p).is_err());
}

#[test]
fn parse_rejects_missing_or_non_array_options() {
    let mut p = full_params();
    p.as_object_mut().expect("object").remove("options");
    assert!(parse_permission_request(&p).is_err());

    let mut p = full_params();
    p["options"] = json!({"optionId": "a1"});
    assert!(parse_permission_request(&p).is_err());

    let mut p = full_params();
    p["options"] = json!("allow");
    assert!(parse_permission_request(&p).is_err());
}

#[test]
fn parse_skips_option_without_id() {
    let mut p = full_params();
    p["options"] = json!([
        {"name": "no id", "kind": "allow_once"},
        {"optionId": 7, "name": "numeric id", "kind": "allow_once"},
        {"optionId": "ok", "name": "Reject", "kind": "reject_once"}
    ]);
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert_eq!(req.options.len(), 1);
    assert_eq!(req.options[0].option_id, "ok");
    assert_eq!(req.options[0].kind, PermOptionKind::RejectOnce);
}

#[test]
fn parse_unknown_and_missing_tool_kind_is_other() {
    for raw in [json!("bogus"), json!(42), json!(null)] {
        let req = parse_permission_request(&params_with_kind(raw.clone()))
            .ok()
            .expect("must parse");
        assert_eq!(req.kind, ToolKind::Other, "kind value {raw}");
    }
    let mut p = params_with_kind(json!("read"));
    p["toolCall"]
        .as_object_mut()
        .expect("object")
        .remove("kind");
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert_eq!(req.kind, ToolKind::Other);
}

#[test]
fn parse_all_tool_kind_strings_map() {
    let pairs = [
        ("read", ToolKind::Read),
        ("edit", ToolKind::Edit),
        ("delete", ToolKind::Delete),
        ("move", ToolKind::Move),
        ("search", ToolKind::Search),
        ("execute", ToolKind::Execute),
        ("think", ToolKind::Think),
        ("fetch", ToolKind::Fetch),
        ("switch_mode", ToolKind::SwitchMode),
        ("other", ToolKind::Other),
    ];
    for (text, expected) in pairs {
        let req = parse_permission_request(&params_with_kind(json!(text)))
            .ok()
            .expect("must parse");
        assert_eq!(req.kind, expected, "kind string {text}");
    }
}

#[test]
fn parse_unknown_option_kind_is_unknown() {
    let mut p = full_params();
    p["options"] = json!([
        {"optionId": "x1", "name": "Weird", "kind": "bogus_kind"},
        {"optionId": "x2", "name": "No kind"},
        {"optionId": "x3", "name": "Always", "kind": "allow_always"},
        {"optionId": "x4", "name": "Reject always", "kind": "reject_always"}
    ]);
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert_eq!(req.options.len(), 4);
    assert_eq!(req.options[0].kind, PermOptionKind::Unknown);
    assert_eq!(req.options[1].kind, PermOptionKind::Unknown);
    assert_eq!(req.options[2].kind, PermOptionKind::AllowAlways);
    assert_eq!(req.options[3].kind, PermOptionKind::RejectAlways);
}

#[test]
fn parse_title_sanitized_and_bounded() {
    let mut p = full_params();
    p["toolCall"]["title"] = json!("ab\n\u{1b}cd\t\u{7}".repeat(2000));
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert!(req.title.chars().count() <= 160, "title too long");
    assert!(!has_control(&req.title), "title has control chars");
    assert!(!req.title.is_empty());
}

#[test]
fn parse_contradiction_keys_case_insensitive() {
    let upper = req_with("read", allow_once_opts(), Some(json!({"Command": "x"})));
    assert!(upper.raw_input_contradicts);
    let path_only = req_with("read", allow_once_opts(), Some(json!({"path": "/a"})));
    assert!(!path_only.raw_input_contradicts);
    let absent = req_with("read", allow_once_opts(), None);
    assert!(!absent.raw_input_contradicts);
}

// ---- decide_auto ---------------------------------------------------------

#[test]
fn decide_read_with_allow_once_is_allowed() {
    let req = req_with("read", allow_once_opts(), Some(json!({"path": "/tmp/x"})));
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Allow {
            option_id: "a1".to_string()
        }
    );
}

#[test]
fn decide_search_and_think_allowed() {
    for kind in ["search", "think"] {
        let req = req_with(kind, allow_once_opts(), None);
        assert_eq!(
            decide_auto(PermissionMode::AskWrites, &req),
            AutoDecision::Allow {
                option_id: "a1".to_string()
            },
            "kind {kind}"
        );
    }
}

#[test]
fn decide_read_without_allow_once_asks() {
    let no_allow = json!([{"optionId": "r1", "name": "Reject", "kind": "reject_once"}]);
    let req = req_with("read", no_allow, None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Ask
    );

    let empty = req_with("read", json!([]), None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &empty),
        AutoDecision::Ask
    );
}

#[test]
fn decide_read_labelled_execute_fixture_asks() {
    let req = req_with(
        "read",
        allow_once_opts(),
        Some(json!({"command": "rm -rf x"})),
    );
    assert!(req.raw_input_contradicts);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Ask
    );
}

#[test]
fn decide_write_kinds_always_ask() {
    for kind in [
        "edit",
        "delete",
        "move",
        "execute",
        "fetch",
        "switch_mode",
        "other",
    ] {
        let req = req_with(kind, allow_once_opts(), None);
        assert_eq!(
            decide_auto(PermissionMode::AskWrites, &req),
            AutoDecision::Ask,
            "kind {kind} must ask"
        );
    }
}

#[test]
fn decide_missing_kind_asks() {
    let mut p = params_with_kind(json!("read"));
    p["toolCall"]
        .as_object_mut()
        .expect("object")
        .remove("kind");
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Ask
    );

    let bogus = req_with("bogus", allow_once_opts(), None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &bogus),
        AutoDecision::Ask
    );
}

#[test]
fn decide_ask_all_always_asks() {
    for kind in ["read", "search", "think", "execute"] {
        let req = req_with(kind, allow_once_opts(), None);
        assert_eq!(
            decide_auto(PermissionMode::AskAll, &req),
            AutoDecision::Ask,
            "kind {kind}"
        );
    }
}

#[test]
fn decide_deny_rejects_with_reject_once() {
    let req = req_with("execute", allow_once_opts(), None);
    assert_eq!(
        decide_auto(PermissionMode::Deny, &req),
        AutoDecision::Reject {
            option_id: Some("r1".to_string())
        }
    );
}

#[test]
fn decide_deny_without_reject_option_has_none() {
    let only_allow = req_with(
        "execute",
        json!([{"optionId": "a1", "name": "A", "kind": "allow_once"}]),
        None,
    );
    assert_eq!(
        decide_auto(PermissionMode::Deny, &only_allow),
        AutoDecision::Reject { option_id: None }
    );

    // RejectAlways is never selected by Vox, so it must not count as a reject option.
    let only_reject_always = req_with(
        "execute",
        json!([{"optionId": "r2", "name": "Never", "kind": "reject_always"}]),
        None,
    );
    assert_eq!(
        decide_auto(PermissionMode::Deny, &only_reject_always),
        AutoDecision::Reject { option_id: None }
    );
}

#[test]
fn decide_never_selects_allow_always() {
    let req = req_with("read", allow_always_first_opts(), None);
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &req),
        AutoDecision::Allow {
            option_id: "a1".to_string()
        }
    );

    let only_always = req_with(
        "read",
        json!([{"optionId": "a2", "name": "Always", "kind": "allow_always"}]),
        None,
    );
    assert_eq!(
        decide_auto(PermissionMode::AskWrites, &only_always),
        AutoDecision::Ask
    );
}

// ---- outcome_for / outcome_json ------------------------------------------

#[test]
fn outcome_allow_picks_allow_once_only() {
    let req = req_with("edit", allow_always_first_opts(), None);
    assert_eq!(
        outcome_for(UserChoice::Allow, &req),
        Outcome::Selected("a1".to_string())
    );
}

#[test]
fn outcome_allow_with_only_allow_always_is_cancelled() {
    let req = req_with(
        "edit",
        json!([
            {"optionId": "a2", "name": "Always", "kind": "allow_always"},
            {"optionId": "r1", "name": "Reject", "kind": "reject_once"}
        ]),
        None,
    );
    assert_eq!(outcome_for(UserChoice::Allow, &req), Outcome::Cancelled);
}

#[test]
fn outcome_reject_picks_reject_once() {
    let req = req_with(
        "edit",
        json!([
            {"optionId": "a2", "name": "Always", "kind": "allow_always"},
            {"optionId": "r9", "name": "Never", "kind": "reject_always"},
            {"optionId": "r2", "name": "Reject", "kind": "reject_once"}
        ]),
        None,
    );
    assert_eq!(
        outcome_for(UserChoice::Reject, &req),
        Outcome::Selected("r2".to_string())
    );

    let always_only = req_with(
        "edit",
        json!([{"optionId": "r9", "name": "Never", "kind": "reject_always"}]),
        None,
    );
    assert_eq!(
        outcome_for(UserChoice::Reject, &always_only),
        Outcome::Cancelled
    );
}

#[test]
fn outcome_timed_out_and_cancelled_reject_or_cancel() {
    let with_reject = req_with("edit", allow_once_opts(), None);
    assert_eq!(
        outcome_for(UserChoice::TimedOut, &with_reject),
        Outcome::Selected("r1".to_string())
    );
    assert_eq!(
        outcome_for(UserChoice::Cancelled, &with_reject),
        Outcome::Selected("r1".to_string())
    );

    let no_reject = req_with(
        "edit",
        json!([{"optionId": "a1", "name": "Allow once", "kind": "allow_once"}]),
        None,
    );
    assert_eq!(
        outcome_for(UserChoice::TimedOut, &no_reject),
        Outcome::Cancelled
    );
    assert_eq!(
        outcome_for(UserChoice::Cancelled, &no_reject),
        Outcome::Cancelled
    );
}

#[test]
fn outcome_json_shapes() {
    assert_eq!(
        outcome_json(&Outcome::Selected("a1".to_string())),
        json!({"outcome": {"outcome": "selected", "optionId": "a1"}})
    );
    assert_eq!(
        outcome_json(&Outcome::Cancelled),
        json!({"outcome": {"outcome": "cancelled"}})
    );
}

// ---- PermissionMode -------------------------------------------------------

#[test]
fn permission_mode_default_is_ask_writes_and_serde_snake_case() {
    assert_eq!(PermissionMode::default(), PermissionMode::AskWrites);

    assert_eq!(
        serde_json::to_value(PermissionMode::AskWrites).ok(),
        Some(json!("ask_writes"))
    );
    assert_eq!(
        serde_json::to_value(PermissionMode::AskAll).ok(),
        Some(json!("ask_all"))
    );
    assert_eq!(
        serde_json::to_value(PermissionMode::Deny).ok(),
        Some(json!("deny"))
    );

    assert_eq!(
        serde_json::from_value::<PermissionMode>(json!("ask_writes")).ok(),
        Some(PermissionMode::AskWrites)
    );
    assert_eq!(
        serde_json::from_value::<PermissionMode>(json!("ask_all")).ok(),
        Some(PermissionMode::AskAll)
    );
    assert_eq!(
        serde_json::from_value::<PermissionMode>(json!("deny")).ok(),
        Some(PermissionMode::Deny)
    );
    assert!(serde_json::from_value::<PermissionMode>(json!("AskAll")).is_err());
}

// ---- sanitize_text --------------------------------------------------------

#[test]
fn sanitize_text_strips_controls_and_bounds() {
    let (s, truncated): (String, bool) = sanitize_text("  a\tb\nc\u{7}d\u{1b}[0m  ", 100);
    assert!(!truncated);
    assert_eq!(s, "a b cd[0m");
    assert!(!has_control(&s));

    let (long, truncated): (String, bool) = sanitize_text(&"x".repeat(50), 10);
    assert!(truncated);
    assert_eq!(long.chars().count(), 10);
    assert_eq!(long, format!("{}\u{2026}", "x".repeat(9)));
}

#[test]
fn sanitize_text_short_unchanged_not_truncated() {
    assert_eq!(
        sanitize_text("hello world", 11),
        ("hello world".to_string(), false)
    );
    assert_eq!(
        sanitize_text("hello world!", 11),
        (format!("hello worl{}", "\u{2026}"), true)
    );
}

// ---- details (C6) --------------------------------------------------------

#[test]
fn details_execute_generic_title_shows_command() {
    let mut p = full_params();
    p["toolCall"]["title"] = json!("Run tool");
    p["toolCall"]["rawInput"] = json!({"command": "rm -rf ~/x"});
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert_eq!(req.title, "Run tool");
    let details = req.details.expect("details present");
    assert!(
        details.summary.contains("command: rm -rf ~/x"),
        "summary: {}",
        details.summary
    );
}

#[test]
fn details_file_target_path() {
    let req = req_with(
        "edit",
        allow_once_opts(),
        Some(json!({"path": "/tmp/a.txt"})),
    );
    let details = req.details.expect("details present");
    assert!(
        details.summary.contains("path: /tmp/a.txt"),
        "summary: {}",
        details.summary
    );

    let alt = extract_details(&json!({"file_path": "/tmp/b"})).expect("file_path recognised");
    assert!(alt.summary.contains("path: /tmp/b"));
}

#[test]
fn details_command_array_joined() {
    let joined = extract_details(&json!({"command": ["git", "status", "-s"]}))
        .expect("array command recognised");
    assert!(
        joined.summary.contains("command: git status -s"),
        "summary: {}",
        joined.summary
    );

    let cmd = extract_details(&json!({"cmd": "ls"})).expect("cmd recognised");
    assert!(cmd.summary.contains("command: ls"));

    let paths = extract_details(&json!({"paths": ["/a", "/b"]})).expect("paths recognised");
    assert!(
        paths.summary.contains("path: /a, /b"),
        "summary: {}",
        paths.summary
    );
}

#[test]
fn details_oversized_and_control_chars_bounded() {
    let raw = json!({
        "command": "rm -rf\u{7}\n".repeat(1000),
        "path": "p\u{1b}".repeat(5000)
    });
    let details = extract_details(&raw).expect("recognised fields present");
    assert!(
        details.truncated,
        "oversized input must be marked truncated"
    );
    assert!(
        details.summary.chars().count() <= 400,
        "summary exceeds 400 chars"
    );
    assert!(!details.summary.is_empty());
    for line in details.summary.split('\n') {
        assert!(!has_control(line), "control char in line: {line:?}");
        let value = line.splitn(2, ": ").nth(1).unwrap_or("");
        assert!(value.chars().count() <= 200, "field exceeds 200 chars");
    }
}

#[test]
fn details_missing_or_unrecognised_is_none() {
    let mut p = full_params();
    p["toolCall"]
        .as_object_mut()
        .expect("object")
        .remove("rawInput");
    let req = parse_permission_request(&p).ok().expect("must parse");
    assert!(req.details.is_none());

    assert!(extract_details(&json!(null)).is_none());
    assert!(extract_details(&json!("rm -rf")).is_none());
    assert!(extract_details(&json!(["command", "ls"])).is_none());
    assert!(extract_details(&json!({"foo": "bar", "zzz": 1})).is_none());
}

#[test]
fn details_keys_case_insensitive() {
    let details = extract_details(&json!({"COMMAND": "ls -la", "File_Path": "/x"}))
        .expect("case-insensitive keys recognised");
    assert!(
        details.summary.contains("command: ls -la"),
        "summary: {}",
        details.summary
    );
    assert!(
        details.summary.contains("path: /x"),
        "summary: {}",
        details.summary
    );
}
