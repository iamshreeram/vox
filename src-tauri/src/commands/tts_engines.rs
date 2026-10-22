//! Tauri command that exposes the TTS engine catalog to the settings UI.

use crate::managers::tts_engines::{tts_engine_catalog, TtsEngineInfo};

/// Every speech engine in picker order, with platform support and download facts.
#[tauri::command]
#[specta::specta]
pub fn list_tts_engines() -> Vec<TtsEngineInfo> {
    tts_engine_catalog()
}
