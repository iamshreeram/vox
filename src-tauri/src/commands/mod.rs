pub mod agent_bridge;
pub mod agent_tools;
pub mod ambient;
pub mod audio;
pub mod conversation_records;
pub mod converse;
pub mod history;
pub mod memory;
pub mod models;
pub mod realtime;
pub mod source_update;
pub mod transcription;
pub mod tts_composer;
pub mod voice_shortcuts;
pub mod wakeword;

use crate::settings::{
    get_settings, update_checks_forced_disabled, write_settings, AppSettings, LogLevel,
};
use crate::utils::cancel_current_operation;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

/// Overlay Cancel button. An active conversation session takes the press and
/// is stopped exactly as `converse_stop` does; otherwise dictation cancel runs.
#[tauri::command]
#[specta::specta]
pub fn cancel_operation(app: AppHandle) {
    cancel_routed(&app);
}

/// Conversation-aware cancel shared by the overlay button, the tray Cancel item
/// and the `--cancel` CLI path. An active conversation session is stopped; any
/// other state runs the dictation cancel. The keyboard shortcut path in
/// `actions.rs` calls `utils::cancel_current_operation` directly and is unchanged.
pub fn cancel_routed(app: &AppHandle) {
    let conversation_active = app
        .state::<Arc<crate::managers::converse_runtime::ConverseController>>()
        .is_active();
    match crate::managers::converse_overlay::cancel_route(conversation_active) {
        crate::managers::converse_overlay::CancelRoute::Conversation => {
            app.state::<Arc<crate::managers::converse_runtime::ConverseController>>()
                .stop();
        }
        crate::managers::converse_overlay::CancelRoute::Dictation => {
            cancel_current_operation(app);
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn is_portable() -> bool {
    crate::portable::is_portable()
}

#[tauri::command]
#[specta::specta]
pub fn is_update_checks_locked() -> bool {
    update_checks_forced_disabled()
}

#[tauri::command]
#[specta::specta]
pub fn get_app_dir_path(app: AppHandle) -> Result<String, String> {
    let app_data_dir = crate::portable::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    Ok(app_data_dir.to_string_lossy().to_string())
}

#[tauri::command]
#[specta::specta]
pub fn get_app_settings(app: AppHandle) -> Result<AppSettings, String> {
    Ok(get_settings(&app))
}

#[tauri::command]
#[specta::specta]
pub fn get_default_settings() -> Result<AppSettings, String> {
    Ok(crate::settings::get_default_settings())
}

#[tauri::command]
#[specta::specta]
pub fn get_log_dir_path(app: AppHandle) -> Result<String, String> {
    let log_dir = crate::portable::app_log_dir(&app)
        .map_err(|e| format!("Failed to get log directory: {}", e))?;

    Ok(log_dir.to_string_lossy().to_string())
}

#[specta::specta]
#[tauri::command]
pub fn set_log_level(app: AppHandle, level: LogLevel) -> Result<(), String> {
    let tauri_log_level: tauri_plugin_log::LogLevel = level.into();
    let log_level: log::Level = tauri_log_level.into();
    // Update the file log level atomic so the filter picks up the new level
    crate::FILE_LOG_LEVEL.store(
        log_level.to_level_filter() as u8,
        std::sync::atomic::Ordering::Relaxed,
    );

    let mut settings = get_settings(&app);
    settings.log_level = level;
    write_settings(&app, settings);

    Ok(())
}

#[specta::specta]
#[tauri::command]
pub fn open_recordings_folder(app: AppHandle) -> Result<(), String> {
    let app_data_dir = crate::portable::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let recordings_dir = app_data_dir.join("recordings");

    let path = recordings_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open recordings folder: {}", e))?;

    Ok(())
}

#[specta::specta]
#[tauri::command]
pub fn open_log_dir(app: AppHandle) -> Result<(), String> {
    let log_dir = crate::portable::app_log_dir(&app)
        .map_err(|e| format!("Failed to get log directory: {}", e))?;

    let path = log_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open log directory: {}", e))?;

    Ok(())
}

#[specta::specta]
#[tauri::command]
pub fn open_app_data_dir(app: AppHandle) -> Result<(), String> {
    let app_data_dir = crate::portable::app_data_dir(&app)
        .map_err(|e| format!("Failed to get app data directory: {}", e))?;

    let path = app_data_dir.to_string_lossy().as_ref().to_string();
    app.opener()
        .open_path(path, None::<String>)
        .map_err(|e| format!("Failed to open app data directory: {}", e))?;

    Ok(())
}

/// Check if Apple Intelligence is available on this device.
/// Called by the frontend when the user selects Apple Intelligence provider.
#[specta::specta]
#[tauri::command]
pub fn check_apple_intelligence_available() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        crate::apple_intelligence::check_apple_intelligence_availability()
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        false
    }
}

/// Try to initialize Enigo (keyboard/mouse simulation).
/// On macOS, this will return an error if accessibility permissions are not granted.
#[specta::specta]
#[tauri::command]
pub fn initialize_enigo(app: AppHandle) -> Result<(), String> {
    use crate::input::EnigoState;

    // Check if already initialized
    if app.try_state::<EnigoState>().is_some() {
        log::debug!("Enigo already initialized");
        return Ok(());
    }

    // Try to initialize
    match EnigoState::new() {
        Ok(enigo_state) => {
            app.manage(enigo_state);
            log::info!("Enigo initialized successfully after permission grant");
            Ok(())
        }
        Err(e) => {
            if cfg!(target_os = "macos") {
                log::warn!(
                    "Failed to initialize Enigo: {} (accessibility permissions may not be granted)",
                    e
                );
            } else {
                log::warn!("Failed to initialize Enigo: {}", e);
            }
            Err(format!("Failed to initialize input system: {}", e))
        }
    }
}

/// Marker state to track if shortcuts have been initialized.
pub struct ShortcutsInitialized;

/// Initialize keyboard shortcuts.
/// On macOS, this should be called after accessibility permissions are granted.
/// This is idempotent - calling it multiple times is safe.
#[specta::specta]
#[tauri::command]
pub fn initialize_shortcuts(app: AppHandle) -> Result<(), String> {
    // Check if already initialized
    if app.try_state::<ShortcutsInitialized>().is_some() {
        log::debug!("Shortcuts already initialized");
        return Ok(());
    }

    // Initialize shortcuts
    crate::shortcut::init_shortcuts(&app);

    // Mark as initialized before reconciling the macOS Secure Input fallback.
    app.manage(ShortcutsInitialized);
    crate::secure_input::reconcile_fallback(&app);

    log::info!("Shortcuts initialized successfully");
    Ok(())
}

#[cfg(test)]
mod cancel_wiring_tests {
    //! Wiring regression: the tray Cancel item and the single-instance `--cancel`
    //! arm must reach the conversation-aware `cancel_routed`, never the bare
    //! `utils::cancel_current_operation` (which would skip conversation routing).

    const LIB_SOURCE: &str = include_str!("../lib.rs");

    /// Returns the text of the match arm that starts with `needle`, up to the
    /// next line that begins a sibling arm or closes the block.
    fn arm_body<'a>(source: &'a str, needle: &str) -> &'a str {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("arm `{needle}` not found in lib.rs"));
        let rest = &source[start..];
        let end = rest.find("\"quit\" =>").unwrap_or(rest.len().min(400));
        &rest[..end.min(rest.len())]
    }

    #[test]
    fn tray_cancel_arm_routes_through_cancel_routed() {
        let arm = arm_body(LIB_SOURCE, "\"cancel\" => {");
        assert!(
            arm.contains("cancel_routed("),
            "tray cancel arm must call cancel_routed: {arm}"
        );
        assert!(
            !arm.contains("cancel_current_operation("),
            "tray cancel arm must not call cancel_current_operation directly"
        );
    }

    #[test]
    fn cli_cancel_arm_routes_through_cancel_routed() {
        let start = LIB_SOURCE
            .find("args.iter().any(|a| a == \"--cancel\")")
            .expect("--cancel arm not found in lib.rs");
        let arm = &LIB_SOURCE[start..(start + 200).min(LIB_SOURCE.len())];
        assert!(
            arm.contains("cancel_routed("),
            "--cancel arm must call cancel_routed: {arm}"
        );
        assert!(
            !arm.contains("cancel_current_operation("),
            "--cancel arm must not call cancel_current_operation directly"
        );
    }

    #[test]
    fn lib_has_no_direct_cancel_current_operation_call() {
        assert!(
            !LIB_SOURCE.contains("cancel_current_operation("),
            "lib.rs must reach cancellation only through commands::cancel_routed"
        );
    }
}
