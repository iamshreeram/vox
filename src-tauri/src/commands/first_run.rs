//! First-run commands (P4). Main window only: `capabilities/chat.json` never grants them.
//! Errors are fixed texts and never echo settings values.

use crate::managers::first_run::{self, FirstRunOutcome, FirstRunStep};
use crate::settings::write_settings_gated;
use tauri::AppHandle;

/// Records how a first-run step was resolved (`Done` or `Skipped`). Finishing the Brain step as
/// `Done` requires the Brain to be in use for spoken replies, with transcript consent.
#[tauri::command]
#[specta::specta]
pub fn first_run_finish_step(
    app: AppHandle,
    step: FirstRunStep,
    outcome: FirstRunOutcome,
) -> Result<(), String> {
    let mut result = Ok(());
    write_settings_gated(&app, |settings| {
        result = first_run::finish_step(settings, step, outcome);
    });
    result
}
