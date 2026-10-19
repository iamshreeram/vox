//! Tauri commands for the optional model downloads (design D2). Thin glue over
//! `managers::aux_model::AuxModelManager`. Nothing here starts a download on its own:
//! `aux_model_download` runs only when the user presses Download.

use crate::managers::aux_model::{
    aux_spec, production_policy, AuxError, AuxKind, AuxModelManager, AuxProgress, AuxState,
};
use serde::Serialize;
use specta::Type;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

/// Tauri event: `AuxProgressEvent` payload, at most one per `PROGRESS_INTERVAL_MS`.
pub const AUX_PROGRESS_EVENT: &str = "aux-model-progress";
/// Tauri event: `AuxFinishedEvent` payload, once per accepted download.
pub const AUX_FINISHED_EVENT: &str = "aux-model-finished";
/// Minimum gap between two progress events. The completed sample is always sent.
pub const PROGRESS_INTERVAL_MS: u64 = 100;

/// The two optional model managers, managed as one state value because Tauri
/// state is keyed by type (two bare `Arc<AuxModelManager>` would collide).
pub struct AuxModels {
    smart_turn: Arc<AuxModelManager>,
    speaker_resnet34: Arc<AuxModelManager>,
}

impl AuxModels {
    /// Both kinds install under `root/<kind>/`. Nothing is downloaded or loaded here.
    pub fn new(root: PathBuf) -> Self {
        Self {
            smart_turn: Arc::new(AuxModelManager::new(
                root.clone(),
                aux_spec(AuxKind::SmartTurn),
                production_policy(),
            )),
            speaker_resnet34: Arc::new(AuxModelManager::new(
                root,
                aux_spec(AuxKind::SpeakerResnet34),
                production_policy(),
            )),
        }
    }

    pub fn for_kind(&self, kind: AuxKind) -> &Arc<AuxModelManager> {
        match kind {
            AuxKind::SmartTurn => &self.smart_turn,
            AuxKind::SpeakerResnet34 => &self.speaker_resnet34,
        }
    }
}

/// Answer of `aux_model_status`: numbers and enums only.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
pub struct AuxStatus {
    pub kind: AuxKind,
    pub state: AuxState,
    pub size_bytes: u64,
}

/// Payload of `aux-model-progress`: numbers and the kind only.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
pub struct AuxProgressEvent {
    pub kind: AuxKind,
    pub downloaded: u64,
    pub total: u64,
}

/// Payload of `aux-model-finished`: the state after the attempt and the reason, if any.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
pub struct AuxFinishedEvent {
    pub kind: AuxKind,
    pub state: AuxState,
    pub error: Option<AuxError>,
}

/// Status for `kind` in `state`. The size comes from the pinned spec.
pub fn build_status(kind: AuxKind, state: AuxState) -> AuxStatus {
    AuxStatus {
        kind,
        state,
        size_bytes: aux_spec(kind).size,
    }
}

/// Admission check for a new download: `Busy` while another operation holds the slot.
pub fn download_admission(state: AuxState) -> Result<(), AuxError> {
    match state {
        AuxState::Downloading | AuxState::Verifying => Err(AuxError::Busy),
        AuxState::NotInstalled | AuxState::Ready | AuxState::Error => Ok(()),
    }
}

pub fn progress_event(progress: AuxProgress) -> AuxProgressEvent {
    AuxProgressEvent {
        kind: progress.kind,
        downloaded: progress.downloaded,
        total: progress.total,
    }
}

pub fn finished_event(
    kind: AuxKind,
    state: AuxState,
    result: Result<(), AuxError>,
) -> AuxFinishedEvent {
    AuxFinishedEvent {
        kind,
        state,
        error: result.err(),
    }
}

/// Keeps progress events at least `PROGRESS_INTERVAL_MS` apart on a monotonic clock
/// (milliseconds). A completed sample (`downloaded >= total`, `total > 0`) is always
/// accepted so the UI never misses the end.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last_ms: Option<u64>,
}

impl ProgressThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when this sample should be emitted.
    pub fn accept(&mut self, now_ms: u64, downloaded: u64, total: u64) -> bool {
        let completed = total > 0 && downloaded >= total;
        let due = match self.last_ms {
            None => true,
            Some(last) => now_ms.saturating_sub(last) >= PROGRESS_INTERVAL_MS,
        };
        if completed || due {
            self.last_ms = Some(now_ms);
            true
        } else {
            false
        }
    }
}

fn models(app: &AppHandle) -> Result<Arc<AuxModels>, AuxError> {
    // Missing state means the app data dir could not be resolved at startup.
    app.try_state::<Arc<AuxModels>>()
        .map(|state| state.inner().clone())
        .ok_or(AuxError::Io)
}

#[tauri::command]
#[specta::specta]
pub async fn aux_model_status(app: AppHandle, kind: AuxKind) -> AuxStatus {
    let state = match models(&app) {
        Ok(models) => models.for_kind(kind).state(),
        Err(_) => AuxState::NotInstalled,
    };
    build_status(kind, state)
}

/// Starts the download in the background and returns once it is accepted.
/// Progress and the result arrive as `aux-model-progress` and `aux-model-finished`.
#[tauri::command]
#[specta::specta]
pub async fn aux_model_download(app: AppHandle, kind: AuxKind) -> Result<(), AuxError> {
    let models = models(&app)?;
    let manager = Arc::clone(models.for_kind(kind));
    download_admission(manager.state())?;
    let progress_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let throttle = Mutex::new(ProgressThrottle::new());
        let started = Instant::now();
        let on_progress = move |progress: AuxProgress| {
            let now_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let due = throttle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .accept(now_ms, progress.downloaded, progress.total);
            if due {
                let _ = progress_app.emit(AUX_PROGRESS_EVENT, progress_event(progress));
            }
        };
        let result = manager.download(&on_progress).await;
        let finished = finished_event(kind, manager.state(), result);
        let _ = app.emit(AUX_FINISHED_EVENT, finished);
    });
    Ok(())
}

/// Cancels the running download of `kind`. A no-op when none is running.
#[tauri::command]
#[specta::specta]
pub fn aux_model_cancel(app: AppHandle, kind: AuxKind) -> Result<(), AuxError> {
    models(&app)?.for_kind(kind).cancel();
    Ok(())
}

/// Removes the installed `kind`. Idempotent. `Busy` while an operation runs.
#[tauri::command]
#[specta::specta]
pub fn aux_model_delete(app: AppHandle, kind: AuxKind) -> Result<(), AuxError> {
    models(&app)?.for_kind(kind).delete()
}

#[cfg(test)]
#[path = "aux_model_cmd_tests.rs"]
mod tests;
