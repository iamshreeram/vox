//! Tauri commands for the "new version available" feature
//! (`managers::source_update`). Thin wrappers: all logic lives in the manager.

use crate::managers::source_update::{
    check_and_publish, install_blocker_now, start_update, SourceUpdateInfo, SourceUpdateMode,
};
use crate::managers::source_update_runner::UpdateProgress;
use tauri::AppHandle;

/// Which update flow the footer should use for this running build.
#[tauri::command]
#[specta::specta]
pub fn get_update_mode() -> SourceUpdateMode {
    crate::managers::source_update::current_mode()
}

/// Check GitHub now (manual check; honours the short debounce).
#[tauri::command]
#[specta::specta]
pub async fn check_source_update(app: AppHandle) -> Result<SourceUpdateInfo, String> {
    check_and_publish(&app, false).await
}

/// The last published check result, if any.
#[tauri::command]
#[specta::specta]
pub fn get_cached_source_update(app: AppHandle) -> Option<SourceUpdateInfo> {
    crate::managers::source_update::cached(&app)
}

/// Why an install cannot start right now, or `None` when it can.
#[tauri::command]
#[specta::specta]
pub async fn get_source_update_install_blocker(app: AppHandle) -> Option<String> {
    install_blocker_now(&app).await
}

/// Starts the update in the background, with no window. Attaches to a run that is
/// already in progress.
#[tauri::command]
#[specta::specta]
pub async fn start_source_update(app: AppHandle) -> Result<(), String> {
    start_update(&app).await
}

/// The latest progress of the running (or last finished) background update.
#[tauri::command]
#[specta::specta]
pub fn get_source_update_progress(app: AppHandle) -> Option<UpdateProgress> {
    crate::managers::source_update::progress(&app)
}

#[cfg(test)]
mod tests {
    use crate::window_commands::{window_command_allowed, CHAT_LABEL, MAIN_LABEL, ORB_LABEL};

    const BUBBLES_LABEL: &str = "orb-bubbles";

    /// The progress command follows the same rule as the other source-update
    /// commands: never allowed in the orb, chat or bubble windows, allowed in main.
    #[test]
    fn progress_command_is_gated_like_the_other_source_update_commands() {
        for command in [
            "get_cached_source_update",
            "start_source_update",
            "get_source_update_progress",
        ] {
            assert!(window_command_allowed(MAIN_LABEL, command), "{command}");
            assert!(!window_command_allowed(ORB_LABEL, command), "{command}");
            assert!(!window_command_allowed(CHAT_LABEL, command), "{command}");
            assert!(!window_command_allowed(BUBBLES_LABEL, command), "{command}");
        }
    }
}
