use crate::managers::wakeword::WakeWordDetection;
use tauri::{AppHandle, Emitter, Manager};

pub const WAKEWORD_DETECTED_EVENT: &str = "wakeword://detected";

fn update_wake_word_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.wake_word_enabled = enabled;
}

#[tauri::command]
#[specta::specta]
pub fn wakeword_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    update_wake_word_enabled(&mut settings, enabled);
    crate::settings::write_settings(&app, settings);
    let listener =
        app.state::<std::sync::Arc<crate::managers::wakeword_listener::WakeWordListener>>();
    if enabled {
        listener.start();
    } else {
        listener.stop();
    }
    Ok(())
}

/// Emits the wake-word detection event. Thin wrapper around `AppHandle::emit`
/// -- the payload shape is covered by
/// `wake_word_detection_serializes_with_expected_fields` below; this crate
/// has no `tauri::test` mock-AppHandle infrastructure, matching the existing
/// house style in `commands/memory.rs` (which also only unit-tests its pure
/// helpers, never the `#[tauri::command]` wrapper with a real AppHandle).
#[allow(dead_code)]
pub fn emit_wakeword_detection(
    app: &AppHandle,
    detection: &WakeWordDetection,
) -> tauri::Result<()> {
    app.emit(WAKEWORD_DETECTED_EVENT, detection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    #[test]
    fn enabling_wake_word_updates_the_setting() {
        let mut settings = get_default_settings();
        update_wake_word_enabled(&mut settings, true);
        assert!(settings.wake_word_enabled);
    }

    #[test]
    fn disabling_wake_word_updates_the_setting() {
        let mut settings = get_default_settings();
        update_wake_word_enabled(&mut settings, true);
        update_wake_word_enabled(&mut settings, false);
        assert!(!settings.wake_word_enabled);
    }

    #[test]
    fn wake_word_detection_serializes_with_expected_fields() {
        let detection = WakeWordDetection {
            model_name: "hey_jarvis".to_string(),
            confidence: 0.87,
            timestamp_ms: 12345,
        };
        let value = serde_json::to_value(&detection).unwrap();
        assert_eq!(value["model_name"], "hey_jarvis");
        // f32 -> JSON widens to f64, so 0.87_f32 round-trips as
        // 0.8700000047683716 in the JSON number; cast back to f32 before
        // comparing instead of asserting against the f64 literal directly.
        assert_eq!(value["confidence"].as_f64().unwrap() as f32, 0.87_f32);
        assert_eq!(value["timestamp_ms"], 12345);
    }

    #[test]
    fn wakeword_detected_event_name_is_stable() {
        assert_eq!(WAKEWORD_DETECTED_EVENT, "wakeword://detected");
    }
}
