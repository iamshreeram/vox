//! Contract tests for the ACP permission approval payload and the respond helper
//! (DESIGN.md Revision 3: S4 "Approval dialog", C6). Red until step S4 creates
//! `managers/acp_permission_ui.rs`.
//! Registered in managers/mod.rs as `#[cfg(test)] mod acp_permission_ui_tests;`.
//! Workers must not edit this file.
//!
//! Commit waits are capped at 2 s and panic on timeout, so a stuck broker fails
//! the test instead of hanging the run.

use crate::managers::acp_permission::{
    OperationDetails, Outcome, PermOption, PermOptionKind, PermissionRequest, ToolKind,
};
use crate::managers::acp_permission_broker::{
    CommitFn, PermissionBroker, PermissionPrompter, PermissionView, RequestBinding, SubmitResult,
};
use crate::managers::acp_permission_ui::{
    build_closed_payload, build_request_payload, respond, EVENT_CLOSED, EVENT_REQUEST,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WAIT_CAP: Duration = Duration::from_secs(2);

fn details(summary: &str, truncated: bool) -> OperationDetails {
    OperationDetails {
        summary: summary.to_string(),
        truncated,
    }
}

fn view(details: Option<OperationDetails>, queued: usize) -> PermissionView {
    PermissionView {
        token: "perm-1".to_string(),
        title: "Run a command".to_string(),
        kind: ToolKind::Execute,
        details,
        remaining: Duration::from_secs(60),
        queued,
    }
}

// ---- payload builder ----

#[test]
fn payload_maps_all_fields() {
    let v = view(Some(details("command: rm -rf ~/x", true)), 2);
    let p = build_request_payload(&v, 60);
    assert_eq!(p.token, "perm-1");
    assert_eq!(p.title, "Run a command");
    assert_eq!(p.kind, "execute");
    assert_eq!(p.details.as_deref(), Some("command: rm -rf ~/x"));
    assert!(p.truncated);
    assert_eq!(p.timeout_secs, 60);
    assert_eq!(p.queued, 2);
}

#[test]
fn payload_details_none_is_null_in_json() {
    let p = build_request_payload(&view(None, 0), 60);
    let value = serde_json::to_value(&p).unwrap();
    assert!(value["details"].is_null(), "details must be null: {value}");
    assert_eq!(value["truncated"], json!(false));
    assert_eq!(value["timeoutSecs"], json!(60));
    assert_eq!(value["token"], json!("perm-1"));
    assert_eq!(value["queued"], json!(0));
}

#[test]
fn payload_truncated_flag_follows_details() {
    let not_cut = build_request_payload(&view(Some(details("path: /tmp/a", false)), 0), 60);
    assert!(!not_cut.truncated);
    let cut = build_request_payload(&view(Some(details("path: /tmp/a…", true)), 0), 60);
    assert!(cut.truncated);
}

#[test]
fn payload_kind_names_for_every_toolkind() {
    let cases = [
        (ToolKind::Read, "read"),
        (ToolKind::Edit, "edit"),
        (ToolKind::Delete, "delete"),
        (ToolKind::Move, "move"),
        (ToolKind::Search, "search"),
        (ToolKind::Execute, "execute"),
        (ToolKind::Think, "think"),
        (ToolKind::Fetch, "fetch"),
        (ToolKind::SwitchMode, "switch_mode"),
        (ToolKind::Other, "other"),
    ];
    for (kind, name) in cases {
        let mut v = view(None, 0);
        v.kind = kind;
        assert_eq!(build_request_payload(&v, 60).kind, name, "{kind:?}");
    }
}

#[test]
fn closed_payload_json_shape() {
    let p = build_closed_payload("perm-9");
    assert_eq!(p.token, "perm-9");
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        json!({"token": "perm-9"})
    );
}

#[test]
fn event_names_are_stable() {
    assert_eq!(EVENT_REQUEST, "acp-permission-request");
    assert_eq!(EVENT_CLOSED, "acp-permission-closed");
}

// ---- respond helper (UI answer -> broker single atomic resolve) ----

#[derive(Default)]
struct NullPrompter;

impl PermissionPrompter for NullPrompter {
    fn present(&self, _view: &PermissionView) {}
    fn close(&self, _token: &str) {}
}

type Commits = Arc<Mutex<Vec<(RequestBinding, Outcome)>>>;

fn broker_with_commits() -> (Arc<PermissionBroker>, Commits) {
    let commits: Commits = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&commits);
    let commit: CommitFn = Arc::new(move |b: RequestBinding, o: Outcome| {
        sink.lock().unwrap().push((b, o));
    });
    (
        PermissionBroker::new(Arc::new(NullPrompter), commit),
        commits,
    )
}

/// Request with allow_once `a1` and reject_once `r1` options.
fn request() -> PermissionRequest {
    PermissionRequest {
        session_id: "s1".to_string(),
        tool_call_id: Some("t1".to_string()),
        title: "Do thing".to_string(),
        kind: ToolKind::Edit,
        options: vec![
            PermOption {
                option_id: "a1".to_string(),
                name: "Allow once".to_string(),
                kind: PermOptionKind::AllowOnce,
            },
            PermOption {
                option_id: "r1".to_string(),
                name: "Reject once".to_string(),
                kind: PermOptionKind::RejectOnce,
            },
        ],
        raw_input_contradicts: false,
        details: None,
    }
}

fn binding() -> RequestBinding {
    RequestBinding {
        conn_gen: 1,
        run_token: 1,
        session_id: "s1".to_string(),
        rpc_id: json!(7),
    }
}

fn submit(broker: &PermissionBroker) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    match broker.submit(binding(), request(), deadline) {
        SubmitResult::Queued { token } => token,
        other => panic!("expected Queued, got {other:?}"),
    }
}

/// Waits until at least `n` outcomes were committed; panics after 2 s.
fn wait_commits(commits: &Commits, n: usize) {
    let deadline = Instant::now() + WAIT_CAP;
    loop {
        let len = commits.lock().unwrap().len();
        if len >= n {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {n} commits; have {len}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn respond_allow_resolves_pending_ticket_once() {
    let (broker, commits) = broker_with_commits();
    let token = submit(&broker);

    assert!(respond(&broker, &token, true));
    assert!(!respond(&broker, &token, true), "second answer must lose");

    wait_commits(&commits, 1);
    std::thread::sleep(Duration::from_millis(50));
    let all = commits.lock().unwrap().clone();
    assert_eq!(all.len(), 1, "outcome must be written exactly once");
    assert_eq!(all[0].0, binding());
    assert_eq!(all[0].1, Outcome::Selected("a1".to_string()));
}

#[test]
fn respond_deny_selects_reject_once() {
    let (broker, commits) = broker_with_commits();
    let token = submit(&broker);

    assert!(respond(&broker, &token, false));

    wait_commits(&commits, 1);
    let all = commits.lock().unwrap().clone();
    assert_eq!(all[0].1, Outcome::Selected("r1".to_string()));
}

#[test]
fn respond_unknown_token_false() {
    let (broker, _commits) = broker_with_commits();
    assert!(!respond(&broker, "perm-does-not-exist", true));
    assert!(!respond(&broker, "perm-does-not-exist", false));
}

#[test]
fn payload_serializes_to_json_object() {
    // Guards the wire shape the frontend listener depends on.
    let p = build_request_payload(&view(None, 1), 60);
    let value: Value = serde_json::to_value(&p).unwrap();
    assert!(value.is_object());
    for key in [
        "token",
        "title",
        "kind",
        "details",
        "truncated",
        "timeoutSecs",
        "queued",
    ] {
        assert!(value.get(key).is_some(), "missing key {key}: {value}");
    }
}
