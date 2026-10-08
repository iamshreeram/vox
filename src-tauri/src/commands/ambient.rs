use crate::managers::ambient::RollingTranscript;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

pub const AMBIENT_ADDRESSED_EVENT: &str = "ambient://addressed";

fn update_ambient_mode_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    if enabled {
        crate::managers::ambient::coordinator::enabling_ambient_mode_disables_wake_word(settings);
    } else {
        settings.ambient_mode_enabled = false;
    }
}

#[tauri::command]
#[specta::specta]
pub fn ambient_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    update_ambient_mode_enabled(&mut settings, enabled);
    crate::settings::write_settings(&app, settings);
    let listener = app.state::<Arc<crate::managers::ambient_listener::AmbientListener>>();
    if enabled {
        app.state::<Arc<crate::managers::wakeword_listener::WakeWordListener>>()
            .stop();
        listener.start();
    } else {
        listener.stop();
    }
    Ok(())
}

/// Shared RAM-only transcript registered alongside the ambient listener.
#[tauri::command]
#[specta::specta]
pub fn ambient_clear(transcript: State<'_, Arc<RollingTranscript>>) -> Result<(), String> {
    transcript.clear();
    Ok(())
}

#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct AmbientAddressedPayload {
    pub extracted_request: String,
}

#[allow(dead_code)]
pub fn emit_ambient_addressed(app: &AppHandle, extracted_request: &str) -> tauri::Result<()> {
    app.emit(
        AMBIENT_ADDRESSED_EVENT,
        AmbientAddressedPayload {
            extracted_request: extracted_request.to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    #[test]
    fn enabling_ambient_mode_disables_wake_word_and_round_trips_settings_value() {
        let mut settings = get_default_settings();
        settings.wake_word_enabled = true;
        update_ambient_mode_enabled(&mut settings, true);
        let serialized = serde_json::to_value(&settings).unwrap();
        let loaded: crate::settings::AppSettings = serde_json::from_value(serialized).unwrap();
        assert!(loaded.ambient_mode_enabled);
        assert!(!loaded.wake_word_enabled);
    }

    #[test]
    fn disabling_ambient_mode_does_not_change_wake_word() {
        let mut settings = get_default_settings();
        settings.wake_word_enabled = true;
        update_ambient_mode_enabled(&mut settings, false);
        assert!(!settings.ambient_mode_enabled);
        assert!(settings.wake_word_enabled);
    }

    #[test]
    fn ambient_clear_empties_a_real_rolling_transcript_fixture() {
        let transcript = RollingTranscript::new(90_000);
        transcript.add_segment("hello", 0, 0, 0.9, false);
        assert_eq!(transcript.stored_segment_count(), 1);
        transcript.clear();
        assert_eq!(transcript.stored_segment_count(), 0);
    }

    #[test]
    fn ambient_addressed_payload_serializes_with_expected_field() {
        let payload = AmbientAddressedPayload {
            extracted_request: "what time is it".to_string(),
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(value["extracted_request"], "what time is it");
    }

    #[test]
    fn ambient_addressed_event_name_is_stable() {
        assert_eq!(AMBIENT_ADDRESSED_EVENT, "ambient://addressed");
    }
}
