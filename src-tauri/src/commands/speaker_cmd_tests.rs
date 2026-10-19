//! Pinned by S12: the pure speaker command functions (`commands/speaker.rs`) over a real
//! `OwnershipGate` and fake recorder, embedder and clock. No model, no microphone.

use super::*;
use crate::managers::ownership::{Owner, OwnershipGate};
use crate::managers::speaker_enroll::{
    EnrollClock, EnrollConfig, EnrollEmbedder, EnrollRecorder, EnrollmentController, RecorderBusy,
};
use crate::managers::speaker_id::{VoiceprintStore, SPEAKER_SPEC};
use crate::managers::voice_models::{ModelKind, ModelSession, SessionLoader, VoiceModelService};
use crate::managers::voice_runtime::{SettingsGetter, VoiceRuntime};
use crate::settings::get_default_settings;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

struct NoModel;

impl SessionLoader for NoModel {
    fn load(&self, _kind: ModelKind, _path: &Path) -> Option<Box<dyn ModelSession>> {
        None
    }
}

/// Every window holds three seconds of speech.
struct SpeechRecorder;

impl EnrollRecorder for SpeechRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        Ok(())
    }

    fn stop(&self) -> Vec<f32> {
        vec![0.5_f32; 16_000 * 3]
    }
}

/// Every clip embeds to the same unit-like vector, so the clips agree.
struct FixedEmbedder;

impl EnrollEmbedder for FixedEmbedder {
    fn embed(&self, _buffer16k: &[f32], _cancel: &AtomicBool) -> Option<Vec<f32>> {
        Some(vec![1.0_f32; SPEAKER_SPEC.dim])
    }
}

/// Virtual clock: sleeping advances time without waiting.
struct VirtualClock;

impl EnrollClock for VirtualClock {
    fn now_ms(&self) -> u64 {
        0
    }

    fn sleep_ms(&self, _ms: u64) {}
}

struct Harness {
    _dir: tempfile::TempDir,
    gate: Arc<OwnershipGate>,
    rt: VoiceRuntime,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let gate = OwnershipGate::new();
    let store = Arc::new(VoiceprintStore::new(dir.path().join("voiceprint")));
    let enroll = Arc::new(EnrollmentController::new(
        Arc::clone(&gate),
        Arc::new(SpeechRecorder),
        Arc::new(FixedEmbedder),
        Arc::clone(&store),
        Arc::new(VirtualClock),
        EnrollConfig::default(),
    ));
    let service = Arc::new(VoiceModelService::new(Arc::new(NoModel)));
    let settings: SettingsGetter = Arc::new(get_default_settings);
    let rt = VoiceRuntime::new(service, store, enroll, settings);
    Harness {
        _dir: dir,
        gate,
        rt,
    }
}

fn install_speaker_model(rt: &VoiceRuntime) {
    rt.service
        .install(ModelKind::Speaker, PathBuf::from("/models/speaker.onnx"));
}

fn enroll_fully(rt: &VoiceRuntime) {
    let id = begin_in(rt).expect("begin");
    for step in 0..3 {
        let result = step_in(rt, id, step).expect("step");
        assert!(result.ok);
    }
    finish_in(rt, id).expect("finish");
}

#[test]
fn status_flags_follow_the_model_and_the_store() {
    let h = harness();
    assert_eq!(
        status_of(&h.rt),
        SpeakerStatus {
            enrolled: false,
            enrollment_active: false,
            model_ready: false,
        }
    );
    install_speaker_model(&h.rt);
    assert!(status_of(&h.rt).model_ready);
}

#[test]
fn begin_without_the_model_is_model_missing_and_takes_no_lease() {
    let h = harness();
    assert_eq!(begin_in(&h.rt), Err(SpeakerCmdError::ModelMissing));
    assert!(!status_of(&h.rt).enrollment_active);
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn begin_step_finish_stores_a_voiceprint() {
    let h = harness();
    install_speaker_model(&h.rt);
    enroll_fully(&h.rt);
    let status = status_of(&h.rt);
    assert!(status.enrolled);
    assert!(!status.enrollment_active);
}

#[test]
fn begin_while_dictation_owns_the_mic_is_busy() {
    let h = harness();
    install_speaker_model(&h.rt);
    let _dictation = h
        .gate
        .try_acquire(Owner::Dictation)
        .expect("dictation lease");
    assert_eq!(
        begin_in(&h.rt),
        Err(SpeakerCmdError::Enroll(EnrollError::Busy))
    );
}

#[test]
fn delete_after_enrolling_clears_the_voiceprint_and_is_idempotent() {
    let h = harness();
    install_speaker_model(&h.rt);
    enroll_fully(&h.rt);
    assert!(status_of(&h.rt).enrolled);
    assert_eq!(delete_in(&h.rt), Ok(()));
    assert!(!status_of(&h.rt).enrolled);
    assert_eq!(delete_in(&h.rt), Ok(()));
}

#[test]
fn delete_with_nothing_enrolled_succeeds() {
    let h = harness();
    assert_eq!(delete_in(&h.rt), Ok(()));
}

#[test]
fn a_stale_session_id_is_refused_by_step() {
    let h = harness();
    install_speaker_model(&h.rt);
    let _live = begin_in(&h.rt).expect("begin");
    assert_eq!(
        step_in(&h.rt, 999_999, 0),
        Err(SpeakerCmdError::Enroll(EnrollError::StaleSession))
    );
}

#[test]
fn cancel_drops_the_session_and_its_lease() {
    let h = harness();
    install_speaker_model(&h.rt);
    let id = begin_in(&h.rt).expect("begin");
    assert!(status_of(&h.rt).enrollment_active);
    assert_eq!(cancel_in(&h.rt, id), Ok(()));
    assert!(!status_of(&h.rt).enrollment_active);
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn status_serializes_to_bools_only() {
    let value = serde_json::to_value(status_of(&harness().rt)).expect("json");
    let object = value.as_object().expect("object");
    assert_eq!(object.len(), 3);
    for field in object.values() {
        assert!(field.is_boolean(), "non-bool field: {field}");
    }
}

#[test]
fn errors_serialize_to_enum_tags_only() {
    const TAGS: [&str; 13] = [
        "ModelMissing",
        "StoreFailed",
        "Enroll",
        "Busy",
        "StaleSession",
        "NoActiveSession",
        "BadStep",
        "TooLittleSpeech",
        "EmbedFailed",
        "NotEnoughSteps",
        "Inconsistent",
        "UnsupportedPlatform",
        "Cancelled",
    ];
    let cases = [
        SpeakerCmdError::ModelMissing,
        SpeakerCmdError::StoreFailed,
        SpeakerCmdError::Enroll(EnrollError::Busy),
        SpeakerCmdError::Enroll(EnrollError::StaleSession),
        SpeakerCmdError::Enroll(EnrollError::TooLittleSpeech),
    ];
    for case in cases {
        let text = serde_json::to_string(&case).expect("json");
        for word in text.split(|c: char| !c.is_alphanumeric()) {
            assert!(
                word.is_empty() || TAGS.contains(&word),
                "unexpected text in {text}"
            );
        }
    }
    let step = serde_json::to_value(StepResult {
        speech_ms: 3000,
        ok: true,
    })
    .expect("json");
    assert!(step["speech_ms"].is_number());
    assert!(step["ok"].is_boolean());
}
