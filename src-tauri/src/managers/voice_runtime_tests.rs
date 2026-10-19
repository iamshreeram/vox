//! Pinned by S12: production wiring of the optional voice features (`voice_runtime.rs`).
//! Fakes only: no real model, no microphone, no network.

use super::*;
use crate::managers::aux_model::AuxState;
use crate::managers::ownership::Owner;
use crate::managers::speaker_enroll::EnrollConfig;
use crate::managers::speaker_enroll::{EnrollClock, EnrollEmbedder, EnrollRecorder, RecorderBusy};
use crate::managers::speaker_id::{self, SPEAKER_SPEC};
use crate::managers::voice_models::{ModelSession, SessionLoader};
use crate::settings::{get_default_settings, SpeakerSensitivity};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

struct NoModel;

impl SessionLoader for NoModel {
    fn load(&self, _kind: ModelKind, _path: &Path) -> Option<Box<dyn ModelSession>> {
        None
    }
}

struct IdleRecorder;

impl EnrollRecorder for IdleRecorder {
    fn start(&self) -> Result<(), RecorderBusy> {
        Ok(())
    }

    fn stop(&self) -> Vec<f32> {
        Vec::new()
    }
}

struct NoEmbedder;

impl EnrollEmbedder for NoEmbedder {
    fn embed(&self, _buffer16k: &[f32], _cancel: &AtomicBool) -> Option<Vec<f32>> {
        None
    }
}

struct ZeroClock;

impl EnrollClock for ZeroClock {
    fn now_ms(&self) -> u64 {
        0
    }

    fn sleep_ms(&self, _ms: u64) {}
}

/// A runtime over fakes, with a settings cell the test can change after construction.
struct Harness {
    _dir: tempfile::TempDir,
    gate: Arc<OwnershipGate>,
    cell: Arc<Mutex<AppSettings>>,
    rt: VoiceRuntime,
}

fn harness(settings: AppSettings) -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let gate = OwnershipGate::new();
    let cell = Arc::new(Mutex::new(settings));
    let getter_cell = Arc::clone(&cell);
    let getter: SettingsGetter = Arc::new(move || {
        getter_cell
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    });
    let service = Arc::new(VoiceModelService::new(Arc::new(NoModel)));
    let store = Arc::new(VoiceprintStore::new(dir.path().join("voiceprint")));
    let enroll = Arc::new(EnrollmentController::new(
        Arc::clone(&gate),
        Arc::new(IdleRecorder),
        Arc::new(NoEmbedder),
        Arc::clone(&store),
        Arc::new(ZeroClock),
        EnrollConfig::default(),
    ));
    let rt = VoiceRuntime::new(service, store, enroll, getter);
    Harness {
        _dir: dir,
        gate,
        cell,
        rt,
    }
}

fn set_cell(cell: &Mutex<AppSettings>, change: impl FnOnce(&mut AppSettings)) {
    let mut guard = cell.lock().expect("settings cell");
    change(&mut guard);
}

fn with_flags(smart_turn: bool, speaker_filter: bool) -> AppSettings {
    let mut s = get_default_settings();
    s.converse_smart_turn = smart_turn;
    s.speaker_filter_enabled = speaker_filter;
    s
}

fn install_model(rt: &VoiceRuntime, kind: ModelKind) {
    rt.service
        .install(kind, PathBuf::from("/models/fake-model.onnx"));
}

fn write_enrolled_voiceprint(store: &VoiceprintStore) {
    let clips = vec![vec![1.0_f32; SPEAKER_SPEC.dim]; 3];
    let vp = speaker_id::enroll(&clips, SPEAKER_SPEC.sha256).expect("enroll fixture");
    store.save(&vp).expect("save fixture");
}

#[test]
fn smart_turn_detector_only_when_enabled_and_installed() {
    for smart_turn in [false, true] {
        for installed in [false, true] {
            let settings = with_flags(smart_turn, false);
            let h = harness(settings.clone());
            if installed {
                install_model(&h.rt, ModelKind::SmartTurn);
            }
            let opts = capture_options(&settings, &h.rt);
            assert_eq!(
                opts.turn_detector.is_some(),
                smart_turn && installed,
                "smart_turn={smart_turn} installed={installed}"
            );
            assert!(opts.speaker_gate.is_none());
        }
    }
}

#[test]
fn speaker_gate_only_when_enabled_installed_and_enrolled() {
    for enabled in [false, true] {
        for installed in [false, true] {
            for enrolled in [false, true] {
                let settings = with_flags(false, enabled);
                let h = harness(settings.clone());
                if installed {
                    install_model(&h.rt, ModelKind::Speaker);
                }
                if enrolled {
                    write_enrolled_voiceprint(&h.rt.store);
                }
                let opts = capture_options(&settings, &h.rt);
                assert_eq!(
                    opts.speaker_gate.is_some(),
                    enabled && installed && enrolled,
                    "enabled={enabled} installed={installed} enrolled={enrolled}"
                );
                assert!(opts.turn_detector.is_none());
            }
        }
    }
}

#[test]
fn sensitivity_reader_sees_a_settings_change_made_after_construction() {
    let h = harness(with_flags(false, true));
    install_model(&h.rt, ModelKind::Speaker);
    write_enrolled_voiceprint(&h.rt.store);
    let settings = with_flags(false, true);
    assert!(capture_options(&settings, &h.rt).speaker_gate.is_some());

    let reader = sensitivity_reader(&h.rt);
    assert_eq!(reader(), SpeakerSensitivity::Balanced);
    set_cell(&h.cell, |s| {
        s.speaker_filter_sensitivity = SpeakerSensitivity::Strict;
    });
    assert_eq!(reader(), SpeakerSensitivity::Strict);
}

#[test]
fn everything_off_gives_neither_optional_part() {
    let h = harness(get_default_settings());
    install_model(&h.rt, ModelKind::SmartTurn);
    install_model(&h.rt, ModelKind::Speaker);
    write_enrolled_voiceprint(&h.rt.store);
    let opts = capture_options(&get_default_settings(), &h.rt);
    assert!(opts.turn_detector.is_none());
    assert!(opts.speaker_gate.is_none());
}

#[test]
fn startup_sync_installs_a_ready_kind() {
    let h = harness(get_default_settings());
    apply_kind(
        &h.rt,
        AuxKind::SmartTurn,
        AuxState::Ready,
        Some(PathBuf::from("/models/smart.onnx")),
    );
    apply_kind(
        &h.rt,
        AuxKind::SpeakerResnet34,
        AuxState::NotInstalled,
        None,
    );
    assert!(h.rt.service.is_installed(ModelKind::SmartTurn));
    assert!(!h.rt.service.is_installed(ModelKind::Speaker));
}

#[test]
fn startup_sync_installs_nothing_while_downloading() {
    let h = harness(get_default_settings());
    apply_kind(
        &h.rt,
        AuxKind::SpeakerResnet34,
        AuxState::Downloading,
        Some(PathBuf::from("/models/partial.onnx")),
    );
    apply_kind(&h.rt, AuxKind::SmartTurn, AuxState::Downloading, None);
    assert!(!h.rt.service.is_installed(ModelKind::Speaker));
    assert!(!h.rt.service.is_installed(ModelKind::SmartTurn));
}

#[test]
fn sync_after_delete_uninstalls() {
    let h = harness(get_default_settings());
    apply_kind(
        &h.rt,
        AuxKind::SmartTurn,
        AuxState::Ready,
        Some(PathBuf::from("/models/smart.onnx")),
    );
    assert!(h.rt.service.is_installed(ModelKind::SmartTurn));
    apply_kind(&h.rt, AuxKind::SmartTurn, AuxState::NotInstalled, None);
    assert!(!h.rt.service.is_installed(ModelKind::SmartTurn));
}

#[test]
fn installing_a_kind_twice_is_harmless() {
    let h = harness(get_default_settings());
    let path = PathBuf::from("/models/smart.onnx");
    apply_kind(
        &h.rt,
        AuxKind::SmartTurn,
        AuxState::Ready,
        Some(path.clone()),
    );
    apply_kind(&h.rt, AuxKind::SmartTurn, AuxState::Ready, Some(path));
    assert!(h.rt.service.is_installed(ModelKind::SmartTurn));
    apply_kind(&h.rt, AuxKind::SmartTurn, AuxState::NotInstalled, None);
    assert!(!h.rt.service.is_installed(ModelKind::SmartTurn));
}

#[test]
fn deleting_the_speaker_model_cancels_a_running_enrollment() {
    let h = harness(get_default_settings());
    apply_kind(
        &h.rt,
        AuxKind::SpeakerResnet34,
        AuxState::Ready,
        Some(PathBuf::from("/models/speaker.onnx")),
    );
    let id = h.rt.enroll.begin().expect("begin");
    h.rt.remember_session(id);
    assert!(h.rt.enroll.has_session());

    apply_kind(
        &h.rt,
        AuxKind::SpeakerResnet34,
        AuxState::NotInstalled,
        None,
    );

    assert!(!h.rt.enroll.has_session());
    assert!(h.gate.try_acquire(Owner::Dictation).is_ok());
}

#[test]
fn kinds_map_to_the_matching_service_models() {
    assert_eq!(kind_to_model(AuxKind::SmartTurn), ModelKind::SmartTurn);
    assert_eq!(kind_to_model(AuxKind::SpeakerResnet34), ModelKind::Speaker);
}

#[cfg(unix)]
#[test]
fn private_voiceprint_dir_is_created_with_mode_0700() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().expect("temp dir");
    let dir = base.path().join("voiceprint");
    ensure_private_dir(&dir).expect("create dir");
    let mode = std::fs::metadata(&dir)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

/// Identifiers whose values must never reach a log call (audio, embeddings, speech text).
const PRIVATE_NAMES: [&str; 6] = [
    "buffer",
    "samples",
    "embedding",
    "voiceprint",
    "transcript",
    "text",
];

/// Logging calls in `source`, each cut at its closing `);` (or the end of the file).
fn logging_calls(source: &str) -> Vec<&str> {
    const MACROS: [&str; 6] = ["log::", "info!(", "debug!(", "warn!(", "error!(", "trace!("];
    let mut calls = Vec::new();
    for pattern in MACROS {
        for (start, _) in source.match_indices(pattern) {
            let rest = &source[start..];
            let end = rest.find(");").map_or(rest.len(), |i| i + 2);
            calls.push(&rest[..end]);
        }
    }
    calls
}

/// True when a logging call passes one of the private identifiers as an argument. Text inside
/// string literals (the message itself) is not an argument and is ignored.
fn mentions_private_name(call: &str) -> bool {
    let mut code = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for c in call.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
                code.push(' ');
            }
        } else if c == '"' {
            in_string = true;
        } else {
            code.push(c);
        }
    }
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|token| PRIVATE_NAMES.contains(&token))
}

#[test]
fn the_privacy_check_flags_a_private_argument_but_not_a_message() {
    assert!(mentions_private_name(
        "log::warn!(\"count {}\", buffer.len());"
    ));
    assert!(mentions_private_name("log::debug!(\"{:?}\", voiceprint);"));
    assert!(!mentions_private_name(
        "log::warn!(\"voiceprint directory unavailable\");"
    ));
}

#[test]
fn new_voice_sources_never_print_or_log_private_values() {
    let sources = [
        (
            "managers/voice_runtime.rs",
            include_str!("voice_runtime.rs"),
        ),
        (
            "commands/speaker.rs",
            include_str!("../commands/speaker.rs"),
        ),
    ];
    for (name, text) in sources {
        // Production code only: everything from the first test module on is excluded.
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        for banned in ["println!", "eprintln!", "print!(", "dbg!("] {
            assert!(!production.contains(banned), "{name} contains {banned}");
        }
        for call in logging_calls(production) {
            assert!(
                !mentions_private_name(call),
                "{name}: logging call mentions a private value: {call}"
            );
        }
    }
}
