//! Manager-level tests for the background-update wiring: the pinned target
//! precondition, `latest_full` on every check path, and the production-source
//! scan that keeps the removed Terminal launcher from coming back.
//! Wired from `source_update.rs` with `#[path]`.

use super::*;

fn full_sha(n: u32) -> String {
    format!("{:040x}", 0x1234_0000_u128 + n as u128)
}

fn info(state: SourceUpdateState, latest_full: &str) -> SourceUpdateInfo {
    SourceUpdateInfo {
        state,
        current_commit: "0123456".to_string(),
        latest_commit: short_sha(latest_full),
        latest_full: latest_full.to_string(),
        behind_by: 1,
        changes: Vec::new(),
        compare_url: String::new(),
        note: None,
        can_install: false,
        install_blocker: None,
        auto_pending: false,
    }
}

#[test]
fn pinned_target_accepts_only_an_available_lowercase_full_sha() {
    let full = full_sha(7);
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, &full))),
        Some(full.clone())
    );
    assert_eq!(pinned_target(None), None);
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::UpToDate, &full))),
        None
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Untracked, &full))),
        None
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, ""))),
        None,
        "an unknown full SHA must refuse to start"
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, "0123456"))),
        None,
        "a short SHA must refuse to start"
    );
    assert_eq!(
        pinned_target(Some(&info(
            SourceUpdateState::Available,
            "ABCDEF0123456789ABCDEF0123456789ABCDEF01"
        ))),
        None,
        "uppercase hex must refuse to start"
    );
}

#[test]
fn up_to_date_and_available_checks_carry_the_full_latest_sha() {
    let build = full_sha(1);
    let newest = full_sha(2);
    let window = vec![
        RemoteCommit {
            sha: newest.clone(),
            subject: "new".to_string(),
        },
        RemoteCommit {
            sha: build.clone(),
            subject: "old".to_string(),
        },
    ];
    match evaluate_window(&build, &window).unwrap() {
        WindowEvaluation::Resolved(info) => {
            assert_eq!(info.state, SourceUpdateState::Available);
            assert_eq!(info.latest_full, newest);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    let up_to_date = vec![RemoteCommit {
        sha: build.clone(),
        subject: "same".to_string(),
    }];
    match evaluate_window(&build, &up_to_date).unwrap() {
        WindowEvaluation::Resolved(info) => assert_eq!(info.latest_full, build),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn compare_fallback_path_carries_the_full_latest_sha() {
    let build = full_sha(1);
    let latest = full_sha(9);
    let ahead = resolve_compare(
        &build,
        &latest,
        vec!["x".to_string()],
        Ok(CompareStatus::Ahead { ahead_by: 3 }),
    )
    .unwrap();
    assert_eq!(ahead.state, SourceUpdateState::Available);
    assert_eq!(ahead.latest_full, latest);

    let same = resolve_compare(&build, &latest, Vec::new(), Ok(CompareStatus::Identical)).unwrap();
    assert_eq!(same.latest_full, latest);

    let diverged =
        resolve_compare(&build, &latest, Vec::new(), Ok(CompareStatus::Diverged)).unwrap();
    assert_eq!(diverged.state, SourceUpdateState::Untracked);
    assert_eq!(diverged.latest_full, latest);
}

#[test]
fn latest_full_is_empty_when_the_latest_sha_is_not_a_full_sha() {
    let build = full_sha(1);
    let odd = resolve_compare(
        &build,
        "abc1234",
        Vec::new(),
        Ok(CompareStatus::Ahead { ahead_by: 1 }),
    )
    .unwrap();
    assert_eq!(odd.latest_full, "");
}

/// The part of a source file before its first `#[cfg(test)]` (the test modules).
fn production_part(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(index) => &text[..index],
        None => text,
    }
}

/// Production code of the update feature must not contain any trace of the
/// removed Terminal launcher: no Terminal app, no osascript, no `open -a`, no
/// `.command` script file. Comments count too.
#[test]
fn production_update_code_has_no_terminal_launcher_left() {
    let sources = [
        (
            "managers/source_update.rs",
            include_str!("source_update.rs"),
        ),
        (
            "managers/source_update_runner.rs",
            include_str!("source_update_runner.rs"),
        ),
        (
            "managers/source_update_runner_poll.rs",
            include_str!("source_update_runner_poll.rs"),
        ),
        (
            "managers/source_update_runner_poller.rs",
            include_str!("source_update_runner_poller.rs"),
        ),
        (
            "managers/source_update_runner_run.rs",
            include_str!("source_update_runner_run.rs"),
        ),
        (
            "commands/source_update.rs",
            include_str!("../commands/source_update.rs"),
        ),
    ];
    for (name, text) in sources {
        let production = production_part(text);
        for needle in ["Terminal", "osascript", "open -a", ".command"] {
            assert!(
                !production.contains(needle),
                "{name} production code still mentions {needle:?}"
            );
        }
    }
}

// ---- launch guard release and attach rules (A5 fixes 1 and 3) ---------------

use crate::managers::source_update_runner::LockState;

const GUARD_RUN: &str = "aaaaaaaaaaaaaaaa";
const GUARD_OTHER: &str = "bbbbbbbbbbbbbbbb";

fn active_run(run_id: &str) -> ActiveRun {
    ActiveRun {
        run_id: run_id.to_string(),
        label: format!("com.iamshreeram.vox.update.{run_id}"),
        started: Instant::now(),
    }
}

fn progress_for(run_id: &str, phase: RunPhase) -> UpdateProgress {
    UpdateProgress {
        run_id: run_id.to_string(),
        phase,
        percent: 40,
        target: "0123456".to_string(),
        message: String::new(),
        estimated: false,
        silent: false,
    }
}

#[test]
fn a_run_that_ends_releases_the_launch_guard_for_a_retry() {
    let runtime = SourceUpdateRuntime::new();
    let t0 = Instant::now();
    assert!(runtime.guard.try_acquire(t0));
    *lock_state(&runtime.active) = Some(active_run(GUARD_RUN));
    // A quick failure: the run ends well inside the five-minute window.
    finish_run(&runtime, GUARD_RUN);
    assert!(lock_state(&runtime.active).is_none());
    assert!(
        runtime.guard.try_acquire(t0 + Duration::from_secs(3)),
        "a retry after a finished run must not be refused as busy"
    );
}

#[test]
fn ending_another_run_keeps_the_guard_and_the_active_run() {
    let runtime = SourceUpdateRuntime::new();
    let t0 = Instant::now();
    assert!(runtime.guard.try_acquire(t0));
    *lock_state(&runtime.active) = Some(active_run(GUARD_RUN));
    finish_run(&runtime, GUARD_OTHER);
    assert!(lock_state(&runtime.active).is_some());
    assert!(!runtime.guard.try_acquire(t0 + Duration::from_secs(3)));
}

#[test]
fn a_double_start_while_the_run_is_active_attaches_to_it() {
    let run = active_run(GUARD_RUN);
    let running = progress_for(GUARD_RUN, RunPhase::Compiling);
    assert_eq!(
        attachable_run(Some(&run), Some(&running)).map(|r| r.run_id),
        Some(GUARD_RUN.to_string())
    );
    assert_eq!(
        attachable_run(Some(&run), None).map(|r| r.run_id),
        Some(GUARD_RUN.to_string()),
        "no progress cached yet still attaches"
    );
    assert_eq!(
        start_gate(
            attachable_run(Some(&run), Some(&running)).is_some(),
            LockState::Free
        ),
        StartGate::Attach
    );
}

#[test]
fn a_settled_run_is_not_attached_to_and_other_runs_do_not_settle_it() {
    let run = active_run(GUARD_RUN);
    for phase in [RunPhase::Done, RunPhase::Failed] {
        let settled = progress_for(GUARD_RUN, phase);
        assert!(
            attachable_run(Some(&run), Some(&settled)).is_none(),
            "{phase:?} must start a fresh run instead of attaching"
        );
    }
    let foreign = progress_for(GUARD_OTHER, RunPhase::Failed);
    assert!(attachable_run(Some(&run), Some(&foreign)).is_some());
    assert!(attachable_run(None, None).is_none());
}
