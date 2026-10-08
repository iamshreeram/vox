use crate::managers::ambient::RollingTranscript;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

pub const AMBIENT_ADDRESSED_EVENT: &str = "ambient://addressed";

fn update_ambient_mode_enabled(settings: &mut crate::settings::AppSettings, enabled: bool) {
    settings.ambient_mode_enabled = enabled;
}

/// NOTE: direct settings write placeholder -- once task A5 lands (mutual
/// exclusion with Wake Word), this must go through A5's shared validation
/// function instead of writing the setting directly.
#[tauri::command]
#[specta::specta]
pub fn ambient_set_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = crate::settings::get_settings(&app);
    update_ambient_mode_enabled(&mut settings, enabled);
    crate::settings::write_settings(&app, settings);
    Ok(())
}

/// NOTE: this command will panic at runtime until task A7 registers an
/// `Arc<RollingTranscript>` as managed Tauri state (`app.manage(...)`) --
/// that registration doesn't exist yet. This task only builds the IPC
/// surface/shape; A7 wires it to the live coordinator.
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

/// Emits the ambient-addressed event. Thin wrapper around `AppHandle::emit`
/// -- payload shape covered by the serialization test below; no
/// `tauri::test` mock-AppHandle infra exists in this crate (matches
/// `commands/memory.rs`'s and `commands/wakeword.rs`'s house style).
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
    fn enabling_ambient_mode_updates_the_setting() {
        let mut settings = get_default_settings();
        update_ambient_mode_enabled(&mut settings, true);
        assert!(settings.ambient_mode_enabled);
    }

    #[test]
    fn disabling_ambient_mode_updates_the_setting() {
        let mut settings = get_default_settings();
        update_ambient_mode_enabled(&mut settings, true);
        update_ambient_mode_enabled(&mut settings, false);
        assert!(!settings.ambient_mode_enabled);
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
