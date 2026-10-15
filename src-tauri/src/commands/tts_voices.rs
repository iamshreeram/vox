//! Tauri command that lists the installed voices for the Spoken replies dropdown.
//! STUB (voice picker S1-A): returns an empty list until S1-B calls `list_installed_voices`.

use crate::managers::tts_voices::TtsVoice;

/// Voices installed on this Mac for the Spoken replies dropdown (empty off macOS).
#[tauri::command]
#[specta::specta]
pub async fn list_tts_voices() -> Vec<TtsVoice> {
    Vec::new()
}
