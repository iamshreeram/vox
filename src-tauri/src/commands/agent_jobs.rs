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
    /// M4: the captured result text of the job, when one is held (scaffolding: always None).
    pub result: Option<String>,
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

/// Stores the coding agent allowlist (M4). Scaffolding: the raw list is stored and returned.
#[tauri::command]
#[specta::specta]
pub fn change_agent_jobs_agents(app: AppHandle, agents: Vec<String>) -> Vec<String> {
    let mut current = settings::get_settings(&app);
    current.agent_jobs_agents = agents.clone();
    settings::write_settings(&app, current);
    agents
}

/// Stores the flag that passes the selected agent name to the coding CLI (M4).
/// Scaffolding: the raw flag is stored and returned.
#[tauri::command]
#[specta::specta]
pub fn change_agent_jobs_agent_flag(app: AppHandle, flag: String) -> Result<String, String> {
    let mut current = settings::get_settings(&app);
    current.agent_jobs_agent_flag = flag.clone();
    settings::write_settings(&app, current);
    Ok(flag)
}

/// Turns ask mode on or off (M4). Off by default. Scaffolding: stores the raw value.
#[tauri::command]
#[specta::specta]
pub fn change_agent_jobs_ask_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    persist_bool(&app, |s, v| s.agent_jobs_ask_enabled = v, enabled);
    Ok(())
}

/// Stores the cleaned agent allowlist and returns it (M4 S1b stub; the implementer replaces this body).
pub(crate) fn apply_agents_change(s: &mut AppSettings, input: Vec<String>) -> Vec<String> {
    let _ = (s, input);
    todo!("M4 S1b")
}

/// Stores a valid agent flag, or returns `Invalid agent flag.` and keeps the old value (M4 S1b stub).
pub(crate) fn apply_flag_change(s: &mut AppSettings, flag: String) -> Result<String, String> {
    let _ = (s, flag);
    todo!("M4 S1b")
}

/// Stores the ask mode switch (M4 S1b stub; the implementer replaces this body).
pub(crate) fn apply_ask_change(s: &mut AppSettings, enabled: bool) {
    let _ = (s, enabled);
    todo!("M4 S1b")
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

#[cfg(test)]
#[path = "agent_jobs_m4_tests.rs"]
mod agent_jobs_m4_tests;
