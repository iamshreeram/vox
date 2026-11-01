//! Tauri commands for the "new version available" feature
//! (`managers::source_update`). Thin wrappers: all logic lives in the manager.

use crate::managers::source_update::{
    check_and_publish, install_blocker_now, start_update, SourceUpdateInfo, SourceUpdateMode,
};
use crate::managers::source_update_auto::consent::on_consent_withdrawn;
use crate::managers::source_update_auto::status::{
    auto_update_status, emit_auto_update_changed, AutoUpdateStatus,
};
use crate::managers::source_update_auto::update_coordinator;
use crate::managers::source_update_runner::UpdateProgress;
use crate::settings::{
    auto_update_consent, auto_update_effective_with, get_settings, write_settings_gated,
};
use tauri::{AppHandle, Emitter};

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

/// Turns automatic source updates on or off (R6-8, R7-6). The whole read-modify-write runs under
/// the update coordinator, so it is serialised with the commit sequence. Enabling needs a source
/// build with update checks on. Turning it off discards a built-but-not-committed update.
#[tauri::command]
#[specta::specta]
pub fn change_auto_update_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let coordinator = update_coordinator().guard();
    let before = get_settings(&app);
    if enabled {
        let mut candidate = before.clone();
        candidate.auto_update_enabled = true;
        if !auto_update_effective_with(&candidate, is_source_build()) {
            return Err("Automatic updates need a source build with update checks on.".into());
        }
    }
    write_settings_gated(&app, |settings| settings.auto_update_enabled = enabled);
    let after = get_settings(&app);
    drop(coordinator);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "auto_update_enabled",
            "value": enabled
        }),
    );
    if auto_update_consent(&before) && !auto_update_consent(&after) {
        on_consent_withdrawn(&app);
    } else {
        emit_auto_update_changed(&app);
    }
    Ok(())
}

/// The status of the automatic update for the Settings page (R6-8). Built from the settings,
/// the attempt state, the pending record and the active run. Stable codes only.
#[tauri::command]
#[specta::specta]
pub fn get_auto_update_status(app: AppHandle) -> AutoUpdateStatus {
    auto_update_status(&app)
}

/// True when this build is a source build (the same test the feature uses).
fn is_source_build() -> bool {
    crate::managers::source_update::current_mode() == SourceUpdateMode::Source
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
            "change_auto_update_setting",
            "get_auto_update_status",
        ] {
            assert!(window_command_allowed(MAIN_LABEL, command), "{command}");
            assert!(!window_command_allowed(ORB_LABEL, command), "{command}");
            assert!(!window_command_allowed(CHAT_LABEL, command), "{command}");
            assert!(!window_command_allowed(BUBBLES_LABEL, command), "{command}");
        }
    }
}
