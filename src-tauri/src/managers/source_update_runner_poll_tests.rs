//! Table tests for `source_update_runner_poll`. Pure: no files, no launchd.
//! Wired from the poll module with `#[path]`.

use super::*;
use crate::managers::source_update_runner::{JobProbe, LockState, RunPhase, StatusMethod};

const RUN: &str = "0123456789abcdef";
const FULL: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_FULL: &str = "fedcba9876543210fedcba9876543210fedcba98";
const NOW: u64 = 1_800_000_000;

fn status_bytes(phase: &str, pct: u32, message: &str) -> Vec<u8> {
    format!(
        r#"{{"v":1,"run_id":"{RUN}","phase":"{phase}","percent":{pct},"target":"0123456","target_full":"{FULL}","message":"{message}","method":"exact","ts":{NOW}}}"#
    )
    .into_bytes()
}

fn live() -> JobProbe {
    JobProbe::Registered {
        pid: Some(4242),
        last_exit: None,
    }
}

fn dead() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: None,
    }
}

/// One tick with the given observations. `age` is the status age in seconds.
fn tick(
    state: &mut PollState,
    bytes: Option<&[u8]>,
    job: JobProbe,
    lock: LockState,
    running: &str,
    age: u64,
    now_ms: u64,
) -> Vec<Effect> {
    poll_step(
        state,
        &PollInput {
            status_bytes: bytes,
            status_age_secs: age,
            job,
            lock,
            running_build_sha: running,
            pending_built: false,
            now_unix: NOW,
            now_ms,
        },
    )
}

fn has(effects: &[Effect], wanted: &Effect) -> bool {
    effects.contains(wanted)
}

fn progress_of(effects: &[Effect]) -> Vec<&UpdateProgress> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::PublishProgress(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn count_tray(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, Effect::RefreshTray))
        .count()
}

// ---- progress --------------------------------------------------------------

#[test]
fn progress_publishes_only_on_change() {
    let mut state = PollState::new(RUN);
    let first = tick(
        &mut state,
        Some(&status_bytes("compiling", 10, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    assert_eq!(progress_of(&first).len(), 1);
    assert_eq!(progress_of(&first)[0].percent, 10);
    assert_eq!(progress_of(&first)[0].phase, RunPhase::Compiling);

    let same = tick(
        &mut state,
        Some(&status_bytes("compiling", 10, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        500,
    );
    assert!(
        same.is_empty(),
        "unchanged status must emit nothing: {same:?}"
    );

    let next = tick(
        &mut state,
        Some(&status_bytes("compiling", 11, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        1000,
    );
    assert_eq!(progress_of(&next).len(), 1);
    assert_eq!(progress_of(&next)[0].percent, 11);
}

#[test]
fn progress_sequence_is_published_in_order() {
    let mut state = PollState::new(RUN);
    let mut seen = Vec::new();
    for (pct, now) in [
        (0_u32, 0_u64),
        (4, 500),
        (10, 1000),
        (42, 9000),
        (88, 12_000),
    ] {
        let effects = tick(
            &mut state,
            Some(&status_bytes("compiling", pct, "building")),
            live(),
            LockState::HeldAlive,
            FULL,
            0,
            now,
        );
        seen.extend(progress_of(&effects).iter().map(|p| p.percent));
    }
    assert_eq!(seen, vec![0, 4, 10, 42, 88]);
}

#[test]
fn regressing_percent_is_ignored_and_previous_progress_kept() {
    let mut state = PollState::new(RUN);
    tick(
        &mut state,
        Some(&status_bytes("compiling", 50, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    let effects = tick(
        &mut state,
        Some(&status_bytes("compiling", 20, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        1000,
    );
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(state.published_progress().map(|p| p.percent), Some(50));
    assert_eq!(state.invalid_reads(), 1);
}

#[test]
fn invalid_status_bytes_are_ignored_and_counted() {
    let mut state = PollState::new(RUN);
    tick(
        &mut state,
        Some(&status_bytes("compiling", 30, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    let effects = tick(
        &mut state,
        Some(b"{ not json"),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        1000,
    );
    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(state.invalid_reads(), 1);
    assert_eq!(state.published_progress().map(|p| p.percent), Some(30));

    let recovered = tick(
        &mut state,
        Some(&status_bytes("compiling", 31, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        1500,
    );
    assert_eq!(state.invalid_reads(), 0);
    assert_eq!(progress_of(&recovered)[0].percent, 31);
}

#[test]
fn status_for_another_run_is_invalid() {
    let mut state = PollState::new(RUN);
    let foreign = format!(
        r#"{{"v":1,"run_id":"fedcba9876543210","phase":"compiling","percent":5,"target":"0123456","target_full":"{FULL}","message":"x","method":"exact","ts":{NOW}}}"#
    );
    let effects = tick(
        &mut state,
        Some(foreign.as_bytes()),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    assert!(effects.is_empty());
    assert_eq!(state.invalid_reads(), 1);
}

// ---- tray coalescing -------------------------------------------------------

#[test]
fn first_progress_refreshes_the_tray_and_publish_precedes_it() {
    let mut state = PollState::new(RUN);
    let effects = tick(
        &mut state,
        Some(&status_bytes("preparing", 0, "start")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    assert_eq!(
        effects,
        vec![
            Effect::PublishProgress(UpdateProgress {
                run_id: RUN.to_string(),
                phase: RunPhase::Preparing,
                percent: 0,
                target: "0123456".to_string(),
                message: "start".to_string(),
                estimated: false,
            }),
            Effect::RefreshTray,
        ]
    );
}

#[test]
fn small_percent_change_in_same_phase_does_not_refresh_the_tray() {
    let mut state = PollState::new(RUN);
    tick(
        &mut state,
        Some(&status_bytes("compiling", 20, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    let effects = tick(
        &mut state,
        Some(&status_bytes("compiling", 23, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        5_000,
    );
    assert_eq!(count_tray(&effects), 0, "{effects:?}");
    assert_eq!(progress_of(&effects).len(), 1);
}

#[test]
fn five_point_change_refreshes_after_the_two_second_gap() {
    let mut state = PollState::new(RUN);
    tick(
        &mut state,
        Some(&status_bytes("compiling", 20, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    // Too soon after the last refresh: deferred.
    let early = tick(
        &mut state,
        Some(&status_bytes("compiling", 26, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        500,
    );
    assert_eq!(count_tray(&early), 0);
    // A later tick with nothing new pays the deferred refresh.
    let later = tick(
        &mut state,
        Some(&status_bytes("compiling", 26, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        2_000,
    );
    assert_eq!(count_tray(&later), 1, "{later:?}");
    // And it is not repeated.
    let again = tick(
        &mut state,
        Some(&status_bytes("compiling", 26, "b")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        2_500,
    );
    assert_eq!(count_tray(&again), 0);
}

#[test]
fn phase_change_is_a_tray_refresh_but_never_more_than_one_per_two_seconds() {
    let mut state = PollState::new(RUN);
    let mut refreshes = 0;
    // Ten changes spaced 100 ms apart: alternate phase so every one is a change.
    for step in 0..10_u64 {
        let phase = if step % 2 == 0 {
            "compiling"
        } else {
            "bundling"
        };
        let pct = 10 + step as u32;
        let effects = tick(
            &mut state,
            Some(&status_bytes(phase, pct, "b")),
            live(),
            LockState::HeldAlive,
            FULL,
            0,
            step * 100,
        );
        refreshes += count_tray(&effects);
    }
    // Only the first refresh is immediate; the rest wait for the 2 s window.
    assert_eq!(refreshes, 1);
}

// ---- terminal outcomes -----------------------------------------------------

#[test]
fn done_with_no_process_publishes_done_then_cleans_up_and_stops() {
    let mut state = PollState::new(RUN);
    let effects = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        dead(),
        LockState::Free,
        FULL,
        0,
        0,
    );
    let done_at = effects
        .iter()
        .position(|e| matches!(e, Effect::PublishDone(p) if p.percent == 100))
        .expect("PublishDone");
    let cleanup_at = effects
        .iter()
        .position(|e| *e == Effect::Cleanup)
        .expect("Cleanup");
    assert!(done_at < cleanup_at);
    assert_eq!(effects.last(), Some(&Effect::Stop));
}

#[test]
fn live_job_with_terminal_status_publishes_but_is_never_cleaned_up() {
    let mut state = PollState::new(RUN);
    let effects = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::PublishDone(_))));
    assert!(!has(&effects, &Effect::Cleanup));
    assert!(!has(&effects, &Effect::Stop));
    // Still alive a while later: nothing destructive, nothing republished.
    let later = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        9_000,
    );
    assert!(later.is_empty(), "{later:?}");
}

#[test]
fn cleanup_waits_for_the_process_to_exit_after_a_live_terminal_status() {
    let mut state = PollState::new(RUN);
    tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        live(),
        LockState::HeldAlive,
        FULL,
        0,
        0,
    );
    let while_held = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        dead(),
        LockState::HeldAlive,
        FULL,
        0,
        500,
    );
    assert!(!has(&while_held, &Effect::Cleanup), "lock still held");
    let released = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        dead(),
        LockState::Free,
        FULL,
        0,
        1000,
    );
    assert!(has(&released, &Effect::Cleanup));
    assert!(has(&released, &Effect::Stop));
}

#[test]
fn failed_status_publishes_its_message() {
    let mut state = PollState::new(RUN);
    let effects = tick(
        &mut state,
        Some(&status_bytes("failed", 37, "bun install failed")),
        dead(),
        LockState::Free,
        FULL,
        0,
        0,
    );
    let failed = effects
        .iter()
        .find_map(|e| match e {
            Effect::PublishFailed(p) => Some(p),
            _ => None,
        })
        .expect("PublishFailed");
    assert_eq!(failed.phase, RunPhase::Failed);
    assert_eq!(failed.message, "bun install failed");
    assert_eq!(failed.percent, 37);
    assert!(has(&effects, &Effect::Cleanup));
}

#[test]
fn done_for_another_build_waits_while_live_then_fails_when_the_job_is_gone() {
    let mut state = PollState::new(RUN);
    let waiting = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        live(),
        LockState::HeldAlive,
        OTHER_FULL,
        0,
        0,
    );
    assert!(!waiting
        .iter()
        .any(|e| matches!(e, Effect::PublishDone(_) | Effect::PublishFailed(_))));
    let gone = tick(
        &mut state,
        Some(&status_bytes("done", 100, "ok")),
        dead(),
        LockState::Free,
        OTHER_FULL,
        0,
        1000,
    );
    assert!(gone.iter().any(|e| matches!(e, Effect::PublishFailed(p)
        if p.message == "running version does not match the update target")));
}

// ---- silent death, interruption, unreadable status -------------------------

#[test]
fn silently_dead_job_is_interrupted_after_the_threshold_then_cleaned_up() {
    let mut state = PollState::new(RUN);
    let before = tick(
        &mut state,
        Some(&status_bytes("compiling", 40, "building")),
        dead(),
        LockState::Free,
        FULL,
        10,
        0,
    );
    assert!(!before.iter().any(|e| matches!(e, Effect::WriteStatus(_))));
    assert!(!has(&before, &Effect::Cleanup));

    let at_threshold = tick(
        &mut state,
        Some(&status_bytes("compiling", 40, "building")),
        dead(),
        LockState::Free,
        FULL,
        INTERRUPTED_AFTER_SECS,
        1000,
    );
    let written = at_threshold
        .iter()
        .find_map(|e| match e {
            Effect::WriteStatus(s) => Some(s.clone()),
            _ => None,
        })
        .expect("WriteStatus");
    assert_eq!(written.phase, RunPhase::Failed);
    assert_eq!(written.message, INTERRUPTED_MESSAGE);
    assert_eq!(written.percent, 40);
    assert!(at_threshold
        .iter()
        .any(|e| matches!(e, Effect::PublishFailed(p)
        if p.message.starts_with("update interrupted"))));

    // The shell rewrote the file; the next read is the terminal failure.
    let rewritten = status_bytes("failed", 40, "interrupted");
    let cleaned = tick(
        &mut state,
        Some(&rewritten),
        dead(),
        LockState::Free,
        FULL,
        0,
        1500,
    );
    assert!(has(&cleaned, &Effect::Cleanup));
    assert!(has(&cleaned, &Effect::Stop));
    assert_eq!(
        cleaned
            .iter()
            .filter(|e| matches!(e, Effect::PublishFailed(_)))
            .count(),
        0,
        "the failure is published exactly once"
    );
}

#[test]
fn interruption_never_fires_while_the_job_has_a_process() {
    let mut state = PollState::new(RUN);
    let effects = tick(
        &mut state,
        Some(&status_bytes("compiling", 40, "building")),
        live(),
        LockState::HeldAlive,
        FULL,
        INTERRUPTED_AFTER_SECS * 10,
        0,
    );
    assert!(!effects
        .iter()
        .any(|e| matches!(e, Effect::WriteStatus(_) | Effect::Cleanup)));
}

#[test]
fn unreadable_status_fails_only_after_thirty_invalid_reads_with_the_job_dead() {
    let mut state = PollState::new(RUN);
    for read in 1..UNREADABLE_AFTER_READS {
        let effects = tick(
            &mut state,
            None,
            dead(),
            LockState::Free,
            FULL,
            0,
            u64::from(read),
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::PublishFailed(_))),
            "read {read} must not fail yet"
        );
    }
    let effects = tick(
        &mut state,
        None,
        dead(),
        LockState::Free,
        FULL,
        0,
        u64::from(UNREADABLE_AFTER_READS),
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::PublishFailed(p)
        if p.message == UNREADABLE_MESSAGE)));
    assert_eq!(effects.last(), Some(&Effect::Stop));
}

#[test]
fn unreadable_status_never_fails_while_the_job_is_live() {
    let mut state = PollState::new(RUN);
    for read in 0..(UNREADABLE_AFTER_READS * 3) {
        let effects = tick(
            &mut state,
            None,
            live(),
            LockState::HeldAlive,
            FULL,
            0,
            u64::from(read) * 500,
        );
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::PublishFailed(_))));
    }
}

#[test]
fn unreadable_failure_with_a_held_lock_publishes_but_keeps_everything_in_place() {
    let mut state = PollState::new(RUN);
    let mut last = Vec::new();
    for read in 0..UNREADABLE_AFTER_READS {
        last = tick(
            &mut state,
            None,
            dead(),
            LockState::HeldAlive,
            FULL,
            0,
            u64::from(read),
        );
    }
    assert!(last.iter().any(|e| matches!(e, Effect::PublishFailed(_))));
    assert!(!has(&last, &Effect::Cleanup));
    assert!(!has(&last, &Effect::Stop));
}

// ---- helpers ---------------------------------------------------------------

#[test]
fn done_waits_for_restart_only_for_a_live_mismatched_done() {
    let done = RunStatus {
        run_id: RUN.to_string(),
        phase: RunPhase::Done,
        percent: 100,
        target: "0123456".to_string(),
        target_full: FULL.to_string(),
        message: String::new(),
        method: StatusMethod::Exact,
        ts: NOW,
    };
    assert!(done_waits_for_restart(&done, OTHER_FULL, true));
    assert!(!done_waits_for_restart(&done, OTHER_FULL, false));
    assert!(!done_waits_for_restart(&done, FULL, true));
    assert!(!done_waits_for_restart(&done, "", true));
}

// ---- status age for a run without its own status (A5 fix 2) ----------------

fn exited_with(code: i32) -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: Some(code),
    }
}

#[test]
fn status_age_comes_from_the_status_only_once_this_run_has_one() {
    // No valid status for this run yet: the old status.json of a previous run
    // must not make the run look ancient; the plist age (or time since start) counts.
    assert_eq!(status_age_for(false, Some(999_999), Some(5), 2), 5);
    assert_eq!(status_age_for(false, Some(999_999), None, 2), 2);
    assert_eq!(status_age_for(false, None, None, 0), 0);
    // This run has a status: its own file age counts.
    assert_eq!(status_age_for(true, Some(42), Some(5), 2), 42);
    assert_eq!(status_age_for(true, None, Some(5), 2), 2);
}

#[test]
fn foreign_old_status_with_a_fresh_plist_does_not_clean_up_silently() {
    let mut state = PollState::new(RUN);
    let text = String::from_utf8(status_bytes("done", 100, "ok")).unwrap();
    let foreign = text.replace(RUN, "ffffffffffffffff").into_bytes();
    let first = tick(
        &mut state,
        Some(&foreign),
        dead(),
        LockState::Free,
        FULL,
        0,
        0,
    );
    assert!(!has(&first, &Effect::Cleanup));
    assert!(!first.iter().any(|e| matches!(e, Effect::PublishDone(_))));
}

#[test]
fn run_without_status_publishes_the_exit_then_cleans_up_once() {
    let mut state = PollState::new(RUN);
    let first = tick(
        &mut state,
        None,
        exited_with(1),
        LockState::Free,
        FULL,
        0,
        0,
    );
    let failed: Vec<&UpdateProgress> = first
        .iter()
        .filter_map(|e| match e {
            Effect::PublishFailed(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1, "{first:?}");
    assert_eq!(
        failed[0].message,
        "The update stopped unexpectedly (exit 1)."
    );
    assert!(!has(&first, &Effect::Cleanup), "publish before cleanup");

    let second = tick(
        &mut state,
        None,
        exited_with(1),
        LockState::Free,
        FULL,
        0,
        500,
    );
    assert!(has(&second, &Effect::Cleanup));
    assert!(has(&second, &Effect::Stop));
    assert!(!second.iter().any(|e| matches!(e, Effect::PublishFailed(_))));
}
