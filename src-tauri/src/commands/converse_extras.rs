//! Tauri commands for the new converse toggles (Smart Turn, latency panel, speaker
//! filter). Each command is a thin read-modify-write over a pure applier that takes
//! only `&mut AppSettings`, so no applier can reach a model manager or download.

use crate::settings::{self, AppSettings, SpeakerSensitivity};
use tauri::AppHandle;

/// Smart Turn on/off. Only `converse_smart_turn` changes.
pub fn apply_smart_turn(settings: &mut AppSettings, enabled: bool) {
    settings.converse_smart_turn = enabled;
}

/// Live latency panel on/off. Only `converse_latency_panel` changes.
pub fn apply_latency_panel(settings: &mut AppSettings, enabled: bool) {
    settings.converse_latency_panel = enabled;
}

/// Speaker filter on/off. Only `speaker_filter_enabled` changes.
pub fn apply_speaker_filter_enabled(settings: &mut AppSettings, enabled: bool) {
    settings.speaker_filter_enabled = enabled;
}

/// Speaker filter strictness. Only `speaker_filter_sensitivity` changes.
pub fn apply_speaker_filter_sensitivity(
    settings: &mut AppSettings,
    sensitivity: SpeakerSensitivity,
) {
    settings.speaker_filter_sensitivity = sensitivity;
}

fn persist_extra(app: &AppHandle, apply: impl FnOnce(&mut AppSettings)) {
    let mut current = settings::get_settings(app);
    apply(&mut current);
    settings::write_settings(app, current);
}

#[tauri::command]
#[specta::specta]
pub fn change_converse_smart_turn_setting(app: AppHandle, enabled: bool) {
    persist_extra(&app, |s| apply_smart_turn(s, enabled));
}

#[tauri::command]
#[specta::specta]
pub fn change_converse_latency_panel_setting(app: AppHandle, enabled: bool) {
    persist_extra(&app, |s| apply_latency_panel(s, enabled));
}

#[tauri::command]
#[specta::specta]
pub fn change_speaker_filter_enabled_setting(app: AppHandle, enabled: bool) {
    persist_extra(&app, |s| apply_speaker_filter_enabled(s, enabled));
}

#[tauri::command]
#[specta::specta]
pub fn change_speaker_filter_sensitivity_setting(app: AppHandle, sensitivity: SpeakerSensitivity) {
    persist_extra(&app, |s| apply_speaker_filter_sensitivity(s, sensitivity));
}

#[cfg(test)]
#[path = "converse_extras_tests.rs"]
mod tests;
