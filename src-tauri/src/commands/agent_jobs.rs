//! Tauri commands for the confirmation and coding job settings (M3, DESIGN Rev 5 D1, D6, R2-8).
//!
//! The three setting commands are real: each is a read-modify-write of one field, modelled on
//! `agent_tools.rs`. The status and cancel commands read the shared job manager.

use crate::job_runtime::JobWiring;
use crate::managers::job_speaker;
use crate::settings::{self, AppSettings};
use tauri::{AppHandle, State};

/// Read-only status of the coding job, for the Settings status card and the tray.
///
/// `state` is one of `idle`, `running`, `succeeded`, `failed`, `cancelled`, `timed_out` or
/// `spawn_failed`. The DTO never carries job output.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, specta::Type)]
pub struct AgentJobStatusDto {
    /// Current job state name.
    pub state: String,
    /// Project the job runs in, when a job exists.
    pub project: Option<String>,
    /// Whole seconds since the job started (0 when idle).
    pub elapsed_secs: u64,
}

/// Read-modify-write of one boolean field, then persist.
fn persist_bool(app: &AppHandle, apply: impl FnOnce(&mut AppSettings, bool), value: bool) {
    let mut current = settings::get_settings(app);
    apply(&mut current, value);
    settings::write_settings(app, current);
}

/// Turns the confirmation prompt before model-planned actions on or off. On by default.
#[tauri::command]
#[specta::specta]
pub fn change_agent_confirm_actions(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.agent_confirm_actions = v, enabled);
    Ok(())
}

/// Turns coding jobs on or off. Off by default.
#[tauri::command]
#[specta::specta]
pub fn change_agent_jobs_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.agent_jobs_enabled = v, enabled);
    Ok(())
}

/// Sets the coding job time limit in minutes. The stored value is clamped to 1..=120.
#[tauri::command]
#[specta::specta]
pub fn change_agent_jobs_timeout_minutes(app: AppHandle, minutes: u32) -> Result<(), String> {
    let clamped = settings::clamp_job_timeout_minutes(minutes);
    let mut current = settings::get_settings(&app);
    current.agent_jobs_timeout_minutes = clamped;
    settings::write_settings(&app, current);
    Ok(())
}

/// Reports the coding job state for the Settings status card.
#[tauri::command]
#[specta::specta]
pub fn agent_job_status(wiring: State<'_, JobWiring>) -> AgentJobStatusDto {
    job_speaker::status_dto(&wiring.manager)
}

/// Cancels the running coding job. Returns false when nothing is running.
#[tauri::command]
#[specta::specta]
pub fn agent_job_cancel(wiring: State<'_, JobWiring>) -> bool {
    job_speaker::cancel(&wiring.manager)
}
