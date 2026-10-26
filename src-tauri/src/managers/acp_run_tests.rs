//! Contract tests for `acp_run` (DESIGN.md Revision 3: S3 "Turn timing +
//! cancel", C2/C4 run registry and commit linearization, C12 delivery
//! failure). Red until step S3 implements the module. Workers must not edit
//! this file.
//!
//! Pure tests: no IO, no Tauri. Timing tests build instants as `base + Duration`
//! (no sleeps). Barrier tests pause a commit just before it takes the registry
//! lock via `commit_with_hook`; every wait is capped (2 s) so a deadlock fails
//! the test instead of hanging the run.

use crate::managers::acp_run::{
    BeginError, CommitResult, ResponseSink, RunHandle, RunRegistry, SessionSet, SinkError,
    TurnTiming,
};
use std::collections::HashSet;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(2);

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}

// ---- TurnTiming (pure; all time is passed in) ----

#[test]
fn hard_deadline_is_start_plus_three_turn_timeouts() {
    let start = Instant::now();
    let t = TurnTiming::new(start, secs(2));
    assert_eq!(t.hard_deadline(), start + secs(6));
}

#[test]
fn no_approvals_effective_deadline_is_start_plus_turn_timeout() {
    let start = Instant::now();
    let t = TurnTiming::new(start, secs(2));
    assert_eq!(t.pending(), 0);
    assert_eq!(t.effective_deadline(start), start + secs(2));
    assert_eq!(t.effective_deadline(at(start, 1000)), start + secs(2));
}

#[test]
fn approval_pause_extends_deadline_by_waited_time() {
    let start = Instant::now();
    let mut t = TurnTiming::new(start, secs(2));
    t.enter_approval(at(start, 500));
    t.exit_approval(at(start, 1500));
    assert_eq!(t.pending(), 0);
    assert_eq!(t.effective_deadline(at(start, 1500)), start + secs(3));
}

#[test]
fn overlapping_approvals_count_the_union_not_the_sum() {
    let start = Instant::now();
    let mut t = TurnTiming::new(start, secs(2));
    t.enter_approval(at(start, 0));
    t.enter_approval(at(start, 500));
    t.exit_approval(at(start, 1000));
    assert_eq!(t.pending(), 1);
    t.exit_approval(at(start, 1500));
    assert_eq!(t.pending(), 0);
    // Union of [0, 1.5 s] is 1.5 s, not the 2.0 s sum of the two waits.
    assert_eq!(t.effective_deadline(at(start, 1500)), at(start, 3500));
}

#[test]
fn pending_approval_deadline_moves_with_now() {
    let start = Instant::now();
    let mut t = TurnTiming::new(start, secs(2));
    t.enter_approval(at(start, 1000));
    assert_eq!(t.pending(), 1);
    // While waiting, the deadline is turn_timeout + paused so far, and it moves with now.
    assert_eq!(t.effective_deadline(at(start, 1500)), at(start, 2500));
    assert_eq!(t.effective_deadline(at(start, 2000)), at(start, 3000));
    assert!(!t.expired(at(start, 2499)));
}

#[test]
fn extension_is_capped_by_hard_deadline() {
    let start = Instant::now();
    let mut t = TurnTiming::new(start, secs(2));
    t.enter_approval(at(start, 500));
    t.exit_approval(at(start, 5500));
    assert_eq!(t.effective_deadline(at(start, 5500)), start + secs(6));
    assert_eq!(t.hard_deadline(), start + secs(6));
    assert!(t.expired(start + secs(6)));

    let mut open = TurnTiming::new(start, secs(2));
    open.enter_approval(at(start, 500));
    assert_eq!(open.effective_deadline(at(start, 10_000)), start + secs(6));
}

#[test]
fn approval_budget_is_min_of_configured_and_remaining_hard_budget() {
    let start = Instant::now();
    let t = TurnTiming::new(start, secs(10));
    assert_eq!(t.approval_budget(start, secs(60)), secs(30));
    assert_eq!(t.approval_budget(start + secs(25), secs(60)), secs(5));
}

#[test]
fn approval_budget_zero_when_no_budget_left() {
    let start = Instant::now();
    let t = TurnTiming::new(start, secs(2)); // hard deadline = start + 6 s
    assert_eq!(t.approval_budget(start + secs(6), secs(60)), Duration::ZERO);
    assert_eq!(t.approval_budget(start + secs(7), secs(60)), Duration::ZERO);
}

#[test]
fn expired_exactly_at_effective_deadline() {
    let start = Instant::now();
    let t = TurnTiming::new(start, secs(2));
    assert!(t.expired(start + secs(2)));
    assert!(!t.expired(start + secs(2) - Duration::from_nanos(1)));
}

#[test]
fn exit_without_enter_is_noop() {
    let start = Instant::now();
    let mut t = TurnTiming::new(start, secs(2));
    t.exit_approval(at(start, 1000));
    assert_eq!(t.pending(), 0);
    assert_eq!(t.effective_deadline(at(start, 1000)), start + secs(2));
    // A later normal pause must still be counted exactly (no underflow residue).
    t.enter_approval(at(start, 1000));
    t.exit_approval(at(start, 1500));
    assert_eq!(t.effective_deadline(at(start, 1500)), at(start, 2500));
}

// ---- RunRegistry (one active run; all transitions under one lock) ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ok,
    Full,
    Disconnected,
}

/// Records lines sent while `Mode::Ok`; `Full`/`Disconnected` refuse every line.
struct FakeSink {
    mode: Mode,
    lines: Mutex<Vec<String>>,
}

impl FakeSink {
    fn new(mode: Mode) -> Arc<FakeSink> {
        Arc::new(FakeSink {
            mode,
            lines: Mutex::new(Vec::new()),
        })
    }

    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

impl ResponseSink for FakeSink {
    fn try_send_line(&self, line: String) -> Result<(), SinkError> {
        match self.mode {
            Mode::Ok => {
                self.lines.lock().unwrap().push(line);
                Ok(())
            }
            Mode::Full => Err(SinkError::Full),
            Mode::Disconnected => Err(SinkError::Disconnected),
        }
    }
}

fn cancel_fn(sid: &str) -> String {
    format!("CANCEL {sid}")
}

fn registry(mode: Mode) -> (Arc<RunRegistry>, Arc<FakeSink>) {
    let reg = RunRegistry::new();
    let sink = FakeSink::new(mode);
    reg.set_connection(1, sink.clone());
    (reg, sink)
}

fn active_run(reg: &Arc<RunRegistry>) -> RunHandle {
    let h = reg.new_handle();
    reg.begin(&h)
        .expect("begin should succeed with no active run");
    h
}

/// Active run whose session is known as "s1".
fn bound_run(reg: &Arc<RunRegistry>) -> RunHandle {
    let h = active_run(reg);
    assert_eq!(reg.set_session(&h, "s1"), SessionSet::Ok);
    h
}

#[test]
fn begin_busy_when_other_run_active() {
    let (reg, _sink) = registry(Mode::Ok);
    let h1 = reg.new_handle();
    let h2 = reg.new_handle();
    assert_eq!(reg.begin(&h1), Ok(()));
    assert_eq!(reg.begin(&h2), Err(BeginError::Busy));
    assert_eq!(reg.set_session(&h1, "s1"), SessionSet::Ok);
    reg.finish(&h1);
    // The Busy handle never became active, so it can begin once the slot is free.
    assert_eq!(reg.begin(&h2), Ok(()));
}

#[test]
fn busy_handle_cannot_cancel_active_run() {
    let (reg, sink) = registry(Mode::Ok);
    let active = bound_run(&reg);
    let busy = reg.new_handle();
    assert_eq!(reg.begin(&busy), Err(BeginError::Busy));
    assert!(!reg.cancel(&busy, &cancel_fn));
    assert!(sink.lines().is_empty());
    assert!(!reg.is_cancelled(&active));
}

#[test]
fn cancel_sends_session_cancel_when_session_known() {
    let (reg, sink) = registry(Mode::Ok);
    let h = active_run(&reg);
    assert_eq!(reg.set_session(&h, "sess-1"), SessionSet::Ok);
    assert!(reg.cancel(&h, &cancel_fn));
    assert_eq!(sink.lines(), vec!["CANCEL sess-1".to_string()]);
    assert!(reg.is_cancelled(&h));
}

#[test]
fn cancel_before_session_known_latches_and_sends_nothing() {
    let (reg, sink) = registry(Mode::Ok);
    let h = active_run(&reg);
    assert!(reg.cancel(&h, &cancel_fn));
    assert!(sink.lines().is_empty());
    assert!(reg.is_cancelled(&h));
}

#[test]
fn set_session_after_latch_returns_cancelled() {
    let (reg, _sink) = registry(Mode::Ok);
    let h = active_run(&reg);
    assert!(reg.cancel(&h, &cancel_fn));
    assert_eq!(reg.set_session(&h, "late"), SessionSet::Cancelled);
    assert!(reg.is_cancelled(&h));
}

#[test]
fn cancel_twice_second_returns_false_and_sends_one_line() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert!(reg.cancel(&h, &cancel_fn));
    assert!(!reg.cancel(&h, &cancel_fn));
    assert_eq!(sink.lines(), vec!["CANCEL s1".to_string()]);
}

#[test]
fn stale_handle_after_finish_cannot_cancel() {
    let (reg, sink) = registry(Mode::Ok);
    let old = bound_run(&reg);
    reg.finish(&old);
    assert!(!reg.cancel(&old, &cancel_fn));
    assert!(sink.lines().is_empty());
    let fresh = reg.new_handle();
    assert_eq!(reg.begin(&fresh), Ok(()));
    assert!(!reg.is_cancelled(&fresh));
}

#[test]
fn stale_handle_after_new_run_began_cannot_cancel() {
    let (reg, sink) = registry(Mode::Ok);
    let old = bound_run(&reg);
    reg.finish(&old);
    let new = active_run(&reg);
    assert_eq!(reg.set_session(&new, "s2"), SessionSet::Ok);
    assert!(!reg.cancel(&old, &cancel_fn));
    assert!(sink.lines().is_empty());
    assert!(!reg.is_cancelled(&new));
    // The new run is unaffected and can still cancel itself.
    assert!(reg.cancel(&new, &cancel_fn));
    assert_eq!(sink.lines(), vec!["CANCEL s2".to_string()]);
}

#[test]
fn commit_delivers_for_valid_binding() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::Delivered
    );
    assert_eq!(sink.lines(), vec!["RESP".to_string()]);
    assert!(!reg.delivery_failed(&h));
}

#[test]
fn commit_dropped_for_wrong_conn_gen() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert_eq!(
        reg.commit(2, h.token(), "s1", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_dropped_for_wrong_run_token() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    let wrong = h.token().wrapping_add(1);
    assert_eq!(
        reg.commit(1, wrong, "s1", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_dropped_for_wrong_session() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert_eq!(
        reg.commit(1, h.token(), "other", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_dropped_after_finish() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    reg.finish(&h);
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_dropped_after_cancel() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert!(reg.cancel(&h, &cancel_fn));
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert_eq!(sink.lines(), vec!["CANCEL s1".to_string()]);
}

#[test]
fn commit_dropped_after_clear_connection() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    reg.clear_connection(1);
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::Dropped
    );
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_full_sink_returns_delivery_failed_and_sets_flag() {
    let (reg, sink) = registry(Mode::Full);
    let h = bound_run(&reg);
    assert!(!reg.delivery_failed(&h));
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::DeliveryFailed
    );
    assert!(reg.delivery_failed(&h));
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_disconnected_sink_returns_delivery_failed() {
    let (reg, sink) = registry(Mode::Disconnected);
    let h = bound_run(&reg);
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::DeliveryFailed
    );
    assert!(reg.delivery_failed(&h));
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_does_not_block_when_sink_is_full() {
    let (reg, _sink) = registry(Mode::Full);
    let h = bound_run(&reg);
    let started = Instant::now();
    let result = reg.commit(1, h.token(), "s1", "RESP".to_string());
    let elapsed = started.elapsed();
    assert_eq!(result, CommitResult::DeliveryFailed);
    assert!(
        elapsed < Duration::from_millis(200),
        "commit blocked for {elapsed:?}"
    );
}

/// A commit paused just before it takes the registry lock (via `commit_with_hook`).
struct Paused {
    paused: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    worker: thread::JoinHandle<CommitResult>,
}

impl Paused {
    fn await_pause(&self) {
        self.paused
            .recv_timeout(WAIT)
            .expect("commit never reached the pre-lock hook");
    }

    fn release(self) -> CommitResult {
        self.release
            .send(())
            .expect("commit thread gone before release");
        self.worker.join().expect("commit thread panicked")
    }
}

/// Commits on conn_gen 1 / session "s1" with the given token, paused pre-lock.
fn spawn_paused_commit(reg: Arc<RunRegistry>, token: u64, line: &'static str) -> Paused {
    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let worker = thread::spawn(move || {
        let hook = move || {
            paused_tx.send(()).expect("test side gone");
            release_rx
                .recv_timeout(WAIT)
                .expect("test never released the commit");
        };
        reg.commit_with_hook(1, token, "s1", line.to_string(), &hook)
    });
    Paused {
        paused: paused_rx,
        release: release_tx,
        worker,
    }
}

#[test]
fn commit_paused_then_cancelled_is_dropped() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    let paused = spawn_paused_commit(reg.clone(), h.token(), "RESP");
    paused.await_pause();
    assert!(reg.cancel(&h, &cancel_fn));
    assert_eq!(paused.release(), CommitResult::Dropped);
    // Only the session/cancel line reaches the agent; no selected response.
    assert_eq!(sink.lines(), vec!["CANCEL s1".to_string()]);
}

#[test]
fn commit_paused_then_finished_is_dropped() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    let paused = spawn_paused_commit(reg.clone(), h.token(), "RESP");
    paused.await_pause();
    reg.finish(&h);
    assert_eq!(paused.release(), CommitResult::Dropped);
    assert!(sink.lines().is_empty());
}

#[test]
fn commit_paused_then_cancel_finish_and_new_run_is_dropped() {
    let (reg, sink) = registry(Mode::Ok);
    let old = bound_run(&reg);
    let paused = spawn_paused_commit(reg.clone(), old.token(), "OLD-RESP");
    paused.await_pause();
    assert!(reg.cancel(&old, &cancel_fn));
    reg.finish(&old);
    let new = active_run(&reg);
    assert_eq!(reg.set_session(&new, "s2"), SessionSet::Ok);
    // The old binding must be dropped even though its conn_gen is still current.
    assert_eq!(paused.release(), CommitResult::Dropped);
    // The new run's own commit still delivers.
    assert_eq!(
        reg.commit(1, new.token(), "s2", "NEW-RESP".to_string()),
        CommitResult::Delivered
    );
    assert_eq!(
        sink.lines(),
        vec!["CANCEL s1".to_string(), "NEW-RESP".to_string()]
    );
}

#[test]
fn commit_before_cancel_is_delivered_then_cancel_follows() {
    let (reg, sink) = registry(Mode::Ok);
    let h = bound_run(&reg);
    assert_eq!(
        reg.commit(1, h.token(), "s1", "RESP".to_string()),
        CommitResult::Delivered
    );
    assert!(reg.cancel(&h, &cancel_fn));
    assert_eq!(
        sink.lines(),
        vec!["RESP".to_string(), "CANCEL s1".to_string()]
    );
}

#[test]
fn flood_guard_allows_32_then_denies_and_resets_per_run() {
    let (reg, _sink) = registry(Mode::Ok);
    let first = active_run(&reg);
    for i in 0..32 {
        assert!(
            reg.note_permission_request(&first),
            "request {} of run 1 must be allowed",
            i + 1
        );
    }
    assert!(
        !reg.note_permission_request(&first),
        "33rd request must be denied"
    );
    assert!(
        !reg.note_permission_request(&first),
        "34th request must stay denied"
    );
    reg.finish(&first);

    let second = active_run(&reg);
    for i in 0..32 {
        assert!(
            reg.note_permission_request(&second),
            "request {} of run 2 must be allowed (counter resets per run)",
            i + 1
        );
    }
    assert!(!reg.note_permission_request(&second));
}

#[test]
fn note_permission_request_false_without_active_run() {
    let (reg, _sink) = registry(Mode::Ok);
    let idle = reg.new_handle();
    assert!(!reg.note_permission_request(&idle));
    let done = active_run(&reg);
    reg.finish(&done);
    assert!(!reg.note_permission_request(&done));
}

#[test]
fn tokens_are_unique() {
    let reg = RunRegistry::new();
    let tokens: HashSet<u64> = (0..1000).map(|_| reg.new_handle().token()).collect();
    assert_eq!(tokens.len(), 1000);
}
