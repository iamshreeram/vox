//! Tests for the poller stop handle, the registry and the status guard.

use super::*;
use crate::managers::source_update_runner::{RunPhase, StatusMethod};
use std::sync::atomic::AtomicUsize;
use std::thread;

const RUN: &str = "0123456789abcdef";
const FULL: &str = "0123456789abcdef0123456789abcdef01234567";
const SHORT: Duration = Duration::from_millis(50);

fn status() -> RunStatus {
    RunStatus {
        run_id: RUN.to_string(),
        phase: RunPhase::Waiting,
        percent: 91,
        target: "0123456".to_string(),
        target_full: FULL.to_string(),
        message: String::new(),
        method: StatusMethod::Exact,
        ts: 1,
    }
}

/// A task that ticks like the poller: checks the flag before each tick.
fn spawn_ticking(ticks: Arc<AtomicUsize>) -> PollerHandle {
    let (handle, life) = lifecycle();
    thread::spawn(move || {
        while should_continue(&life.stop) {
            ticks.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(1));
        }
    });
    handle
}

/// A task whose tick never returns in time, so it cannot honour the stop.
fn spawn_stuck(hold: Duration) -> PollerHandle {
    let (handle, life) = lifecycle();
    thread::spawn(move || {
        let _life = life;
        thread::sleep(hold);
    });
    handle
}

#[test]
fn stop_flag_is_honoured_and_join_is_ok_after_a_stopped_loop() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let handle = spawn_ticking(Arc::clone(&ticks));
    thread::sleep(Duration::from_millis(20));
    assert!(handle.wait_stopped(Duration::from_secs(2)).is_ok());
    let after_stop = ticks.load(Ordering::SeqCst);
    thread::sleep(SHORT);
    assert_eq!(
        ticks.load(Ordering::SeqCst),
        after_stop,
        "no tick after stop"
    );
}

#[test]
fn stop_and_join_is_err_on_a_stuck_loop() {
    let handle = spawn_stuck(Duration::from_millis(400));
    assert_eq!(
        handle.wait_stopped(Duration::from_millis(50)),
        Err(PollerStopError)
    );
}

#[test]
fn request_stop_alone_sets_the_flag_without_waiting() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let handle = spawn_ticking(Arc::clone(&ticks));
    handle.request_stop();
    assert!(handle.stop_and_join().is_ok());
}

#[test]
fn no_status_is_written_after_stop() {
    let stop = AtomicBool::new(true);
    let writes = AtomicUsize::new(0);
    let path = PathBuf::from("/nonexistent/status.json");
    let written = write_status_unless_stopped(&stop, &path, &status(), |_, _| {
        writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect("no error");
    assert!(!written);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn status_is_written_while_running() {
    let stop = AtomicBool::new(false);
    let writes = AtomicUsize::new(0);
    let path = PathBuf::from("/nonexistent/status.json");
    let written = write_status_unless_stopped(&stop, &path, &status(), |_, _| {
        writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    })
    .expect("no error");
    assert!(written);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[test]
fn registry_stops_the_registered_poller_and_forgets_it() {
    let registry = PollerRegistry::new();
    assert_eq!(registry.stop_and_join(), Ok(()), "empty registry is ok");
    let ticks = Arc::new(AtomicUsize::new(0));
    registry.replace(RUN, spawn_ticking(ticks));
    assert_eq!(registry.registered_run().as_deref(), Some(RUN));
    assert_eq!(
        registry.stop_and_join_within(Duration::from_secs(2)),
        Ok(())
    );
    assert_eq!(registry.registered_run(), None);
}

#[test]
fn registry_keeps_a_stuck_poller_registered_on_error() {
    let registry = PollerRegistry::new();
    registry.replace(RUN, spawn_stuck(Duration::from_millis(300)));
    assert_eq!(
        registry.stop_and_join_within(Duration::from_millis(30)),
        Err(PollerStopError)
    );
    assert_eq!(registry.registered_run().as_deref(), Some(RUN));
    assert_eq!(
        registry.stop_and_join_within(Duration::from_secs(3)),
        Ok(())
    );
}

#[test]
fn replacing_a_registered_poller_asks_the_old_one_to_stop() {
    let registry = PollerRegistry::new();
    let ticks = Arc::new(AtomicUsize::new(0));
    let old = spawn_ticking(Arc::clone(&ticks));
    registry.replace(RUN, old);
    let other = "fedcba9876543210";
    registry.replace(other, spawn_stuck(Duration::ZERO));
    assert_eq!(registry.registered_run().as_deref(), Some(other));
    thread::sleep(SHORT);
    let after = ticks.load(Ordering::SeqCst);
    thread::sleep(SHORT);
    assert_eq!(ticks.load(Ordering::SeqCst), after, "old poller stopped");
}
