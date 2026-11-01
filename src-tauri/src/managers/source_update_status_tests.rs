//! Tests for the automatic-update status builder (see `source_update_status.rs`).

use std::fs;

use super::{
    build_status, phase_for, stable_reason_code, AutoLastResult, AutoPhase, AutoUpdateStatus,
    StatusFacts,
};
use crate::managers::source_update_auto::{
    load_auto_state, save_auto_state, AutoResult, AutoState,
};
use crate::managers::source_update_pending::{
    encode_pending, staged_app_path, CommitInfo, PendingOrigin, PendingRun, PendingState,
    PendingStore, PENDING_FILE_NAME,
};
use crate::managers::source_update_runner::{job_label, plist_path};

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn built_run(dir: &std::path::Path) -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: TARGET.to_string(),
        staged_app: staged_app_path(dir, RUN),
        exe_name: "vox-bin".to_string(),
        exe_sha256: SHA.to_string(),
        info_plist_sha256: SHA.to_string(),
        origin: PendingOrigin::Auto,
        ts: 1_700_000_000,
        state: PendingState::Built,
    }
}

fn facts<'a>(
    active_run: bool,
    pending: Option<&'a PendingRun>,
    last: Option<&'a AutoState>,
) -> StatusFacts<'a> {
    StatusFacts {
        enabled: true,
        effective: true,
        active_run,
        pending,
        last,
    }
}

#[test]
fn phase_follows_the_active_run_and_the_pending_record() {
    let dir = tempfile::tempdir().unwrap();
    let built = built_run(dir.path());
    let committing = PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: INSTALL.to_string(),
            install_label: job_label(INSTALL),
            install_plist: plist_path(dir.path(), INSTALL),
            committing_ts: 1,
        }),
        ..built.clone()
    };
    assert_eq!(phase_for(false, None), AutoPhase::Idle);
    assert_eq!(phase_for(true, None), AutoPhase::Building);
    assert_eq!(phase_for(false, Some(&built)), AutoPhase::Waiting);
    assert_eq!(phase_for(true, Some(&built)), AutoPhase::Building);
    assert_eq!(phase_for(false, Some(&committing)), AutoPhase::Installing);
    assert_eq!(phase_for(true, Some(&committing)), AutoPhase::Installing);
}

#[test]
fn status_without_any_record_is_idle_with_no_attempt() {
    let status = build_status(&StatusFacts {
        enabled: false,
        effective: false,
        active_run: false,
        pending: None,
        last: None,
    });
    assert_eq!(
        status,
        AutoUpdateStatus {
            enabled: false,
            effective: false,
            phase: AutoPhase::Idle,
            last_result: AutoLastResult::NoAttempt,
            reason_code: None,
            target_short: None,
            ts: None,
        }
    );
}

#[test]
fn failed_attempt_shows_its_stable_code_and_the_short_target() {
    let last = AutoState::default().with_failure(TARGET, "interrupted", 42);
    let status = build_status(&facts(false, None, Some(&last)));
    assert_eq!(status.last_result, AutoLastResult::Failed);
    assert_eq!(status.reason_code.as_deref(), Some("interrupted"));
    assert_eq!(status.target_short.as_deref(), Some("01234567"));
    assert_eq!(status.ts, Some(42));
}

#[test]
fn skipped_attempt_shows_its_policy_code() {
    let last = AutoState::default().with_skipped(TARGET, "backoff", 7);
    let status = build_status(&facts(false, None, Some(&last)));
    assert_eq!(status.last_result, AutoLastResult::Skipped);
    assert_eq!(status.reason_code.as_deref(), Some("backoff"));
}

#[test]
fn free_text_in_a_reason_is_never_shown() {
    let last = AutoState {
        last_target: Some(TARGET.to_string()),
        last_result: Some(AutoResult::Failed),
        reason_code: Some("network is down".to_string()),
        ts: 1,
        failed_target: Some(TARGET.to_string()),
        failed_ts: Some(1),
    };
    let status = build_status(&facts(false, None, Some(&last)));
    assert_eq!(status.reason_code, None);
}

#[test]
fn updated_attempt_has_no_reason_code() {
    let last = AutoState::default().with_updated(TARGET, 9);
    let status = build_status(&facts(false, None, Some(&last)));
    assert_eq!(status.last_result, AutoLastResult::Updated);
    assert_eq!(status.reason_code, None);
}

#[test]
fn a_resting_build_is_waiting_for_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let built = built_run(dir.path());
    let status = build_status(&facts(false, Some(&built), None));
    assert_eq!(status.phase, AutoPhase::Waiting);
    assert_eq!(status.target_short.as_deref(), Some("01234567"));
    assert_eq!(status.last_result, AutoLastResult::NoAttempt);
}

#[test]
fn stable_codes_pass_and_other_text_does_not() {
    assert_eq!(stable_reason_code("busy"), Some("busy"));
    assert_eq!(stable_reason_code("install_failed"), Some("install_failed"));
    assert_eq!(stable_reason_code("Busy"), None);
    assert_eq!(stable_reason_code(""), None);
}

#[test]
fn wire_names_are_the_documented_ones() {
    let status = build_status(&facts(true, None, None));
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(value["phase"], "building");
    assert_eq!(value["last_result"], "none");
    assert_eq!(value["enabled"], true);
}

#[test]
fn builder_reads_the_real_files() {
    let dir = tempfile::tempdir().unwrap();
    let failed = AutoState::default().with_failure(TARGET, "build_failed", 11);
    save_auto_state(dir.path(), &failed).unwrap();
    let built = built_run(dir.path());
    fs::write(
        dir.path().join(PENDING_FILE_NAME),
        encode_pending(&built, dir.path()).unwrap(),
    )
    .unwrap();

    let last = load_auto_state(dir.path()).unwrap();
    let pending = PendingStore::new(dir.path()).load().unwrap();
    let status = build_status(&StatusFacts {
        enabled: true,
        effective: true,
        active_run: false,
        pending: pending.as_ref(),
        last: last.as_ref(),
    });

    assert_eq!(status.phase, AutoPhase::Waiting);
    assert_eq!(status.last_result, AutoLastResult::Failed);
    assert_eq!(status.reason_code.as_deref(), Some("build_failed"));
    assert_eq!(status.target_short.as_deref(), Some("01234567"));
    assert_eq!(status.ts, Some(11));
}
