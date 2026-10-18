//! Natural voice: data shapes and pure mappers for the settings UI (model status, voice list,
//! user-safe error messages). The Tauri command functions and app wiring live in this module's
//! second half (added in step S5c); everything here is deterministic and unit tested.
#![allow(dead_code)]

use crate::managers::natural_engine::{FailureReason, HealthState};
use crate::managers::natural_model::{DownloadProgress, ModelError, ModelState};
use crate::managers::natural_voices::NATURAL_VOICES;
use serde::Serialize;
use specta::Type;

/// Everything the Spoken Replies settings row needs in one call.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct NaturalVoiceStatus {
    /// False off macOS: the section is hidden and downloads are refused.
    pub supported: bool,
    /// `not_installed` | `downloading` | `verifying` | `installing` | `deleting` | `loading` | `ready`
    pub model: String,
    pub downloaded: u64,
    pub total: u64,
    /// `ready` | `failed`
    pub health: String,
    /// When `health == "failed"`: `load` | `synth` | `sink` | `worker_exit`.
    pub failure: Option<String>,
}

/// One entry of the natural voice picker.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct NaturalVoiceOption {
    pub id: String,
    pub name: String,
    /// `American` | `British`
    pub accent: String,
    /// `Female` | `Male`
    pub gender: String,
}

pub fn model_state_name(state: ModelState) -> &'static str {
    match state {
        ModelState::NotInstalled => "not_installed",
        ModelState::Downloading => "downloading",
        ModelState::Verifying => "verifying",
        ModelState::Installing => "installing",
        ModelState::Deleting => "deleting",
        ModelState::Loading => "loading",
        ModelState::Ready => "ready",
    }
}

pub fn failure_name(reason: FailureReason) -> &'static str {
    match reason {
        FailureReason::Load => "load",
        FailureReason::Synth => "synth",
        FailureReason::Sink => "sink",
        FailureReason::WorkerExit => "worker_exit",
    }
}

pub fn build_status(
    supported: bool,
    state: ModelState,
    progress: DownloadProgress,
    health: HealthState,
) -> NaturalVoiceStatus {
    let (health_name, failure) = match health {
        HealthState::Ready => ("ready", None),
        HealthState::Failed(reason) => ("failed", Some(failure_name(reason).to_string())),
    };
    NaturalVoiceStatus {
        supported,
        model: model_state_name(state).to_string(),
        downloaded: progress.downloaded,
        total: progress.total,
        health: health_name.to_string(),
        failure,
    }
}

pub fn voice_options() -> Vec<NaturalVoiceOption> {
    NATURAL_VOICES
        .iter()
        .map(|v| NaturalVoiceOption {
            id: v.id.to_string(),
            name: v.name.to_string(),
            accent: v.accent().to_string(),
            gender: v.gender().to_string(),
        })
        .collect()
}

/// A message safe to show in the UI: never contains URLs, paths or network text.
pub fn error_message(error: &ModelError) -> String {
    match error {
        ModelError::Busy => "Another model operation is already running.",
        ModelError::Cancelled => "The download was cancelled.",
        ModelError::NotInstalled => "The natural voice model is not installed.",
        ModelError::Corrupt(_) => {
            "The model files failed verification. Please download the model again."
        }
        ModelError::UrlNotAllowed => "The download address is not allowed.",
        ModelError::Download(_) => {
            "The download failed. Check your internet connection and try again."
        }
        ModelError::Io(_) => "The model files could not be written to disk.",
    }
    .to_string()
}

// ---------------------------------------------------------------------------------------------
// Tauri commands (S5c). Never log speech text, URLs or error bodies.
// ---------------------------------------------------------------------------------------------

use crate::managers::natural_engine::NaturalHealth;
use crate::managers::natural_model::ModelCoordinator;
use crate::managers::tts::TtsManager;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

const UNAVAILABLE: &str = "Natural voice is not available";
const PROGRESS_EVENT: &str = "natural-voice-download-progress";
const FINISHED_EVENT: &str = "natural-voice-download-finished";

/// Payload of `natural-voice-download-progress`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct NaturalVoiceDownloadProgress {
    pub downloaded: u64,
    pub total: u64,
}

/// Payload of `natural-voice-download-finished`. `message` is UI-safe (see `error_message`).
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct NaturalVoiceDownloadFinished {
    pub ok: bool,
    pub message: Option<String>,
}

fn coordinator(app: &AppHandle) -> Result<Arc<ModelCoordinator>, String> {
    app.try_state::<Arc<ModelCoordinator>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| UNAVAILABLE.to_string())
}

fn health(app: &AppHandle) -> Result<Arc<NaturalHealth>, String> {
    app.try_state::<Arc<NaturalHealth>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| UNAVAILABLE.to_string())
}

fn is_busy(state: ModelState) -> bool {
    !matches!(state, ModelState::NotInstalled | ModelState::Ready)
}

#[tauri::command]
#[specta::specta]
pub fn natural_voice_status(app: AppHandle) -> Result<NaturalVoiceStatus, String> {
    let coordinator = coordinator(&app)?;
    let health = health(&app)?;
    Ok(build_status(
        cfg!(target_os = "macos"),
        coordinator.state(),
        coordinator.progress(),
        health.state(),
    ))
}

/// Starts the model download in the background and returns immediately. Progress and the
/// result are reported through events.
#[tauri::command]
#[specta::specta]
pub async fn natural_voice_download(app: AppHandle) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(UNAVAILABLE.to_string());
    }
    let coordinator = coordinator(&app)?;
    let health = health(&app)?;
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    tauri::async_runtime::spawn(async move {
        let progress_app = app.clone();
        let on_progress = move |p: DownloadProgress| {
            let _ = progress_app.emit(
                PROGRESS_EVENT,
                NaturalVoiceDownloadProgress {
                    downloaded: p.downloaded,
                    total: p.total,
                },
            );
        };
        let result = coordinator.download(&on_progress).await;
        let payload = match &result {
            Ok(()) => NaturalVoiceDownloadFinished {
                ok: true,
                message: None,
            },
            Err(err) => NaturalVoiceDownloadFinished {
                ok: false,
                message: Some(error_message(err)),
            },
        };
        if result.is_ok() {
            health.bump();
        } else {
            log::warn!("natural voice: model download did not complete");
        }
        let _ = app.emit(FINISHED_EVENT, payload);
    });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn natural_voice_cancel_download(app: AppHandle) -> Result<(), String> {
    coordinator(&app)?.cancel();
    Ok(())
}

/// Stops speech, removes the model, then lets the natural voice re-evaluate its health.
#[tauri::command]
#[specta::specta]
pub fn natural_voice_delete(app: AppHandle) -> Result<(), String> {
    let coordinator = coordinator(&app)?;
    let health = health(&app)?;
    if let Some(tts) = app.try_state::<Arc<TtsManager>>() {
        let _ = tts.stop();
    }
    coordinator.delete().map_err(|e| error_message(&e))?;
    health.bump();
    Ok(())
}

/// Retry button: a health bump makes the next spoken reply recover the natural voice.
#[tauri::command]
#[specta::specta]
pub fn natural_voice_retry(app: AppHandle) -> Result<(), String> {
    health(&app)?.bump();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn list_natural_voices() -> Vec<NaturalVoiceOption> {
    voice_options()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_model_state_has_a_stable_snake_case_name() {
        let names: Vec<&str> = [
            ModelState::NotInstalled,
            ModelState::Downloading,
            ModelState::Verifying,
            ModelState::Installing,
            ModelState::Deleting,
            ModelState::Loading,
            ModelState::Ready,
        ]
        .into_iter()
        .map(model_state_name)
        .collect();
        assert_eq!(
            names,
            [
                "not_installed",
                "downloading",
                "verifying",
                "installing",
                "deleting",
                "loading",
                "ready"
            ]
        );
    }

    #[test]
    fn a_ready_status_has_no_failure_and_carries_progress() {
        let status = build_status(
            true,
            ModelState::Downloading,
            DownloadProgress {
                downloaded: 10,
                total: 40,
            },
            HealthState::Ready,
        );
        assert_eq!(
            status,
            NaturalVoiceStatus {
                supported: true,
                model: "downloading".into(),
                downloaded: 10,
                total: 40,
                health: "ready".into(),
                failure: None,
            }
        );
    }

    #[test]
    fn a_failed_status_names_the_reason() {
        for (reason, name) in [
            (FailureReason::Load, "load"),
            (FailureReason::Synth, "synth"),
            (FailureReason::Sink, "sink"),
            (FailureReason::WorkerExit, "worker_exit"),
        ] {
            let status = build_status(
                true,
                ModelState::Ready,
                DownloadProgress::default(),
                HealthState::Failed(reason),
            );
            assert_eq!(status.health, "failed");
            assert_eq!(status.failure.as_deref(), Some(name));
        }
    }

    #[test]
    fn unsupported_platforms_are_reported() {
        let status = build_status(
            false,
            ModelState::NotInstalled,
            DownloadProgress::default(),
            HealthState::Ready,
        );
        assert!(!status.supported);
        assert_eq!(status.model, "not_installed");
    }

    #[test]
    fn the_voice_list_has_all_curated_voices_with_labels() {
        let voices = voice_options();
        assert_eq!(voices.len(), 28);
        assert_eq!(
            voices[3],
            NaturalVoiceOption {
                id: "af_heart".into(),
                name: "Heart".into(),
                accent: "American".into(),
                gender: "Female".into(),
            }
        );
        assert!(voices
            .iter()
            .any(|v| v.id == "bm_george" && v.accent == "British" && v.gender == "Male"));
    }

    #[test]
    fn error_messages_never_leak_urls_paths_or_network_text() {
        let secret = "https://github.com/x/y?token=abc /Users/someone/secret";
        for error in [
            ModelError::Busy,
            ModelError::Cancelled,
            ModelError::NotInstalled,
            ModelError::Corrupt(secret.into()),
            ModelError::UrlNotAllowed,
            ModelError::Download(secret.into()),
            ModelError::Io(secret.into()),
        ] {
            let message = error_message(&error);
            assert!(!message.is_empty());
            for leaked in ["http", "github", "token", "/Users", "secret"] {
                assert!(
                    !message.contains(leaked),
                    "{error:?} leaks {leaked:?}: {message}"
                );
            }
        }
        assert!(
            error_message(&ModelError::Corrupt(String::new())).contains("download the model again")
        );
    }
}
