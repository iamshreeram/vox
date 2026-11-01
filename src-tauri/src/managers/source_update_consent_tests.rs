//! Tests for the consent-dependent automatic-update state (see `source_update_consent.rs`).

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{
    apply_manual_conversion, auto_marker_path, auto_pending, cleanup_after_build_without_consent,
    cleanup_plan, manual_conversion_plan, silent_for_run, AutoMarker, AutoPendingFacts,
    CleanupPlan, CommitRequest,
};
use crate::managers::source_update::{SourceUpdateInfo, SourceUpdateState};
use crate::managers::source_update_auto::{
    load_auto_state, save_auto_state, AutoResult, AutoState, SkipReason, WaitReason,
};
use crate::managers::source_update_pending::{
    encode_pending, staged_app_path, CommitInfo, PendingOrigin, PendingRun, PendingState,
    PENDING_FILE_NAME, STAGED_DIR_NAME,
};
use crate::managers::source_update_runner::{job_label, plist_path};

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_TARGET: &str = "fedcba9876543210fedcba9876543210fedcba98";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn built_run(dir: &Path, origin: PendingOrigin) -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: TARGET.to_string(),
        staged_app: staged_app_path(dir, RUN),
        exe_name: "vox-bin".to_string(),
        exe_sha256: SHA.to_string(),
        info_plist_sha256: SHA.to_string(),
        origin,
        ts: 1_700_000_000,
        state: PendingState::Built,
    }
}

fn committing_run(dir: &Path) -> PendingRun {
    PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: INSTALL.to_string(),
            install_label: job_label(INSTALL),
            install_plist: plist_path(dir, INSTALL),
            committing_ts: 1_700_000_100,
        }),
        ..built_run(dir, PendingOrigin::Auto)
    }
}

fn write_pending(dir: &Path, run: &PendingRun) {
    let bytes = encode_pending(run, dir).expect("encode");
    fs::write(dir.join(PENDING_FILE_NAME), bytes).expect("write pending");
}

fn make_staged(dir: &Path) -> PathBuf {
    let app = staged_app_path(dir, RUN);
    fs::create_dir_all(&app).expect("staged dir");
    app
}

fn staged_run_dir(dir: &Path) -> PathBuf {
    dir.join(STAGED_DIR_NAME).join(RUN)
}

#[test]
fn marker_round_trips_with_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let markers = AutoMarker::new(dir.path());
    assert!(!markers.exists(RUN));
    markers.write(RUN).unwrap();
    assert!(markers.exists(RUN));
    let path = auto_marker_path(dir.path(), RUN).unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    markers.remove(RUN).unwrap();
    assert!(!markers.exists(RUN));
    markers.remove(RUN).expect("removing twice is fine");
}

#[test]
fn marker_refuses_symlinks_and_malformed_run_ids() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::write(&elsewhere, "x").unwrap();
    let path = auto_marker_path(dir.path(), RUN).unwrap();
    symlink(&elsewhere, &path).unwrap();
    let markers = AutoMarker::new(dir.path());
    assert!(!markers.exists(RUN), "a symlink is never a marker");
    assert!(markers.write(RUN).is_err());
    assert!(markers.remove(RUN).is_err());
    assert!(fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(elsewhere.exists(), "the link target is never touched");

    assert!(markers.write("../x").is_err());
    assert!(!markers.exists("../x"));
    assert!(markers.remove("ZZ").is_err());
    assert!(auto_marker_path(dir.path(), "ZZ").is_none());
}

#[test]
fn silent_flag_follows_the_marker_of_that_run_only() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!silent_for_run(dir.path(), RUN));
    AutoMarker::new(dir.path()).write(RUN).unwrap();
    assert!(silent_for_run(dir.path(), RUN));
    assert!(
        !silent_for_run(dir.path(), INSTALL),
        "another run is not silent"
    );
    AutoMarker::new(dir.path()).remove(RUN).unwrap();
    assert!(!silent_for_run(dir.path(), RUN));
}

#[test]
fn auto_pending_rules() {
    let dir = tempfile::tempdir().unwrap();
    let auto_built = built_run(dir.path(), PendingOrigin::Auto);
    let manual_built = built_run(dir.path(), PendingOrigin::Manual);
    let mut other_target = built_run(dir.path(), PendingOrigin::Auto);
    other_target.target_full = OTHER_TARGET.to_string();
    let committing = committing_run(dir.path());
    let failed_same = AutoState::default().with_failure(TARGET, "build_failed", 10);
    let failed_other = AutoState::default().with_failure(OTHER_TARGET, "build_failed", 10);
    let skipped_same = AutoState::default().with_skipped(TARGET, "backoff", 10);

    let check =
        |effective: bool, marked: bool, pending: Option<&PendingRun>, last: Option<&AutoState>| {
            auto_pending(&AutoPendingFacts {
                auto_effective: effective,
                active_run_marked: marked,
                pending,
                target: TARGET,
                last,
            })
        };
    assert!(
        !check(false, true, Some(&auto_built), None),
        "auto off hides nothing"
    );
    assert!(!check(true, false, None, None), "nothing healthy");
    assert!(check(true, true, None, None), "an automatic run is active");
    assert!(
        check(true, false, Some(&auto_built), None),
        "built for this target"
    );
    assert!(check(true, false, Some(&committing), None), "committing");
    assert!(
        !check(true, false, Some(&manual_built), None),
        "a manual build is not automatic"
    );
    assert!(
        !check(true, false, Some(&other_target), None),
        "another target"
    );
    assert!(
        !check(true, true, Some(&auto_built), Some(&failed_same)),
        "failed attempt"
    );
    assert!(
        !check(true, false, Some(&auto_built), Some(&skipped_same)),
        "skipped attempt"
    );
    assert!(
        check(true, true, None, Some(&failed_other)),
        "failure of another target"
    );
}

#[test]
fn cleanup_plan_only_removes_a_built_run_without_consent() {
    let dir = tempfile::tempdir().unwrap();
    let built = built_run(dir.path(), PendingOrigin::Auto);
    let committing = committing_run(dir.path());
    assert_eq!(cleanup_plan(Some(&built), false), CleanupPlan::RemoveBuilt);
    assert_eq!(cleanup_plan(Some(&built), true), CleanupPlan::Keep);
    assert_eq!(cleanup_plan(Some(&committing), false), CleanupPlan::Keep);
    assert_eq!(cleanup_plan(None, false), CleanupPlan::Keep);
}

#[test]
fn built_run_without_consent_is_removed_and_recorded_as_skipped() {
    let dir = tempfile::tempdir().unwrap();
    write_pending(dir.path(), &built_run(dir.path(), PendingOrigin::Auto));
    make_staged(dir.path());

    let plan = cleanup_after_build_without_consent(dir.path(), false, 500).unwrap();

    assert_eq!(plan, CleanupPlan::RemoveBuilt);
    assert!(!dir.path().join(PENDING_FILE_NAME).exists());
    assert!(!staged_run_dir(dir.path()).exists());
    let state = load_auto_state(dir.path())
        .unwrap()
        .expect("state recorded");
    assert_eq!(state.last_result, Some(AutoResult::Skipped));
    assert_eq!(state.reason_code.as_deref(), Some("disabled"));
    assert_eq!(state.last_target.as_deref(), Some(TARGET));
    assert_eq!(state.ts, 500);
}

#[test]
fn cleanup_keeps_earlier_failure_memory() {
    let dir = tempfile::tempdir().unwrap();
    let earlier = AutoState::default().with_failure(OTHER_TARGET, "interrupted", 100);
    save_auto_state(dir.path(), &earlier).unwrap();
    write_pending(dir.path(), &built_run(dir.path(), PendingOrigin::Auto));

    cleanup_after_build_without_consent(dir.path(), false, 500).unwrap();

    let state = load_auto_state(dir.path()).unwrap().unwrap();
    assert_eq!(state.failed_target.as_deref(), Some(OTHER_TARGET));
    assert_eq!(state.failed_ts, Some(100));
}

#[test]
fn committing_run_is_never_touched_by_the_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let committing = committing_run(dir.path());
    write_pending(dir.path(), &committing);
    make_staged(dir.path());
    let before = fs::read(dir.path().join(PENDING_FILE_NAME)).unwrap();

    let plan = cleanup_after_build_without_consent(dir.path(), false, 500).unwrap();

    assert_eq!(plan, CleanupPlan::Keep);
    assert_eq!(
        fs::read(dir.path().join(PENDING_FILE_NAME)).unwrap(),
        before
    );
    assert!(staged_run_dir(dir.path()).exists());
    assert!(
        load_auto_state(dir.path()).unwrap().is_none(),
        "nothing recorded"
    );
}

#[test]
fn built_run_is_kept_while_consent_holds() {
    let dir = tempfile::tempdir().unwrap();
    write_pending(dir.path(), &built_run(dir.path(), PendingOrigin::Auto));
    let plan = cleanup_after_build_without_consent(dir.path(), true, 500).unwrap();
    assert_eq!(plan, CleanupPlan::Keep);
    assert!(dir.path().join(PENDING_FILE_NAME).exists());
}

#[test]
fn opt_out_during_a_build_leaves_nothing_to_commit() {
    // R9-1: the build is not cancelled. Opt-out happens first (nothing to remove yet), the
    // build then finishes and writes its record, and that record is removed when it lands.
    let dir = tempfile::tempdir().unwrap();
    let at_opt_out = cleanup_after_build_without_consent(dir.path(), false, 400).unwrap();
    assert_eq!(at_opt_out, CleanupPlan::Keep);

    write_pending(dir.path(), &built_run(dir.path(), PendingOrigin::Auto));
    make_staged(dir.path());
    let when_built = cleanup_after_build_without_consent(dir.path(), false, 500).unwrap();

    assert_eq!(when_built, CleanupPlan::RemoveBuilt);
    assert!(
        !dir.path().join(PENDING_FILE_NAME).exists(),
        "no record to commit"
    );
    assert!(
        !staged_run_dir(dir.path()).exists(),
        "no staged artifact remains"
    );
}

#[test]
fn manual_conversion_plan_table() {
    assert_eq!(
        manual_conversion_plan(false, false),
        super::ManualConversion {
            remove_active_marker: false,
            request_commit: false
        }
    );
    assert!(manual_conversion_plan(true, false).remove_active_marker);
    assert!(!manual_conversion_plan(true, false).request_commit);
    assert!(manual_conversion_plan(false, true).request_commit);
    assert!(!manual_conversion_plan(false, true).remove_active_marker);
}

#[test]
fn manual_start_converts_an_active_automatic_run() {
    let dir = tempfile::tempdir().unwrap();
    AutoMarker::new(dir.path()).write(RUN).unwrap();

    let plan = apply_manual_conversion(dir.path(), Some(RUN)).unwrap();

    assert!(plan.remove_active_marker);
    assert!(!plan.request_commit);
    assert!(
        !silent_for_run(dir.path(), RUN),
        "later progress is not silent"
    );
}

#[test]
fn manual_start_requests_a_commit_of_a_built_update() {
    let dir = tempfile::tempdir().unwrap();
    write_pending(dir.path(), &built_run(dir.path(), PendingOrigin::Auto));
    AutoMarker::new(dir.path()).write(RUN).unwrap();

    let plan = apply_manual_conversion(dir.path(), None).unwrap();

    assert!(plan.request_commit);
    assert!(
        !silent_for_run(dir.path(), RUN),
        "the built run's marker is dropped"
    );
    assert!(super::take_manual_commit_request(RUN));
    assert!(
        !super::take_manual_commit_request(RUN),
        "the request is one-shot"
    );
}

#[test]
fn commit_request_is_one_shot() {
    let request = CommitRequest::new();
    assert!(!request.take(RUN));
    request.request(RUN);
    assert!(request.take(RUN));
    assert!(!request.take(RUN));
}

// B6 (validator major 2): a manual request is tied to the built run it was made for. Another
// run's request never applies, and a request is not visible after its run is gone.
#[test]
fn b6_manual_request_only_applies_to_the_run_it_was_made_for() {
    const FIRST: &str = "aaaaaaaaaaaaaaaa";
    const SECOND: &str = "bbbbbbbbbbbbbbbb";
    let request = CommitRequest::new();
    request.request(FIRST);
    assert!(request.holds(FIRST));
    assert!(!request.holds(SECOND));
    assert!(!request.take(SECOND), "a later run never consumes it");
    assert!(request.take(FIRST));
    assert!(!request.holds(FIRST));
}

// B6 (validator major 2): the process-wide request is run-scoped as well.
#[test]
fn b6_global_manual_request_is_scoped_to_its_run() {
    const FIRST: &str = "aaaaaaaaaaaaaaaa";
    const SECOND: &str = "bbbbbbbbbbbbbbbb";
    super::request_manual_commit(FIRST);
    assert!(super::manual_commit_requested(FIRST));
    assert!(!super::manual_commit_requested(SECOND));
    assert!(!super::take_manual_commit_request(SECOND));
    assert!(super::take_manual_commit_request(FIRST));
}

#[test]
fn every_policy_code_is_a_stable_reason_code() {
    let skips = [
        SkipReason::Disabled,
        SkipReason::NotSource,
        SkipReason::NoUpdate,
        SkipReason::Blocked,
        SkipReason::Backoff,
        SkipReason::Unverified,
        SkipReason::VerifyFailed,
    ];
    let waits = [
        WaitReason::ActiveRun,
        WaitReason::Busy,
        WaitReason::NotIdle,
        WaitReason::Verifying,
    ];
    for code in skips
        .iter()
        .map(|r| r.code())
        .chain(waits.iter().map(|r| r.code()))
    {
        assert_eq!(
            crate::managers::source_update_auto::status::stable_reason_code(code),
            Some(code)
        );
    }
    assert_eq!(
        crate::managers::source_update_auto::status::stable_reason_code("network timed out"),
        None
    );
}

#[test]
fn published_info_without_the_flag_reads_as_not_pending() {
    let info = SourceUpdateInfo {
        state: SourceUpdateState::Available,
        current_commit: "0123456".to_string(),
        latest_commit: "fedcba9".to_string(),
        latest_full: TARGET.to_string(),
        behind_by: 1,
        changes: Vec::new(),
        compare_url: String::new(),
        note: None,
        can_install: true,
        install_blocker: None,
        auto_pending: true,
    };
    let mut value = serde_json::to_value(&info).unwrap();
    value.as_object_mut().unwrap().remove("auto_pending");
    let back: SourceUpdateInfo = serde_json::from_value(value).unwrap();
    assert!(!back.auto_pending);
}

#[test]
fn install_is_silent_only_for_an_automatic_commit_without_conversion() {
    // R7-7 a: the install marker is written only for an automatic record that no manual
    // conversion touched (the `manual` input of the commit covers both conversion paths).
    assert!(super::install_is_silent(PendingOrigin::Auto, false));
    assert!(!super::install_is_silent(PendingOrigin::Auto, true));
    assert!(!super::install_is_silent(PendingOrigin::Manual, false));
    assert!(!super::install_is_silent(PendingOrigin::Manual, true));
}

#[test]
fn adopted_install_is_silent_exactly_when_an_auto_marker_exists() {
    // Restore adoption recomputes silence from the record origin and the marker on disk.
    assert!(super::silent_after_adoption(PendingOrigin::Auto, true));
    assert!(!super::silent_after_adoption(PendingOrigin::Auto, false));
    assert!(!super::silent_after_adoption(PendingOrigin::Manual, true));
    assert!(!super::silent_after_adoption(PendingOrigin::Manual, false));
}

#[test]
fn progress_of_an_adopted_install_follows_origin_and_marker() {
    // The install run of a committing record is silent only for an automatic origin with its
    // marker on disk. Every other run is silent exactly when its marker exists.
    let dir = tempfile::tempdir().expect("dir");
    write_pending(dir.path(), &committing_run(dir.path()));
    assert!(
        !super::progress_silent(dir.path(), INSTALL),
        "no marker yet"
    );
    AutoMarker::new(dir.path()).write(INSTALL).expect("marker");
    assert!(super::progress_silent(dir.path(), INSTALL));

    let mut manual = committing_run(dir.path());
    manual.origin = PendingOrigin::Manual;
    write_pending(dir.path(), &manual);
    assert!(
        !super::progress_silent(dir.path(), INSTALL),
        "a manual origin is never silent"
    );
    AutoMarker::new(dir.path())
        .write(RUN)
        .expect("build marker");
    assert!(
        super::progress_silent(dir.path(), RUN),
        "other runs use the marker alone"
    );
}
