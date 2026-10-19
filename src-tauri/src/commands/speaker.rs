//! Tauri commands for the speaker filter and enrollment (design D6, A3'). Thin wrappers over the
//! pure `*_in` functions, which are pinned by `speaker_cmd_tests.rs`.
//!
//! Logs in this file carry numbers and fixed messages only: never audio, embeddings or text.

use std::sync::Arc;

use crate::managers::speaker_enroll::{EnrollError, StepResult};
use crate::managers::voice_models::ModelKind;
use crate::managers::voice_runtime::VoiceRuntime;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};

/// Numbers and booleans only.
#[derive(Serialize, Type, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeakerStatus {
    pub enrolled: bool,
    pub enrollment_active: bool,
    pub model_ready: bool,
}

/// Command failures. Serialized as enum tags only.
#[derive(Serialize, Type, Debug, Clone, PartialEq, Eq)]
pub enum SpeakerCmdError {
    /// The Speaker model is not installed, so no enrollment can start.
    ModelMissing,
    Enroll(EnrollError),
    /// The voiceprint store failed, or the runtime is not available.
    StoreFailed,
}

/// Flags for the Experimental page. Reads the store and the controller; takes no lease.
pub fn status_of(rt: &VoiceRuntime) -> SpeakerStatus {
    SpeakerStatus {
        enrolled: rt.store.is_enrolled(),
        enrollment_active: rt.enroll.has_session(),
        model_ready: rt.service.is_installed(ModelKind::Speaker),
    }
}

/// Starts an enrollment. `ModelMissing` is checked before any lease is taken.
pub fn begin_in(rt: &VoiceRuntime) -> Result<u64, SpeakerCmdError> {
    if !rt.service.is_installed(ModelKind::Speaker) {
        return Err(SpeakerCmdError::ModelMissing);
    }
    let id = rt.enroll.begin().map_err(SpeakerCmdError::Enroll)?;
    rt.remember_session(id);
    Ok(id)
}

/// Records one window of the enrollment.
pub fn step_in(
    rt: &VoiceRuntime,
    session_id: u64,
    step: u8,
) -> Result<StepResult, SpeakerCmdError> {
    rt.enroll
        .step(session_id, step)
        .map_err(SpeakerCmdError::Enroll)
}

pub fn cancel_in(rt: &VoiceRuntime, session_id: u64) -> Result<(), SpeakerCmdError> {
    rt.enroll
        .cancel(session_id)
        .map_err(SpeakerCmdError::Enroll)?;
    rt.forget_session(session_id);
    Ok(())
}

pub fn finish_in(rt: &VoiceRuntime, session_id: u64) -> Result<(), SpeakerCmdError> {
    rt.enroll
        .finish(session_id)
        .map_err(SpeakerCmdError::Enroll)?;
    rt.forget_session(session_id);
    Ok(())
}

/// Removes the voiceprint. Succeeds when nothing is enrolled. Does not touch an enrollment.
pub fn delete_in(rt: &VoiceRuntime) -> Result<(), SpeakerCmdError> {
    rt.store.delete().map_err(|_| SpeakerCmdError::StoreFailed)
}

fn runtime(app: &AppHandle) -> Result<Arc<VoiceRuntime>, SpeakerCmdError> {
    app.try_state::<Arc<VoiceRuntime>>()
        .map(|state| state.inner().clone())
        .ok_or(SpeakerCmdError::StoreFailed)
}

#[tauri::command]
#[specta::specta]
pub fn speaker_status(app: AppHandle) -> SpeakerStatus {
    match runtime(&app) {
        Ok(rt) => status_of(&rt),
        Err(_) => SpeakerStatus {
            enrolled: false,
            enrollment_active: false,
            model_ready: false,
        },
    }
}

#[tauri::command]
#[specta::specta]
pub fn speaker_enroll_begin(app: AppHandle) -> Result<u64, SpeakerCmdError> {
    let rt = runtime(&app)?;
    begin_in(&rt)
}

/// Blocking window work runs on the blocking pool. A panicked step is reported as `Cancelled`.
#[tauri::command]
#[specta::specta]
pub async fn speaker_enroll_step(
    app: AppHandle,
    session_id: u64,
    step: u8,
) -> Result<StepResult, SpeakerCmdError> {
    let rt = runtime(&app)?;
    tokio::task::spawn_blocking(move || step_in(&rt, session_id, step))
        .await
        .unwrap_or(Err(SpeakerCmdError::Enroll(EnrollError::Cancelled)))
}

#[tauri::command]
#[specta::specta]
pub fn speaker_enroll_cancel(app: AppHandle, session_id: u64) -> Result<(), SpeakerCmdError> {
    let rt = runtime(&app)?;
    cancel_in(&rt, session_id)
}

#[tauri::command]
#[specta::specta]
pub fn speaker_enroll_finish(app: AppHandle, session_id: u64) -> Result<(), SpeakerCmdError> {
    let rt = runtime(&app)?;
    finish_in(&rt, session_id)
}

#[tauri::command]
#[specta::specta]
pub fn speaker_delete_voiceprint(app: AppHandle) -> Result<(), SpeakerCmdError> {
    let rt = runtime(&app)?;
    delete_in(&rt)
}

#[cfg(test)]
#[path = "speaker_cmd_tests.rs"]
mod tests;
