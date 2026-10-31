//! Phonon voice: the settings commands for the Phonon peer engine (DESIGN.md Rev 4, P4b).
//!
//! Mirrors `natural_voice` (Kokoro) with its own state: `PhononCoordinator` and `PhononHealth`
//! are managed on macOS only, so off macOS the status reports unsupported and not installed,
//! and every other command refuses. The platform-neutral cores below hold the logic and are
//! unit tested; the Tauri wrappers only look up state and spawn the install. Events:
//! `phonon-voice-state` (progress, OK finish, cancel) and `phonon-voice-error` (other failures).
//! Never log speech text, URLs or error bodies.

use crate::commands::natural_voice::{
    build_status, error_message, phonon_failure_message, NaturalVoiceDownloadFinished,
    NaturalVoiceDownloadProgress, NaturalVoiceStatus,
};
use crate::managers::natural_engine::{FailureReason, HealthState, NaturalHealth};
use crate::managers::natural_model::{DownloadProgress, ModelCoordinator, ModelError, ModelState};
use crate::managers::natural_tamil::{
    tamil_failure_message, tamil_voice_status_for, TamilVoiceStatus,
};
use crate::managers::tts::TtsManager;
use crate::settings::get_settings;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Event for progress ticks, the OK finish and a cancel (a state change, not an error).
pub const STATE_EVENT: &str = "phonon-voice-state";
/// Event for every other failure. Its payload is the same finished shape with `ok: false`.
pub const ERROR_EVENT: &str = "phonon-voice-error";

const UNAVAILABLE: &str = "Phonon voice is not available";

/// Event sink: `(event name, JSON payload)`.
pub type PhononEmit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Tauri state: the Phonon model coordinator (macOS only).
pub struct PhononCoordinator(pub Arc<ModelCoordinator>);

/// Tauri state: the Phonon engine health. Its own type, because `Arc<NaturalHealth>` is
/// already managed for Kokoro and Tauri state is keyed by type.
pub struct PhononHealth(pub Arc<NaturalHealth>);

/// Tauri state: the Tamil engine health. Its own type, like `PhononHealth`: the router's Tamil
/// route holds the same handle.
pub struct TamilHealth(pub Arc<NaturalHealth>);

/// Tauri state: the folder of the Tamil pack (`tts_models/pocket-tts-tamil-int4`).
pub struct TamilModelDir(pub PathBuf);

fn is_busy(state: ModelState) -> bool {
    !matches!(state, ModelState::NotInstalled | ModelState::Ready)
}

fn finished_payload(ok: bool, message: Option<String>) -> Value {
    serde_json::to_value(NaturalVoiceDownloadFinished { ok, message }).unwrap_or(Value::Null)
}

fn progress_payload(progress: DownloadProgress) -> Value {
    serde_json::to_value(NaturalVoiceDownloadProgress {
        downloaded: progress.downloaded,
        total: progress.total,
    })
    .unwrap_or(Value::Null)
}

/// Sink for the Phonon health (`NaturalHealth::set_event_sink`). Each applied failure emits one
/// `phonon-voice-error` whose payload is the finished shape with `ok: false` and a user-safe
/// message. Only Phonon failures reach this sink; Kokoro keeps its own sink.
pub fn phonon_failure_sink(emit: PhononEmit) -> Box<dyn Fn(FailureReason) + Send + Sync> {
    Box::new(move |reason| {
        let message = phonon_failure_message(reason);
        emit(ERROR_EVENT, finished_payload(false, Some(message)));
    })
}

/// Pure status for the Phonon row: model state and progress from the coordinator, health from
/// the Phonon health.
pub fn phonon_voice_status_core(
    supported: bool,
    coordinator: &ModelCoordinator,
    health: &NaturalHealth,
) -> NaturalVoiceStatus {
    build_status(
        supported,
        coordinator.state(),
        coordinator.progress(),
        health.state(),
    )
}

/// Installs the Phonon model and reports through `emit`. A busy coordinator is refused with
/// nothing emitted. A success bumps the Phonon health once. A cancel is reported on the state
/// event; every other failure on the error event.
pub async fn phonon_voice_install_core(
    coordinator: Arc<ModelCoordinator>,
    health: Arc<NaturalHealth>,
    emit: PhononEmit,
) -> Result<(), String> {
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let progress_emit = emit.clone();
    let on_progress = move |progress: DownloadProgress| {
        progress_emit(STATE_EVENT, progress_payload(progress));
    };
    match coordinator.download(&on_progress).await {
        Ok(()) => {
            health.bump();
            emit(STATE_EVENT, finished_payload(true, None));
            Ok(())
        }
        Err(error) => {
            let message = error_message(&error);
            log::warn!("phonon voice: model download did not complete");
            let event = if matches!(error, ModelError::Cancelled) {
                STATE_EVENT
            } else {
                ERROR_EVENT
            };
            emit(event, finished_payload(false, Some(message.clone())));
            Err(message)
        }
    }
}

/// Installs the Phonon model from a local folder and reports through `emit`. A busy coordinator
/// is refused with nothing emitted. A success emits the finished OK state and bumps the Phonon
/// health once. A failure is reported on the error event with the user text. The source path is
/// never put in a payload or a log line.
pub fn phonon_voice_import_core(
    coordinator: Arc<ModelCoordinator>,
    health: Arc<NaturalHealth>,
    path: &Path,
    emit: PhononEmit,
) -> Result<(), String> {
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    match coordinator.import_folder(path) {
        Ok(()) => {
            health.bump();
            emit(STATE_EVENT, finished_payload(true, None));
            Ok(())
        }
        Err(error) => {
            let message = error_message(&error);
            log::warn!("phonon voice: folder import did not complete");
            emit(ERROR_EVENT, finished_payload(false, Some(message.clone())));
            Err(message)
        }
    }
}

/// Cancels a running Phonon download. Always Ok.
pub fn phonon_voice_cancel_core(coordinator: &ModelCoordinator) -> Result<(), String> {
    coordinator.cancel();
    Ok(())
}

/// Stops speech, removes the Phonon model, then bumps the Phonon health once.
pub fn phonon_voice_delete_core(
    coordinator: &ModelCoordinator,
    health: &NaturalHealth,
    stop_speech: &dyn Fn(),
) -> Result<(), String> {
    stop_speech();
    coordinator.delete().map_err(|e| error_message(&e))?;
    health.bump();
    Ok(())
}

fn coordinator(app: &AppHandle) -> Option<Arc<ModelCoordinator>> {
    app.try_state::<PhononCoordinator>()
        .map(|state| state.inner().0.clone())
}

fn health(app: &AppHandle) -> Option<Arc<NaturalHealth>> {
    app.try_state::<PhononHealth>()
        .map(|state| state.inner().0.clone())
}

#[tauri::command]
#[specta::specta]
pub fn phonon_voice_status(app: AppHandle) -> Result<NaturalVoiceStatus, String> {
    let supported = cfg!(target_os = "macos");
    match (coordinator(&app), health(&app)) {
        (Some(coordinator), Some(health)) => {
            Ok(phonon_voice_status_core(supported, &coordinator, &health))
        }
        _ => Ok(build_status(
            false,
            ModelState::NotInstalled,
            DownloadProgress::default(),
            HealthState::Ready,
        )),
    }
}

/// Starts the Phonon download in the background and returns at once. Progress and the result
/// arrive as events.
#[tauri::command]
#[specta::specta]
pub async fn phonon_voice_install(app: AppHandle) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(UNAVAILABLE.to_string());
    }
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let emit_app = app.clone();
    let emit: PhononEmit = Arc::new(move |name: &str, payload: Value| {
        let _ = emit_app.emit(name, payload);
    });
    tauri::async_runtime::spawn(async move {
        let _ = phonon_voice_install_core(coordinator, health, emit).await;
    });
    Ok(())
}

/// Starts the Phonon import from `path` (a folder picked by the user) on a blocking task and
/// returns at once. The result arrives as events, like the install.
#[tauri::command]
#[specta::specta]
pub fn phonon_voice_import_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(UNAVAILABLE.to_string());
    }
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let emit_app = app.clone();
    let emit: PhononEmit = Arc::new(move |name: &str, payload: Value| {
        let _ = emit_app.emit(name, payload);
    });
    let source = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        let _ = phonon_voice_import_core(coordinator, health, &source, emit);
    });
    Ok(())
}

/// Status of the Tamil voice for the settings row. The pack is checked by stat only, the engine
/// health comes from managed state, and `enabled` is the live setting. Without managed state the
/// status reports the pack as not installed.
#[tauri::command]
#[specta::specta]
pub fn tamil_voice_status(app: AppHandle) -> TamilVoiceStatus {
    let dir = app
        .try_state::<TamilModelDir>()
        .map(|state| state.inner().0.clone())
        .unwrap_or_default();
    let engine_error =
        app.try_state::<TamilHealth>()
            .and_then(|state| match state.inner().0.state() {
                HealthState::Ready => None,
                HealthState::Failed(reason) => Some(tamil_failure_message(reason)),
            });
    let enabled = get_settings(&app).tts_tamil_enabled;
    tamil_voice_status_for(&dir, enabled, engine_error)
}

#[tauri::command]
#[specta::specta]
pub fn phonon_voice_cancel(app: AppHandle) -> Result<(), String> {
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    phonon_voice_cancel_core(&coordinator)
}

/// Stops speech (through the shared TTS manager), removes the Phonon model and bumps health.
#[tauri::command]
#[specta::specta]
pub fn phonon_voice_delete(app: AppHandle) -> Result<(), String> {
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let stop_speech = || {
        if let Some(tts) = app.try_state::<Arc<TtsManager>>() {
            let _ = tts.stop();
        }
    };
    phonon_voice_delete_core(&coordinator, &health, &stop_speech)
}
