//! Tests for the optional-model commands: status mapping, download admission,
//! the progress throttle, event payload shape, and cancel/delete on a real
//! `AuxModelManager` over a temp root (no network).

use super::*;
use crate::managers::aux_model::{aux_spec, AuxKind, AuxState};
use serde_json::Value;

const ENUM_LEAVES: [&str; 13] = [
    "smart_turn",
    "speaker_resnet34",
    "not_installed",
    "downloading",
    "verifying",
    "ready",
    "error",
    "busy",
    "cancelled",
    "corrupt",
    "url_not_allowed",
    "download",
    "io",
];

/// Every leaf of a payload must be a number, null, or a known enum name.
fn assert_scalar_or_enum_leaves(value: &Value) {
    match value {
        Value::Object(map) => map.values().for_each(assert_scalar_or_enum_leaves),
        Value::Number(_) | Value::Null => {}
        Value::String(name) => assert!(
            ENUM_LEAVES.contains(&name.as_str()),
            "payload string is not an enum name: {name:?}"
        ),
        other => panic!("payload leaf must be number/enum/null, got {other:?}"),
    }
}

fn temp_models() -> (tempfile::TempDir, AuxModels) {
    let dir = tempfile::tempdir().expect("temp dir");
    let models = AuxModels::new(dir.path().join("aux_models"));
    (dir, models)
}

#[test]
fn status_is_not_installed_by_default_in_a_temp_root() {
    let (_dir, models) = temp_models();
    for kind in [AuxKind::SmartTurn, AuxKind::SpeakerResnet34] {
        let state = models.for_kind(kind).state();
        assert_eq!(state, AuxState::NotInstalled);
        assert_eq!(build_status(kind, state).state, AuxState::NotInstalled);
    }
}

#[test]
fn status_size_comes_from_the_pinned_spec() {
    assert_eq!(
        build_status(AuxKind::SmartTurn, AuxState::Ready).size_bytes,
        8_679_182
    );
    assert_eq!(
        build_status(AuxKind::SpeakerResnet34, AuxState::Ready).size_bytes,
        aux_spec(AuxKind::SpeakerResnet34).size
    );
}

#[test]
fn status_carries_every_state_unchanged() {
    for state in [
        AuxState::NotInstalled,
        AuxState::Downloading,
        AuxState::Verifying,
        AuxState::Ready,
        AuxState::Error,
    ] {
        let status = build_status(AuxKind::SmartTurn, state);
        assert_eq!(status.kind, AuxKind::SmartTurn);
        assert_eq!(status.state, state);
    }
}

#[test]
fn status_payload_contains_only_numbers_and_enums() {
    let value = serde_json::to_value(build_status(
        AuxKind::SpeakerResnet34,
        AuxState::Downloading,
    ))
    .expect("serialize");
    assert_scalar_or_enum_leaves(&value);
    assert_eq!(value["kind"], "speaker_resnet34");
    assert_eq!(value["state"], "downloading");
}

#[test]
fn download_is_refused_as_busy_only_while_an_operation_runs() {
    assert_eq!(
        download_admission(AuxState::Downloading),
        Err(AuxError::Busy)
    );
    assert_eq!(download_admission(AuxState::Verifying), Err(AuxError::Busy));
    assert_eq!(download_admission(AuxState::NotInstalled), Ok(()));
    assert_eq!(download_admission(AuxState::Ready), Ok(()));
    assert_eq!(download_admission(AuxState::Error), Ok(()));
}

#[test]
fn delete_is_idempotent_on_an_empty_root() {
    let (_dir, models) = temp_models();
    let manager = models.for_kind(AuxKind::SmartTurn);
    assert_eq!(manager.delete(), Ok(()));
    assert_eq!(manager.delete(), Ok(()));
    assert_eq!(manager.state(), AuxState::NotInstalled);
}

#[test]
fn cancel_when_idle_is_a_no_op() {
    let (_dir, models) = temp_models();
    let manager = models.for_kind(AuxKind::SpeakerResnet34);
    manager.cancel();
    manager.cancel();
    assert_eq!(manager.state(), AuxState::NotInstalled);
}

#[test]
fn the_first_progress_sample_is_always_emitted() {
    let mut throttle = ProgressThrottle::new();
    assert!(throttle.accept(0, 1, 100));
}

#[test]
fn samples_inside_one_hundred_ms_are_dropped() {
    let mut throttle = ProgressThrottle::new();
    assert!(throttle.accept(0, 10, 1000));
    assert!(!throttle.accept(50, 20, 1000));
    assert!(!throttle.accept(99, 30, 1000));
    assert!(throttle.accept(100, 40, 1000));
}

#[test]
fn a_dense_stream_emits_at_most_one_event_per_hundred_ms() {
    let mut throttle = ProgressThrottle::new();
    let mut emitted_at = Vec::new();
    for step in 0..1000u64 {
        // One sample per millisecond; the last one (999 of 999) is the completed download.
        if throttle.accept(step, step, 999) {
            emitted_at.push(step);
        }
    }
    assert!(emitted_at.len() >= 2);
    let last = *emitted_at.last().expect("events");
    let body = &emitted_at[..emitted_at.len() - 1];
    for pair in body.windows(2) {
        assert!(pair[1] - pair[0] >= 100, "events too close: {pair:?}");
    }
    assert_eq!(last, 999, "the final sample must always be emitted");
}

#[test]
fn the_final_sample_is_emitted_even_inside_the_window() {
    let mut throttle = ProgressThrottle::new();
    assert!(throttle.accept(0, 1, 1000));
    assert!(throttle.accept(120, 500, 1000));
    assert!(
        throttle.accept(130, 1000, 1000),
        "completion must never be throttled away"
    );
}

#[test]
fn an_unknown_total_is_never_treated_as_final() {
    let mut throttle = ProgressThrottle::new();
    assert!(throttle.accept(0, 0, 0));
    assert!(!throttle.accept(10, 0, 0));
}

#[test]
fn progress_event_copies_the_numbers_and_kind() {
    let event = progress_event(AuxProgress {
        kind: AuxKind::SmartTurn,
        downloaded: 7,
        total: 9,
    });
    assert_eq!(
        event,
        AuxProgressEvent {
            kind: AuxKind::SmartTurn,
            downloaded: 7,
            total: 9,
        }
    );
}

#[test]
fn progress_payload_has_only_numbers_and_enums() {
    let event = progress_event(AuxProgress {
        kind: AuxKind::SpeakerResnet34,
        downloaded: 1,
        total: 2,
    });
    let value = serde_json::to_value(event).expect("serialize");
    assert_scalar_or_enum_leaves(&value);
    assert_eq!(value.as_object().map(|m| m.len()), Some(3));
}

#[test]
fn finished_success_has_no_error_and_a_failure_names_the_reason() {
    let ok = finished_event(AuxKind::SmartTurn, AuxState::Ready, Ok(()));
    assert_eq!(ok.state, AuxState::Ready);
    assert_eq!(ok.error, None);
    let failed = finished_event(
        AuxKind::SmartTurn,
        AuxState::NotInstalled,
        Err(AuxError::Corrupt),
    );
    assert_eq!(failed.error, Some(AuxError::Corrupt));
    let value = serde_json::to_value(failed).expect("serialize");
    assert_scalar_or_enum_leaves(&value);
    assert_eq!(value["error"], "corrupt");
    let ok_value = serde_json::to_value(ok).expect("serialize");
    assert!(ok_value["error"].is_null());
}

#[test]
fn event_names_are_stable() {
    assert_eq!(AUX_PROGRESS_EVENT, "aux-model-progress");
    assert_eq!(AUX_FINISHED_EVENT, "aux-model-finished");
}
