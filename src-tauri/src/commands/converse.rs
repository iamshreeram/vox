//! Tauri commands and the shared toggle for the conversation session. The
//! session runtime lives in `managers::converse_runtime`; these are thin.

use crate::managers::converse_runtime::{ConverseController, ConverseStatus};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Tauri event carrying a human-readable start refusal (`String` payload).
pub const CONVERSE_ERROR_EVENT: &str = "converse-error";

/// Shared toggle used by the shortcut action and the tray item: stop when a
/// session is active, otherwise start one. A start refusal is emitted as a
/// `converse-error` event carrying only the human message.
pub fn toggle_conversation(app: &AppHandle) {
    let controller = app.state::<Arc<ConverseController>>();
    if controller.is_active() {
        controller.stop();
        return;
    }
    if let Err(message) = controller.start() {
        if app.emit(CONVERSE_ERROR_EVENT, message).is_err() {
            log::debug!("converse: error event not delivered");
        }
    }
}

/// Start a conversation session. `Err(message)` carries the human reason.
#[tauri::command]
#[specta::specta]
pub fn converse_start(app: AppHandle) -> Result<(), String> {
    let controller = app.state::<Arc<ConverseController>>();
    controller.start().map(|_| ())
}

/// Ask the running session to stop. True when a running session was asked.
#[tauri::command]
#[specta::specta]
pub fn converse_stop(app: AppHandle) -> bool {
    app.state::<Arc<ConverseController>>().stop()
}

/// "idle" | "running" | "stopping".
#[tauri::command]
#[specta::specta]
pub fn converse_status(app: AppHandle) -> String {
    match app.state::<Arc<ConverseController>>().status() {
        ConverseStatus::Idle => "idle",
        ConverseStatus::Running => "running",
        ConverseStatus::Stopping => "stopping",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_event_name_is_stable() {
        assert_eq!(CONVERSE_ERROR_EVENT, "converse-error");
    }
}
