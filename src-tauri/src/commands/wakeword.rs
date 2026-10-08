use crate::managers::wakeword::WakeWordDetection;
use tauri::{AppHandle, Emitter, Manager};

pub const WAKEWORD_DETECTED_EVENT: &str = "wakeword://detected";

fn update_wake_word_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    if enabled {
        crate::managers::ambient::coordinator::enabling_wake_word_disables_ambient_mode(settings);
    } else {
        settings.wake_word_enabled = false;
    }
}

#[tauri::command]
#[specta::specta]
pub fn wakeword_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    update_wake_word_enabled(&mut settings, enabled);
    crate::settings::write_settings(&app, settings);
    let listener =
        app.state::<std::sync::Arc<crate::managers::wakeword_listener::WakeWordListener>>();
    let ambient_listener =
        app.state::<std::sync::Arc<crate::managers::ambient_listener::AmbientListener>>();
    if enabled {
        ambient_listener.stop();
        listener.start();
    } else {
        listener.stop();
    }
    Ok(())
}

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
    fn enabling_wake_word_disables_ambient_mode_and_round_trips_settings_value() {
        let mut settings = get_default_settings();
        settings.ambient_mode_enabled = true;
        update_wake_word_enabled(&mut settings, true);
        let serialized = serde_json::to_value(&settings).unwrap();
        let loaded: crate::settings::AppSettings = serde_json::from_value(serialized).unwrap();
        assert!(loaded.wake_word_enabled);
        assert!(!loaded.ambient_mode_enabled);
    }

    #[test]
    fn disabling_wake_word_does_not_change_ambient_mode() {
        let mut settings = get_default_settings();
        settings.ambient_mode_enabled = true;
        update_wake_word_enabled(&mut settings, false);
        assert!(!settings.wake_word_enabled);
        assert!(settings.ambient_mode_enabled);
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
        assert_eq!(value["confidence"].as_f64().unwrap() as f32, 0.87_f32);
        assert_eq!(value["timestamp_ms"], 12345);
    }

    #[test]
    fn wakeword_detected_event_name_is_stable() {
        assert_eq!(WAKEWORD_DETECTED_EVENT, "wakeword://detected");
    }
}
