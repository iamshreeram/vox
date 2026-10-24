//! Contract tests for the pre-start cancel latch (DESIGN-PR2-STAGE2 Revision 3,
//! section 2.2, C3). Red until the production step adds `BeginError::Cancelled`
//! and the bounded `precancelled` set with its `cancel_floor`.
//!
//! Boolean contract (ram-ai decision A1b): `RunRegistry::cancel` returns true ONLY
//! when a LIVE (begun) run was cancelled. A cancel before begin latches the token
//! silently and returns false; the latch is observed through the later begin, which
//! is refused with `BeginError::Cancelled` and consumes the token.
//!
//! Registry tests are pure (no IO). The agent-level tests use a real child
//! process (`/bin/sh`) that creates a marker file the moment it starts, so a
//! marker file is proof that the agent was spawned. Every wait is capped.
//! Workers must not edit this file.

use crate::managers::acp_run::{
    BeginError, ResponseSink, RunHandle, RunRegistry, SessionSet, SinkError,
};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

/// Pre-cancel cap from the design (2.2). Cancelling more than this many
/// distinct not-yet-begun handles evicts the oldest.
const PRECANCEL_CAP: usize = 64;

fn cancel_line(session_id: &str) -> String {
    format!("CANCEL {session_id}")
}

/// Records every line it is asked to send. Never refuses.
struct Sink {
    lines: Mutex<Vec<String>>,
}

impl ResponseSink for Sink {
    fn try_send_line(&self, line: String) -> Result<(), SinkError> {
        self.lines.lock().unwrap().push(line);
        Ok(())
    }
}

fn registry() -> (Arc<RunRegistry>, Arc<Sink>) {
    let reg = RunRegistry::new();
    let sink = Arc::new(Sink {
        lines: Mutex::new(Vec::new()),
    });
    reg.set_connection(1, sink.clone());
    (reg, sink)
}

fn sent(sink: &Sink) -> Vec<String> {
    sink.lines.lock().unwrap().clone()
}

/// Begins a fresh handle and returns it. Panics if it cannot begin.
fn active_run(reg: &Arc<RunRegistry>) -> RunHandle {
    let h = reg.new_handle();
    reg.begin(&h)
        .expect("begin should succeed with no active run");
    h
}

// ---- registry: cancel before begin (latch; cancel returns false) ----

#[test]
fn cancel_before_begin_latches_so_the_later_begin_is_refused() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert!(
        !reg.cancel(&h, &cancel_line),
        "a pre-begin latch is not a live cancel"
    );
    assert_eq!(reg.begin(&h), Err(BeginError::Cancelled));
}

#[test]
fn precancel_sends_nothing_because_no_session_exists_yet() {
    let (reg, sink) = registry();
    let h = reg.new_handle();
    assert!(!reg.cancel(&h, &cancel_line));
    let _ = reg.begin(&h);
    assert!(sent(&sink).is_empty());
}

#[test]
fn repeated_cancel_before_begin_never_uncancels() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert!(!reg.cancel(&h, &cancel_line));
    assert!(!reg.cancel(&h, &cancel_line));
    assert!(!reg.cancel(&h, &cancel_line));
    assert_eq!(reg.begin(&h), Err(BeginError::Cancelled));
    assert!(!reg.cancel(&h, &cancel_line));
}

#[test]
fn rejected_precancelled_begin_consumes_the_token() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert!(!reg.cancel(&h, &cancel_line));
    assert_eq!(reg.begin(&h), Err(BeginError::Cancelled));
    // The token is spent: a retry with the same handle is rejected too.
    let retry = reg.begin(&h);
    assert!(
        matches!(
            retry,
            Err(BeginError::Cancelled) | Err(BeginError::Consumed)
        ),
        "retry of a rejected handle must be rejected, got {retry:?}"
    );
}

#[test]
fn cancel_of_a_consumed_token_returns_false() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert!(!reg.cancel(&h, &cancel_line));
    assert_eq!(reg.begin(&h), Err(BeginError::Cancelled));
    assert!(!reg.cancel(&h, &cancel_line));
    assert!(reg.begin(&h).is_err());
}

#[test]
fn precancel_does_not_affect_a_later_fresh_run() {
    let (reg, _sink) = registry();
    let stale = reg.new_handle();
    assert!(!reg.cancel(&stale, &cancel_line));
    assert_eq!(reg.begin(&stale), Err(BeginError::Cancelled));
    let fresh = reg.new_handle();
    assert_eq!(reg.begin(&fresh), Ok(()));
    assert!(!reg.is_cancelled(&fresh));
    assert!(
        reg.cancel(&fresh, &cancel_line),
        "a live cancel returns true"
    );
}

#[test]
fn handle_cancelled_while_another_run_is_live_is_refused_after_it_finishes() {
    let (reg, _sink) = registry();
    let live = active_run(&reg);
    let queued = reg.new_handle();
    assert!(!reg.cancel(&queued, &cancel_line));
    // The live run is untouched by the queued handle's pre-cancel.
    assert!(!reg.is_cancelled(&live));
    reg.finish(&live);
    assert_eq!(reg.begin(&queued), Err(BeginError::Cancelled));
}

#[test]
fn busy_never_begun_handle_cancel_is_false_and_active_run_keeps_running() {
    let (reg, sink) = registry();
    let active = active_run(&reg);
    assert_eq!(reg.set_session(&active, "s1"), SessionSet::Ok);
    let busy = reg.new_handle();
    assert_eq!(reg.begin(&busy), Err(BeginError::Busy));

    assert!(
        !reg.cancel(&busy, &cancel_line),
        "a Busy handle holds no run"
    );
    assert!(
        !reg.is_cancelled(&active),
        "the active run is not cancelled"
    );
    assert!(
        sent(&sink).is_empty(),
        "no session/cancel was sent for the busy handle"
    );

    // The active run is still cancellable by its own handle.
    assert!(reg.cancel(&active, &cancel_line));
    assert_eq!(sent(&sink), vec!["CANCEL s1".to_string()]);
    reg.finish(&active);

    // The busy handle was latched, so it can never begin afterwards.
    assert_eq!(reg.begin(&busy), Err(BeginError::Cancelled));
}

// ---- registry: bounded pre-cancel set and cancel_floor ----

#[test]
fn sixty_four_precancels_all_remain_refused() {
    let (reg, _sink) = registry();
    let handles: Vec<RunHandle> = (0..PRECANCEL_CAP).map(|_| reg.new_handle()).collect();
    for h in &handles {
        assert!(!reg.cancel(h, &cancel_line));
    }
    // Exactly at the cap nothing has been evicted yet.
    for h in &handles {
        assert_eq!(reg.begin(h), Err(BeginError::Cancelled));
    }
}

#[test]
fn oldest_precancel_stays_refused_after_more_than_64_cancels() {
    let (reg, _sink) = registry();
    let oldest = reg.new_handle();
    assert!(!reg.cancel(&oldest, &cancel_line));
    let mut newer = Vec::new();
    for _ in 0..PRECANCEL_CAP {
        let h = reg.new_handle();
        assert!(!reg.cancel(&h, &cancel_line));
        newer.push(h);
    }
    // 65 distinct cancelled handles were created: the oldest was evicted, and
    // the cancel floor must keep it from ever becoming startable (fail closed).
    assert_eq!(reg.begin(&oldest), Err(BeginError::Cancelled));
    assert!(
        reg.begin(&oldest).is_err(),
        "the evicted handle must stay refused on the second attempt"
    );
}

#[test]
fn newest_precancel_still_refused_after_overflow() {
    let (reg, _sink) = registry();
    let oldest = reg.new_handle();
    assert!(!reg.cancel(&oldest, &cancel_line));
    let mut newest = oldest;
    for _ in 0..PRECANCEL_CAP {
        newest = reg.new_handle();
        assert!(!reg.cancel(&newest, &cancel_line));
    }
    assert!(reg.begin(&newest).is_err());
}

#[test]
fn fresh_uncancelled_handle_begins_after_overflow() {
    let (reg, _sink) = registry();
    let oldest = reg.new_handle();
    assert!(!reg.cancel(&oldest, &cancel_line));
    for _ in 0..PRECANCEL_CAP {
        let h = reg.new_handle();
        assert!(!reg.cancel(&h, &cancel_line));
    }
    assert!(reg.begin(&oldest).is_err());
    assert!(reg.begin(&oldest).is_err());
    let fresh = reg.new_handle();
    assert_eq!(reg.begin(&fresh), Ok(()));
    assert!(!reg.is_cancelled(&fresh));
}

// ---- registry: live runs and stale handles are unchanged ----

#[test]
fn cancel_of_a_live_run_still_sends_session_cancel_once() {
    let (reg, sink) = registry();
    let h = active_run(&reg);
    assert_eq!(reg.set_session(&h, "s1"), SessionSet::Ok);
    assert!(reg.cancel(&h, &cancel_line));
    assert!(!reg.cancel(&h, &cancel_line));
    assert_eq!(sent(&sink), vec!["CANCEL s1".to_string()]);
    assert!(reg.is_cancelled(&h));
}

#[test]
fn cancel_of_a_finished_run_is_false_and_leaves_a_later_run_alone() {
    let (reg, _sink) = registry();
    let old = active_run(&reg);
    reg.finish(&old);
    assert!(!reg.cancel(&old, &cancel_line));
    let later = active_run(&reg);
    assert!(!reg.is_cancelled(&later));
    assert!(reg.cancel(&later, &cancel_line));
}

#[test]
fn begin_then_cancel_in_sequence_cancels_the_live_run_once() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert_eq!(reg.begin(&h), Ok(()));
    assert!(reg.cancel(&h, &cancel_line));
    assert!(reg.is_cancelled(&h));
    assert!(!reg.cancel(&h, &cancel_line));
}

// ---- registry: cancel racing begin ----

#[test]
fn cancel_then_begin_in_sequence_refuses_the_begin() {
    let (reg, _sink) = registry();
    let h = reg.new_handle();
    assert!(!reg.cancel(&h, &cancel_line));
    assert_eq!(reg.begin(&h), Err(BeginError::Cancelled));
}

#[test]
fn cancel_racing_begin_always_ends_cancelled_in_both_orders() {
    // Both threads are released together. Whichever order the scheduler picks,
    // the handle must end cancelled. Cancel returns true exactly when it hit a
    // live run, which is exactly when the begin succeeded.
    for _ in 0..200 {
        let (reg, _sink) = registry();
        let h = reg.new_handle();
        let gate = Arc::new(Barrier::new(2));

        let cancel_side = {
            let reg = reg.clone();
            let gate = gate.clone();
            thread::spawn(move || {
                gate.wait();
                reg.cancel(&h, &cancel_line)
            })
        };
        let begin_side = {
            let reg = reg.clone();
            let gate = gate.clone();
            thread::spawn(move || {
                gate.wait();
                reg.begin(&h)
            })
        };

        let cancelled_live = cancel_side.join().expect("cancel thread panicked");
        let began = begin_side.join().expect("begin thread panicked");
        match began {
            Ok(()) => {
                assert!(
                    cancelled_live,
                    "a run that began must have been cancelled live"
                );
                assert!(reg.is_cancelled(&h));
            }
            Err(e) => {
                assert_eq!(e, BeginError::Cancelled);
                assert!(!cancelled_live, "a latched cancel must return false");
                assert!(reg.begin(&h).is_err(), "a refused handle must stay refused");
            }
        }
    }
}

// ---- agent level: prompt_with_handle on a pre-cancelled handle ----

#[cfg(unix)]
mod agent_level {
    use crate::managers::acp_agent::{
        AcpAgentConfig, AcpAgentError, AcpAgentManager, PromptOptions,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

    /// Settle window: a spawn would create the marker well inside this time.
    const SETTLE: Duration = Duration::from_millis(500);

    fn marker_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vox-acp-latch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create marker dir");
        dir
    }

    /// The child touches `marker` as soon as it starts, then sleeps without
    /// ever answering. A marker file therefore proves a spawn happened.
    fn marker_cfg(marker: &Path) -> AcpAgentConfig {
        AcpAgentConfig {
            command: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_string(),
                format!("touch '{}'; exec sleep 30", marker.display()),
            ],
            cwd: std::env::temp_dir(),
            turn_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_millis(800),
            cancel_grace: Duration::from_millis(200),
        }
    }

    fn opts() -> PromptOptions {
        PromptOptions {
            reuse_session: true,
        }
    }

    fn wait_for_file(path: &Path, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    #[test]
    fn precancelled_handle_returns_cancelled_without_spawning_the_agent() {
        let dir = marker_dir("precancel");
        let marker = dir.join("spawned");
        let m = AcpAgentManager::new();
        let h = m.new_run_handle();
        assert!(
            !m.cancel(&h),
            "a pre-begin latch returns false, not a live cancel"
        );
        assert!(!m.cancel(&h), "a second cancel must report false");

        let out = m.prompt_with_handle(&marker_cfg(&marker), "hello", opts(), &h);

        assert!(matches!(out, Err(AcpAgentError::Cancelled)), "{out:?}");
        thread::sleep(SETTLE);
        assert!(
            !marker.exists(),
            "a pre-cancelled run must never spawn or connect to the agent"
        );
    }

    #[test]
    fn reusing_a_precancelled_handle_never_spawns_either() {
        let dir = marker_dir("reuse");
        let marker = dir.join("spawned");
        let m = AcpAgentManager::new();
        let h = m.new_run_handle();
        assert!(!m.cancel(&h));

        let first = m.prompt_with_handle(&marker_cfg(&marker), "one", opts(), &h);
        assert!(matches!(first, Err(AcpAgentError::Cancelled)), "{first:?}");
        let second = m.prompt_with_handle(&marker_cfg(&marker), "two", opts(), &h);
        assert!(
            matches!(
                second,
                Err(AcpAgentError::Cancelled) | Err(AcpAgentError::Busy)
            ),
            "a spent handle must be refused, got {second:?}"
        );
        thread::sleep(SETTLE);
        assert!(!marker.exists(), "neither attempt may spawn the agent");
    }

    #[test]
    fn control_fresh_handle_does_spawn_the_agent() {
        // Control: proves the marker mechanism is real. Without it the negative
        // assertions above could pass for the wrong reason.
        let dir = marker_dir("control");
        let marker = dir.join("spawned");
        let m = AcpAgentManager::new();
        let h = m.new_run_handle();

        let out = m.prompt_with_handle(&marker_cfg(&marker), "hello", opts(), &h);

        assert!(
            !matches!(out, Err(AcpAgentError::Cancelled)),
            "an uncancelled handle must not report Cancelled: {out:?}"
        );
        assert!(
            wait_for_file(&marker, 5),
            "an uncancelled handle must spawn the agent (marker missing)"
        );
    }
}
