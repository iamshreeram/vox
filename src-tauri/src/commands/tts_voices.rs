//! Tauri command that lists the installed voices for the Spoken replies dropdown.

use crate::managers::tts_voices::{list_installed_voices, TtsVoice};

/// Voices installed on this Mac for the Spoken replies dropdown (empty off macOS).
#[tauri::command]
#[specta::specta]
pub async fn list_tts_voices() -> Vec<TtsVoice> {
    list_installed_voices().await
}
