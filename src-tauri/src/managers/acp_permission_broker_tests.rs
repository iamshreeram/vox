//! Contract tests for `acp_permission_broker` (DESIGN.md Revision 3: M2
//! "Broker" and C3). Red until step S2 implements the module.
//! Workers must not edit this file.
//!
//! Every wait is capped (2 s) and panics on timeout, so a deadlock fails the
//! test instead of hanging the run.

#![allow(dead_code)]

use crate::managers::acp_permission::{
    Outcome, PermOption, PermOptionKind, PermissionRequest, ToolKind, UserChoice,
};
use crate::managers::acp_permission_broker::{
    CommitFn, PermissionBroker, PermissionPrompter, PermissionView, RequestBinding, SubmitResult,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

const WAIT_CAP: Duration = Duration::from_secs(2);
const SETTLE: Duration = Duration::from_millis(80);

/// Records presentation events (`present:<token>`, `close:<token>`) in order.
#[derive(Default)]
struct FakePrompter {
    events: Mutex<Vec<(Instant, String)>>,
    views: Mutex<Vec<PermissionView>>,
    cv: Condvar,
}

impl FakePrompter {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn log(&self, event: String) {
        self.events.lock().unwrap().push((Instant::now(), event));
        self.cv.notify_all();
    }

    fn names(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    fn count(&self, event: &str) -> usize {
        self.names().iter().filter(|e| e.as_str() == event).count()
    }

    fn count_prefix(&self, prefix: &str) -> usize {
        self.names()
            .iter()
            .filter(|e| e.starts_with(prefix))
            .count()
    }

    fn position(&self, event: &str) -> Option<usize> {
        self.names().iter().position(|e| e.as_str() == event)
    }

    /// Blocks until `event` is recorded; panics (a failure, not a hang) after 2 s.
    fn wait_for(&self, event: &str) -> Instant {
        let deadline = Instant::now() + WAIT_CAP;
        let mut guard = self.events.lock().unwrap();
        loop {
            if let Some((at, _)) = guard.iter().find(|(_, e)| e.as_str() == event) {
                return *at;
            }
            let now = Instant::now();
            if now >= deadline {
                let seen: Vec<String> = guard.iter().map(|(_, e)| e.clone()).collect();
                panic!("timed out waiting for event {event:?}; seen {seen:?}");
            }
            guard = self.cv.wait_timeout(guard, deadline - now).unwrap().0;
        }
    }

    fn view_of(&self, token: &str) -> PermissionView {
        self.views
            .lock()
            .unwrap()
            .iter()
            .find(|v| v.token == token)
            .cloned()
            .unwrap_or_else(|| panic!("no view was presented for {token}"))
    }
}

impl PermissionPrompter for FakePrompter {
    fn present(&self, view: &PermissionView) {
        self.views.lock().unwrap().push(view.clone());
        self.log(format!("present:{}", view.token));
    }

    fn close(&self, token: &str) {
        self.log(format!("close:{token}"));
    }
}

/// Records committed `(binding, outcome)` pairs.
#[derive(Default)]
struct Commits {
    log: Mutex<Vec<(RequestBinding, Outcome)>>,
    cv: Condvar,
}

impl Commits {
    fn push(&self, binding: RequestBinding, outcome: Outcome) {
        self.log.lock().unwrap().push((binding, outcome));
        self.cv.notify_all();
    }

    fn snapshot(&self) -> Vec<(RequestBinding, Outcome)> {
        self.log.lock().unwrap().clone()
    }

    /// Waits until at least `n` commits exist; panics after 2 s.
    fn wait_len(&self, n: usize) -> Vec<(RequestBinding, Outcome)> {
        let deadline = Instant::now() + WAIT_CAP;
        let mut guard = self.log.lock().unwrap();
        while guard.len() < n {
            let now = Instant::now();
            if now >= deadline {
                panic!("timed out waiting for {n} commits; have {:?}", &*guard);
            }
            guard = self.cv.wait_timeout(guard, deadline - now).unwrap().0;
        }
        guard.clone()
    }
}

fn commit_into(commits: Arc<Commits>) -> CommitFn {
    let f = move |b: RequestBinding, o: Outcome| commits.push(b, o);
    Arc::new(f)
}

struct Harness {
    p: Arc<FakePrompter>,
    c: Arc<Commits>,
    b: Arc<PermissionBroker>,
}

fn harness() -> Harness {
    let p = FakePrompter::new();
    let c = Arc::new(Commits::default());
    let b = PermissionBroker::new(p.clone(), commit_into(c.clone()));
    Harness { p, c, b }
}

/// Request with allow_once `a1` and reject_once `r1` options.
fn req(kind: ToolKind) -> PermissionRequest {
    PermissionRequest {
        session_id: "s1".to_string(),
        tool_call_id: Some("t1".to_string()),
        title: "Do thing".to_string(),
        kind,
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

fn binding(conn_gen: u64, run_token: u64, rpc_id: Value) -> RequestBinding {
    RequestBinding {
        conn_gen,
        run_token,
        session_id: "s1".to_string(),
        rpc_id,
    }
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn soon(ms: u64) -> Instant {
    Instant::now() + Duration::from_millis(ms)
}

fn token_of(result: SubmitResult) -> String {
    match result {
        SubmitResult::Queued { token } => token,
        other => panic!("expected Queued, got {other:?}"),
    }
}

#[test]
fn submit_queues_and_presents_first() {
    let h = harness();
    let tok = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert_eq!(h.b.pending_len(), 1);
    assert_eq!(h.p.count_prefix("present:"), 1);
    assert!(h.b.resolve(&tok, UserChoice::Reject));
    h.c.wait_len(1);
}

#[test]
fn resolve_allow_commits_selected_allow_once() {
    let h = harness();
    let bind = binding(1, 1, json!(1));
    let tok = token_of(h.b.submit(bind.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert!(h.b.resolve(&tok, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    assert_eq!(commits, vec![(bind, Outcome::Selected("a1".to_string()))]);
    h.p.wait_for(&format!("close:{tok}"));
    std::thread::sleep(SETTLE);
    assert_eq!(h.p.count(&format!("close:{tok}")), 1);
}

#[test]
fn resolve_reject_commits_reject_once() {
    let h = harness();
    let bind = binding(1, 1, json!(1));
    let tok = token_of(h.b.submit(bind.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert!(h.b.resolve(&tok, UserChoice::Reject));
    let commits = h.c.wait_len(1);
    assert_eq!(commits, vec![(bind, Outcome::Selected("r1".to_string()))]);
}

#[test]
fn resolve_twice_second_false_commit_once() {
    let h = harness();
    let tok = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert!(h.b.resolve(&tok, UserChoice::Allow));
    assert!(!h.b.resolve(&tok, UserChoice::Reject));
    let commits = h.c.wait_len(1);
    std::thread::sleep(SETTLE);
    assert_eq!(h.c.snapshot().len(), 1);
    assert_eq!(commits[0].1, Outcome::Selected("a1".to_string()));
}

#[test]
fn resolve_unknown_token_false() {
    let h = harness();
    let tok = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert!(!h.b.resolve("no-such-token", UserChoice::Allow));
    std::thread::sleep(SETTLE);
    assert!(h.c.snapshot().is_empty());
    assert!(h.b.resolve(&tok, UserChoice::Allow));
    h.c.wait_len(1);
}

#[test]
fn duplicate_conn_and_rpc_id_not_overwritten() {
    let h = harness();
    let first = binding(1, 1, json!(7));
    let tok = token_of(h.b.submit(first.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    let dup =
        h.b.submit(binding(1, 1, json!(7)), req(ToolKind::Read), far());
    assert_eq!(dup, SubmitResult::Duplicate);
    assert!(h.b.resolve(&tok, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    std::thread::sleep(SETTLE);
    assert_eq!(h.c.snapshot().len(), 1);
    assert_eq!(commits[0], (first, Outcome::Selected("a1".to_string())));
    assert_eq!(h.p.view_of(&tok).kind, ToolKind::Edit);
}

#[test]
fn numeric_one_and_string_one_are_distinct_ids() {
    let h = harness();
    let tok1 = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    let tok2 = token_of(h.b.submit(binding(1, 1, json!("1")), req(ToolKind::Edit), far()));
    assert_ne!(tok1, tok2);
    assert_eq!(h.b.pending_len(), 2);
    h.p.wait_for(&format!("present:{tok1}"));
    assert!(h.b.resolve(&tok1, UserChoice::Allow));
    h.p.wait_for(&format!("present:{tok2}"));
    assert!(h.b.resolve(&tok2, UserChoice::Allow));
    let commits = h.c.wait_len(2);
    let ids: Vec<Value> = commits.iter().map(|(b, _)| b.rpc_id.clone()).collect();
    assert!(ids.contains(&json!(1)));
    assert!(ids.contains(&json!("1")));
}

#[test]
fn same_rpc_id_after_reconnect_is_new_request() {
    let h = harness();
    let first = binding(1, 1, json!(1));
    let second = binding(2, 1, json!(1));
    let tok1 = token_of(h.b.submit(first.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok1}"));
    let tok2 = token_of(h.b.submit(second.clone(), req(ToolKind::Edit), far()));
    assert_ne!(tok1, tok2);
    assert!(h.b.resolve(&tok1, UserChoice::Allow));
    h.p.wait_for(&format!("present:{tok2}"));
    assert!(h.b.resolve(&tok2, UserChoice::Allow));
    let commits = h.c.wait_len(2);
    assert_eq!(commits[0].0, first);
    assert_eq!(commits[1].0, second);
}

#[test]
fn invalidate_conn_drops_only_old_conn_no_commit() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    let tok_b = token_of(h.b.submit(binding(1, 1, json!(2)), req(ToolKind::Edit), far()));
    let bind_c = binding(2, 1, json!(3));
    let tok_c = token_of(h.b.submit(bind_c.clone(), req(ToolKind::Edit), far()));
    assert_eq!(h.b.invalidate_conn(1), 2);
    h.p.wait_for(&format!("close:{tok_a}"));
    h.p.wait_for(&format!("present:{tok_c}"));
    assert!(!h.b.resolve(&tok_a, UserChoice::Allow));
    assert!(h.b.resolve(&tok_c, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    std::thread::sleep(SETTLE);
    assert_eq!(h.c.snapshot().len(), 1);
    assert_eq!(commits[0], (bind_c, Outcome::Selected("a1".to_string())));
    assert_eq!(h.p.count(&format!("present:{tok_b}")), 0);
    assert_eq!(h.p.count(&format!("close:{tok_a}")), 1);
}

#[test]
fn one_prompt_at_a_time() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    let tok_b = token_of(h.b.submit(binding(1, 1, json!(2)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    std::thread::sleep(SETTLE);
    assert_eq!(h.p.count_prefix("present:"), 1);
    assert_eq!(h.p.count(&format!("present:{tok_b}")), 0);
    assert!(h.b.resolve(&tok_a, UserChoice::Allow));
    h.p.wait_for(&format!("present:{tok_b}"));
    let close_a =
        h.p.position(&format!("close:{tok_a}"))
            .expect("close:A recorded");
    let present_b =
        h.p.position(&format!("present:{tok_b}"))
            .expect("present:B recorded");
    assert!(
        close_a < present_b,
        "B presented before A closed: {:?}",
        h.p.names()
    );
    assert!(h.b.resolve(&tok_b, UserChoice::Allow));
    h.c.wait_len(2);
}

#[test]
fn overflow_beyond_three_queued_commits_cancelled_without_present() {
    let h = harness();
    let mut tokens = Vec::new();
    for i in 1..=4u64 {
        tokens.push(token_of(h.b.submit(
            binding(1, 1, json!(i)),
            req(ToolKind::Edit),
            far(),
        )));
    }
    let overflow = binding(1, 1, json!(5));
    assert_eq!(
        h.b.submit(overflow.clone(), req(ToolKind::Edit), far()),
        SubmitResult::Overflow
    );
    let commits = h.c.wait_len(1);
    assert_eq!(commits, vec![(overflow, Outcome::Cancelled)]);
    for tok in &tokens {
        h.p.wait_for(&format!("present:{tok}"));
        assert!(h.b.resolve(tok, UserChoice::Allow));
    }
    h.c.wait_len(5);
    std::thread::sleep(SETTLE);
    assert_eq!(h.p.count_prefix("present:"), 4);
}

#[test]
fn queued_count_in_view() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    assert_eq!(h.p.view_of(&tok_a).queued, 0);
    let tok_b = token_of(h.b.submit(binding(1, 1, json!(2)), req(ToolKind::Edit), far()));
    let tok_c = token_of(h.b.submit(binding(1, 1, json!(3)), req(ToolKind::Edit), far()));
    assert!(h.b.resolve(&tok_a, UserChoice::Allow));
    h.p.wait_for(&format!("present:{tok_b}"));
    // C is still queued behind B when B is presented.
    assert_eq!(h.p.view_of(&tok_b).queued, 1);
    assert!(h.b.resolve(&tok_b, UserChoice::Allow));
    h.p.wait_for(&format!("present:{tok_c}"));
    assert_eq!(h.p.view_of(&tok_c).queued, 0);
    assert!(h.b.resolve(&tok_c, UserChoice::Allow));
    h.c.wait_len(3);
}

#[test]
fn timeout_commits_timed_out_outcome_and_closes_once() {
    let h = harness();
    let bind = binding(1, 1, json!(1));
    let tok = token_of(h.b.submit(bind.clone(), req(ToolKind::Edit), soon(80)));
    h.p.wait_for(&format!("present:{tok}"));
    let commits = h.c.wait_len(1);
    // TimedOut maps to reject_once.
    assert_eq!(commits, vec![(bind, Outcome::Selected("r1".to_string()))]);
    h.p.wait_for(&format!("close:{tok}"));
    std::thread::sleep(SETTLE);
    assert_eq!(h.p.count(&format!("close:{tok}")), 1);
    assert!(!h.b.resolve(&tok, UserChoice::Allow));
}

#[test]
fn expired_queued_ticket_is_never_presented() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    let bind_b = binding(1, 1, json!(2));
    let tok_b = token_of(h.b.submit(bind_b.clone(), req(ToolKind::Edit), soon(40)));
    // B's deadline passes while A is still presented.
    std::thread::sleep(Duration::from_millis(120));
    assert!(h.b.resolve(&tok_a, UserChoice::Allow));
    let commits = h.c.wait_len(2);
    assert!(
        commits.contains(&(bind_b, Outcome::Selected("r1".to_string()))),
        "expired ticket B must commit TimedOut (reject_once): {commits:?}"
    );
    assert_eq!(h.p.count(&format!("present:{tok_b}")), 0);
}

#[test]
fn invalidate_run_wakes_presented_wait_immediately() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    let started = Instant::now();
    assert_eq!(h.b.invalidate_run(1), 1);
    h.p.wait_for(&format!("close:{tok_a}"));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "worker did not wake promptly"
    );
    let bind_c = binding(1, 2, json!(1));
    let tok_c = token_of(h.b.submit(bind_c.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_c}"));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "new run not presented promptly"
    );
    assert_eq!(h.p.count(&format!("close:{tok_a}")), 1);
    assert!(!h.b.resolve(&tok_a, UserChoice::Allow));
    assert!(h.b.resolve(&tok_c, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    std::thread::sleep(SETTLE);
    assert_eq!(h.c.snapshot().len(), 1);
    assert_eq!(commits[0], (bind_c, Outcome::Selected("a1".to_string())));
}

#[test]
fn invalidate_run_drops_queued_without_commit_or_present() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    let tok_b = token_of(h.b.submit(binding(1, 1, json!(2)), req(ToolKind::Edit), far()));
    let bind_c = binding(1, 2, json!(3));
    let tok_c = token_of(h.b.submit(bind_c.clone(), req(ToolKind::Edit), far()));
    assert_eq!(h.b.invalidate_run(1), 2);
    h.p.wait_for(&format!("close:{tok_a}"));
    h.p.wait_for(&format!("present:{tok_c}"));
    assert!(!h.b.resolve(&tok_b, UserChoice::Allow));
    assert!(h.b.resolve(&tok_c, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    std::thread::sleep(SETTLE);
    assert_eq!(h.c.snapshot().len(), 1);
    assert_eq!(commits[0], (bind_c, Outcome::Selected("a1".to_string())));
    assert_eq!(h.p.count(&format!("present:{tok_b}")), 0);
    assert_eq!(h.p.count(&format!("close:{tok_b}")), 0);
}

#[test]
fn invalidate_run_wrong_run_token_is_noop() {
    let h = harness();
    let tok = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok}"));
    assert_eq!(h.b.invalidate_run(99), 0);
    std::thread::sleep(SETTLE);
    assert_eq!(h.p.count(&format!("close:{tok}")), 0);
    assert!(h.b.resolve(&tok, UserChoice::Allow));
    let commits = h.c.wait_len(1);
    assert_eq!(commits[0].1, Outcome::Selected("a1".to_string()));
}

#[test]
fn close_exactly_once_on_resolve_timeout_and_invalidate() {
    let h = harness();
    let tok1 = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok1}"));
    assert!(h.b.resolve(&tok1, UserChoice::Allow));
    h.c.wait_len(1);

    let tok2 = token_of(h.b.submit(binding(1, 2, json!(2)), req(ToolKind::Edit), soon(60)));
    h.p.wait_for(&format!("present:{tok2}"));
    h.c.wait_len(2);

    let tok3 = token_of(h.b.submit(binding(1, 3, json!(3)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok3}"));
    assert_eq!(h.b.invalidate_run(3), 1);
    h.p.wait_for(&format!("close:{tok3}"));

    std::thread::sleep(SETTLE);
    for tok in [&tok1, &tok2, &tok3] {
        assert_eq!(
            h.p.count(&format!("close:{tok}")),
            1,
            "close count for {tok}"
        );
        assert_eq!(
            h.p.count(&format!("present:{tok}")),
            1,
            "present count for {tok}"
        );
    }
    // Invalidated ticket 3 must not commit.
    assert_eq!(h.c.snapshot().len(), 2);
}

#[test]
fn callback_may_reenter_broker_without_deadlock() {
    let p = FakePrompter::new();
    let slot: Arc<OnceLock<Weak<PermissionBroker>>> = Arc::new(OnceLock::new());
    let (tx, rx) = mpsc::channel::<(usize, SubmitResult)>();
    let tx = Mutex::new(tx);
    let once = AtomicBool::new(false);
    let slot_cb = slot.clone();
    let commit: CommitFn = Arc::new(move |_b: RequestBinding, _o: Outcome| {
        if once.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(broker) = slot_cb.get().and_then(Weak::upgrade) else {
            return;
        };
        let pending = broker.pending_len();
        let result = broker.submit(binding(9, 9, json!(9)), req(ToolKind::Read), far());
        let _ = tx.lock().unwrap().send((pending, result));
    });
    let broker = PermissionBroker::new(p.clone(), commit);
    assert!(slot.set(Arc::downgrade(&broker)).is_ok());

    let tok_a = token_of(broker.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    p.wait_for(&format!("present:{tok_a}"));
    assert!(broker.resolve(&tok_a, UserChoice::Allow));
    let (pending, result) = rx
        .recv_timeout(WAIT_CAP)
        .expect("commit callback never returned: broker deadlocked");
    assert!(pending <= 1, "pending_len inside callback was {pending}");
    let tok_new = token_of(result);
    p.wait_for(&format!("present:{tok_new}"));
    assert!(broker.resolve(&tok_new, UserChoice::Reject));
    p.wait_for(&format!("close:{tok_new}"));
}

#[test]
fn commit_runs_without_broker_lock() {
    let p = FakePrompter::new();
    let slot: Arc<OnceLock<Weak<PermissionBroker>>> = Arc::new(OnceLock::new());
    let token_cell: Arc<OnceLock<String>> = Arc::new(OnceLock::new());
    let (tx, rx) = mpsc::channel::<(bool, usize)>();
    let tx = Mutex::new(tx);
    let slot_cb = slot.clone();
    let token_cb = token_cell.clone();
    let commit: CommitFn = Arc::new(move |b: RequestBinding, _o: Outcome| {
        if b.run_token != 1 {
            return;
        }
        let Some(broker) = slot_cb.get().and_then(Weak::upgrade) else {
            return;
        };
        let Some(own) = token_cb.get() else {
            return;
        };
        // Both calls take the broker's locks; they must not deadlock inside a commit.
        let own_resolved = broker.resolve(own, UserChoice::Allow);
        let invalidated = broker.invalidate_run(2);
        let _ = tx.lock().unwrap().send((own_resolved, invalidated));
    });
    let broker = PermissionBroker::new(p.clone(), commit);
    assert!(slot.set(Arc::downgrade(&broker)).is_ok());

    let tok_a = token_of(broker.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    assert!(token_cell.set(tok_a.clone()).is_ok());
    p.wait_for(&format!("present:{tok_a}"));
    let tok_b = token_of(broker.submit(binding(1, 2, json!(2)), req(ToolKind::Edit), far()));
    assert!(broker.resolve(&tok_a, UserChoice::Allow));
    let (own_resolved, invalidated) = rx
        .recv_timeout(WAIT_CAP)
        .expect("commit callback never returned: broker deadlocked");
    assert!(
        !own_resolved,
        "resolve of a completed ticket inside commit must be false"
    );
    assert_eq!(
        invalidated, 1,
        "queued ticket B must be invalidated from inside commit"
    );
    std::thread::sleep(SETTLE);
    assert_eq!(p.count(&format!("present:{tok_b}")), 0);
    assert!(!broker.resolve(&tok_b, UserChoice::Allow));
}

#[test]
fn worker_restarts_after_idle() {
    let h = harness();
    let tok_a = token_of(h.b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_a}"));
    assert!(h.b.resolve(&tok_a, UserChoice::Allow));
    h.c.wait_len(1);
    std::thread::sleep(Duration::from_millis(100));
    let bind_b = binding(1, 2, json!(2));
    let tok_b = token_of(h.b.submit(bind_b.clone(), req(ToolKind::Edit), far()));
    h.p.wait_for(&format!("present:{tok_b}"));
    assert!(h.b.resolve(&tok_b, UserChoice::Reject));
    let commits = h.c.wait_len(2);
    assert_eq!(commits[1], (bind_b, Outcome::Selected("r1".to_string())));
}

#[test]
fn worker_survives_panicking_commit_callback() {
    let p = FakePrompter::new();
    let c = Arc::new(Commits::default());
    let c_cb = c.clone();
    let first = AtomicBool::new(true);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_cb = calls.clone();
    let commit: CommitFn = Arc::new(move |b: RequestBinding, o: Outcome| {
        calls_cb.fetch_add(1, Ordering::SeqCst);
        if first.swap(false, Ordering::SeqCst) {
            panic!("simulated commit failure");
        }
        c_cb.push(b, o);
    });
    let b = PermissionBroker::new(p.clone(), commit);

    let tok1 = token_of(b.submit(binding(1, 1, json!(1)), req(ToolKind::Edit), far()));
    p.wait_for(&format!("present:{tok1}"));
    assert!(b.resolve(&tok1, UserChoice::Allow));

    let bind2 = binding(1, 2, json!(2));
    let tok2 = token_of(b.submit(bind2.clone(), req(ToolKind::Edit), far()));
    p.wait_for(&format!("present:{tok2}"));
    assert!(b.resolve(&tok2, UserChoice::Allow));
    let commits = c.wait_len(1);
    assert_eq!(commits, vec![(bind2, Outcome::Selected("a1".to_string()))]);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "both commits were attempted"
    );
    p.wait_for(&format!("close:{tok2}"));
}

/// Panics on its first `present` call, then delegates to the wrapped fake.
struct PanicFirstPresent {
    inner: Arc<FakePrompter>,
    armed: AtomicBool,
}

impl PermissionPrompter for PanicFirstPresent {
    fn present(&self, view: &PermissionView) {
        if self.armed.swap(false, Ordering::SeqCst) {
            panic!("simulated prompter failure");
        }
        self.inner.present(view);
    }

    fn close(&self, token: &str) {
        self.inner.close(token);
    }
}

#[test]
fn worker_survives_panicking_present() {
    let p = FakePrompter::new();
    let c = Arc::new(Commits::default());
    let prompter = Arc::new(PanicFirstPresent {
        inner: p.clone(),
        armed: AtomicBool::new(true),
    });
    let b = PermissionBroker::new(prompter, commit_into(c.clone()));

    // A panicking present rejects the ticket (reject_once) and it is never shown.
    let bind1 = binding(1, 1, json!(1));
    let _tok1 = token_of(b.submit(bind1.clone(), req(ToolKind::Edit), far()));
    let commits = c.wait_len(1);
    assert_eq!(commits, vec![(bind1, Outcome::Selected("r1".to_string()))]);
    assert_eq!(p.count_prefix("close:"), 0);

    let bind2 = binding(1, 2, json!(2));
    let tok2 = token_of(b.submit(bind2.clone(), req(ToolKind::Edit), far()));
    p.wait_for(&format!("present:{tok2}"));
    assert!(b.resolve(&tok2, UserChoice::Allow));
    let commits = c.wait_len(2);
    assert_eq!(commits[1], (bind2, Outcome::Selected("a1".to_string())));
    assert_eq!(b.pending_len(), 0);
}
