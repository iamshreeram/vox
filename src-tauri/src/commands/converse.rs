//! Tauri commands and the shared toggle for the conversation session. The
//! session runtime lives in `managers::converse_runtime`; these are thin.

use crate::managers::converse_runtime::{ConverseController, ConverseStatus};
use crate::settings::{self, AppSettings};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

/// Pure core of the conversation number setters: the only place that knows the
/// three keys and their ranges. Clamps `value` to the key's range and writes it
/// into `settings`. An unknown key (including the empty string) is refused and
/// leaves `settings` untouched.
pub fn apply_converse_setting(
    settings: &mut AppSettings,
    key: &str,
    value: u32,
) -> Result<(), String> {
    match key {
        "converse_silence_ms" => {
            settings.converse_silence_ms = value.clamp(600, 4000);
        }
        "converse_max_idle_turns" => {
            settings.converse_max_idle_turns = value.clamp(1, 5) as u8;
        }
        "converse_context_turns" => {
            settings.converse_context_turns = value.clamp(0, 12) as u8;
        }
        _ => return Err("Unknown setting".to_string()),
    }
    Ok(())
}

/// Read-modify-write of one conversation number through `apply_converse_setting`.
fn persist_converse_setting(app: &AppHandle, key: &str, value: u32) -> Result<(), String> {
    let mut current = settings::get_settings(app);
    apply_converse_setting(&mut current, key, value)?;
    settings::write_settings(app, current);
    Ok(())
}

/// Persist the silence length (ms) that ends a turn. Clamped to 600..=4000.
#[tauri::command]
#[specta::specta]
pub fn change_converse_silence_ms_setting(app: AppHandle, value: u32) -> Result<(), String> {
    persist_converse_setting(&app, "converse_silence_ms", value)
}

/// Persist the idle turns before a conversation ends. Clamped to 1..=5.
#[tauri::command]
#[specta::specta]
pub fn change_converse_max_idle_turns_setting(app: AppHandle, value: u32) -> Result<(), String> {
    persist_converse_setting(&app, "converse_max_idle_turns", value)
}

/// Persist the number of prior turns sent as context. Clamped to 0..=12.
#[tauri::command]
#[specta::specta]
pub fn change_converse_context_turns_setting(app: AppHandle, value: u32) -> Result<(), String> {
    persist_converse_setting(&app, "converse_context_turns", value)
}

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

#[cfg(test)]
mod apply_setting_tests {
    use super::*;
    use crate::settings::get_default_settings;

    #[test]
    fn silence_is_stored_when_in_range() {
        let mut s = get_default_settings();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_silence_ms", 2500),
            Ok(())
        );
        assert_eq!(s.converse_silence_ms, 2500);
    }

    #[test]
    fn silence_below_min_is_clamped_up() {
        let mut s = get_default_settings();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_silence_ms", 0),
            Ok(())
        );
        assert_eq!(s.converse_silence_ms, 600);
    }

    #[test]
    fn silence_above_max_is_clamped_down() {
        let mut s = get_default_settings();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_silence_ms", u32::MAX),
            Ok(())
        );
        assert_eq!(s.converse_silence_ms, 4000);
    }

    #[test]
    fn idle_turns_are_clamped_to_one_through_five() {
        let mut s = get_default_settings();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_max_idle_turns", 3),
            Ok(())
        );
        assert_eq!(s.converse_max_idle_turns, 3);
        assert_eq!(
            apply_converse_setting(&mut s, "converse_max_idle_turns", 0),
            Ok(())
        );
        assert_eq!(s.converse_max_idle_turns, 1);
        assert_eq!(
            apply_converse_setting(&mut s, "converse_max_idle_turns", u32::MAX),
            Ok(())
        );
        assert_eq!(s.converse_max_idle_turns, 5);
    }

    #[test]
    fn context_turns_are_clamped_to_zero_through_twelve() {
        let mut s = get_default_settings();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_context_turns", 0),
            Ok(())
        );
        assert_eq!(s.converse_context_turns, 0);
        assert_eq!(
            apply_converse_setting(&mut s, "converse_context_turns", 12),
            Ok(())
        );
        assert_eq!(s.converse_context_turns, 12);
        assert_eq!(
            apply_converse_setting(&mut s, "converse_context_turns", u32::MAX),
            Ok(())
        );
        assert_eq!(s.converse_context_turns, 12);
    }

    #[test]
    fn unknown_key_is_refused_and_settings_are_untouched() {
        let mut s = get_default_settings();
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            apply_converse_setting(&mut s, "converse_bogus", 3),
            Err("Unknown setting".to_string())
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }

    #[test]
    fn empty_key_is_refused_and_settings_are_untouched() {
        let mut s = get_default_settings();
        let before = serde_json::to_value(&s).unwrap();
        assert_eq!(
            apply_converse_setting(&mut s, "", 3),
            Err("Unknown setting".to_string())
        );
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }
}
