//! Footer and Settings data for the automatic update (R6-8, R7-7 c): the `AutoUpdateStatus`
//! value, its pure builder, the stable reason-code filter and the change event.
//!
//! Attached to `source_update_auto` with a `#[path]` attribute. Nothing here starts or stops a
//! run; it only reads the settings, the attempt state, the pending record and the active run.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter};

use crate::managers::source_update_auto::{load_auto_state, AutoResult, AutoState};
use crate::managers::source_update_pending::{PendingRun, PendingState, PendingStore};
use crate::managers::source_update_runner::update_state_dir;

/// Event emitted whenever the attempt state or the phase may have changed.
pub const AUTO_UPDATE_EVENT: &str = "auto-update-state-changed";

/// Length of the short target shown in the status.
pub const TARGET_SHORT_CHARS: usize = 8;

/// Phase of the automatic update as the Settings page describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AutoPhase {
    /// Nothing is running or waiting.
    Idle,
    /// A build run is in progress.
    Building,
    /// A build is staged and waits for a quiet moment.
    Waiting,
    /// The install job has been started.
    Installing,
}

/// Outcome of the last automatic attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AutoLastResult {
    /// No attempt is recorded.
    #[serde(rename = "none")]
    NoAttempt,
    /// The last attempt installed the update.
    Updated,
    /// The last attempt failed or was interrupted.
    Failed,
    /// The last attempt was refused by the policy.
    Skipped,
}

/// Status of the automatic update for the Settings page. Carries stable codes only, never
/// free text: `reason_code` is one of the known codes, `target_short` the first eight hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AutoUpdateStatus {
    /// The owner switched automatic updates on.
    pub enabled: bool,
    /// Enabled and effective in this build (source build, checks on).
    pub effective: bool,
    /// Current phase.
    pub phase: AutoPhase,
    /// Outcome of the last attempt.
    pub last_result: AutoLastResult,
    /// Stable reason code of a failed or skipped attempt, or `None`.
    pub reason_code: Option<String>,
    /// First eight hex digits of the target the status is about, or `None`.
    pub target_short: Option<String>,
    /// Unix seconds of the last recorded attempt, or `None`.
    pub ts: Option<i64>,
}

/// Facts the status is built from. Gathered by the caller.
#[derive(Debug, Clone, Copy)]
pub struct StatusFacts<'a> {
    /// The owner switch.
    pub enabled: bool,
    /// [`crate::settings::auto_update_effective`].
    pub effective: bool,
    /// An update run is active in this process.
    pub active_run: bool,
    /// The pending record, if any.
    pub pending: Option<&'a PendingRun>,
    /// The attempt state, if any.
    pub last: Option<&'a AutoState>,
}

/// Reason codes that may appear in the status. Every `code()` of the policy enums is here, and
/// the failure codes of the commit sequence are listed as well.
const STABLE_REASON_CODES: [&str; 16] = [
    "disabled",
    "not_source",
    "no_update",
    "blocked",
    "backoff",
    "unverified",
    "verify_failed",
    "active_run",
    "busy",
    "not_idle",
    "verifying",
    "interrupted",
    "build_failed",
    "install_failed",
    "spawn_failed",
    "invalid_pending",
];

/// The stable code for `raw` when it is a known code, otherwise `None`. Free text never passes.
pub fn stable_reason_code(raw: &str) -> Option<&'static str> {
    STABLE_REASON_CODES
        .iter()
        .copied()
        .find(|known| *known == raw)
}

/// Phase from the active run and the pending record. An install in progress wins, then a build
/// run, then a resting build.
pub fn phase_for(active_run: bool, pending: Option<&PendingRun>) -> AutoPhase {
    match pending.map(|run| &run.state) {
        Some(PendingState::Committing(_)) => AutoPhase::Installing,
        _ if active_run => AutoPhase::Building,
        Some(PendingState::Built) => AutoPhase::Waiting,
        None => AutoPhase::Idle,
    }
}

/// Builds the status from `facts`. Pure.
pub fn build_status(facts: &StatusFacts<'_>) -> AutoUpdateStatus {
    let last_result = match facts.last.and_then(|state| state.last_result) {
        None => AutoLastResult::NoAttempt,
        Some(AutoResult::Updated) => AutoLastResult::Updated,
        Some(AutoResult::Failed) => AutoLastResult::Failed,
        Some(AutoResult::Skipped) => AutoLastResult::Skipped,
    };
    let shows_reason = matches!(
        last_result,
        AutoLastResult::Failed | AutoLastResult::Skipped
    );
    let reason_code = facts
        .last
        .filter(|_| shows_reason)
        .and_then(|state| state.reason_code.as_deref())
        .and_then(stable_reason_code)
        .map(str::to_string);
    let target = facts
        .pending
        .map(|run| run.target_full.as_str())
        .or_else(|| facts.last.and_then(|state| state.last_target.as_deref()));
    AutoUpdateStatus {
        enabled: facts.enabled,
        effective: facts.effective,
        phase: phase_for(facts.active_run, facts.pending),
        last_result,
        reason_code,
        target_short: target.map(short_target),
        ts: facts.last.and_then(|state| i64::try_from(state.ts).ok()),
    }
}

/// The first [`TARGET_SHORT_CHARS`] characters of a full SHA.
fn short_target(full: &str) -> String {
    full.chars().take(TARGET_SHORT_CHARS).collect()
}

/// Reads the settings, the attempt state, the pending record and the active run, then builds
/// the status. Read failures give the empty view (idle, no attempt).
pub fn auto_update_status(app: &AppHandle) -> AutoUpdateStatus {
    let settings = crate::settings::get_settings(app);
    let enabled = settings.auto_update_enabled;
    let effective = crate::settings::auto_update_effective(&settings);
    let active_run = crate::managers::source_update::active_run_id(app).is_some();
    let Ok(app_data) = crate::portable::app_data_dir(app) else {
        return build_status(&StatusFacts {
            enabled,
            effective,
            active_run,
            pending: None,
            last: None,
        });
    };
    let state_dir = update_state_dir(&app_data);
    let last = load_auto_state(&state_dir).ok().flatten();
    let pending = PendingStore::new(&state_dir).load().ok().flatten();
    build_status(&StatusFacts {
        enabled,
        effective,
        active_run,
        pending: pending.as_ref(),
        last: last.as_ref(),
    })
}

/// Emits [`AUTO_UPDATE_EVENT`] with the current status. Call whenever the attempt state, the
/// phase, or the consent changes.
pub fn emit_auto_update_changed(app: &AppHandle) {
    let _ = app.emit(AUTO_UPDATE_EVENT, auto_update_status(app));
}

#[cfg(test)]
#[path = "source_update_status_tests.rs"]
mod tests;
