//! Tauri commands for the "new version available" feature
//! (`managers::source_update`). Thin wrappers: all logic lives in the manager.

use crate::managers::source_update::{
    check_and_publish, install_blocker_now, start_update, SourceUpdateInfo, SourceUpdateMode,
};
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

/// Open Terminal running the update script.
#[tauri::command]
#[specta::specta]
pub async fn start_source_update(app: AppHandle) -> Result<(), String> {
    start_update(&app).await
}
