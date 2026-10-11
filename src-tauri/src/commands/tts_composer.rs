//! Tauri commands for the speech composer settings and the Preview action.
//! All validation lives in `managers::composer_config`; these are thin.

use crate::managers::composer_config::{
    apply_patch, list_composer_providers, settings_from_app, write_to_app, ComposerProviderInfo,
    TtsComposerSettings, TtsComposerSettingsPatch,
};
use crate::managers::speech_pipeline::{run_preview, ComposerPreview, RealComposer};
use crate::managers::tts::TtsManager;
use crate::settings::{get_settings, write_settings, AppSettings};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

/// Serializes composer saves (other setters read-modify-write the whole
/// settings object without this lock; consent safety does not depend on it
/// because the acknowledgement is bound to the command tuple).
static COMPOSER_LOCK: Mutex<()> = Mutex::new(());

/// Pure core of `change_tts_composer_settings`: validate EVERYTHING, and only
/// on success write the normalized result into `settings`. On `Err` the
/// settings are untouched.
pub fn apply_to_settings(
    settings: &mut AppSettings,
    patch: &TtsComposerSettingsPatch,
) -> Result<TtsComposerSettings, String> {
    let current = settings_from_app(settings);
    let next = apply_patch(&current, patch, &settings.post_process_providers)?;
    write_to_app(settings, &next);
    Ok(next)
}

/// Persist FIRST, then invalidate pending speech so any in-flight exchange
/// is cancelled and the next one sees the new settings.
#[tauri::command]
#[specta::specta]
pub fn change_tts_composer_settings(
    app: AppHandle,
    patch: TtsComposerSettingsPatch,
) -> Result<TtsComposerSettings, String> {
    let _guard = COMPOSER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut settings = get_settings(&app);
    let next = apply_to_settings(&mut settings, &patch)?;
    write_settings(&app, settings);
    if let Some(tts) = app.try_state::<Arc<TtsManager>>() {
        tts.invalidate();
    }
    Ok(next)
}

/// Providers the composer can use (the UI shows each endpoint).
#[tauri::command]
#[specta::specta]
pub fn list_tts_composer_providers(app: AppHandle) -> Vec<ComposerProviderInfo> {
    list_composer_providers(&get_settings(&app).post_process_providers)
}

/// Compose a fixed sample with the CURRENT configuration and speak it when
/// spoken replies are enabled.
#[tauri::command]
#[specta::specta]
pub async fn preview_speech_composer(app: AppHandle) -> Result<ComposerPreview, String> {
    let tts = app
        .try_state::<Arc<TtsManager>>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| "Speech is not available".to_string())?;
    let load_app = app.clone();
    let load = move || get_settings(&load_app);
    run_preview(&tts, &load, &RealComposer).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::composer_config::{TtsComposerFallback, TtsComposerMode};
    use crate::settings::get_default_settings;

    #[test]
    fn a_valid_patch_is_written_into_settings_and_returned_normalized() {
        let mut s = get_default_settings();
        let patch = TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Model),
            model: Some("  tiny ".into()),
            fallback: Some(TtsComposerFallback::Verbatim),
            timeout_secs: Some(500),
            ..Default::default()
        };
        let out = apply_to_settings(&mut s, &patch).unwrap();
        assert_eq!(out.model, "tiny");
        assert_eq!(out.timeout_secs, 60);
        assert_eq!(s.tts_composer_mode, TtsComposerMode::Model);
        assert_eq!(s.tts_composer_model, "tiny");
        assert_eq!(s.tts_composer_fallback, TtsComposerFallback::Verbatim);
        assert_eq!(s.tts_composer_timeout_secs, 60);
        assert_eq!(settings_from_app(&s), out);
    }

    #[test]
    fn an_invalid_patch_leaves_every_setting_untouched() {
        let mut s = get_default_settings();
        let before = serde_json::to_value(&s).unwrap();
        let patch = TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Command),
            model: Some("fine".into()),
            command_path: Some("relative/agent".into()),
            ..Default::default()
        };
        assert!(apply_to_settings(&mut s, &patch).is_err());
        assert_eq!(serde_json::to_value(&s).unwrap(), before);
    }

    #[test]
    fn unsupported_providers_are_rejected_using_the_settings_own_provider_list() {
        let mut s = get_default_settings();
        let patch = TtsComposerSettingsPatch {
            provider_id: Some("anthropic".into()),
            ..Default::default()
        };
        assert!(apply_to_settings(&mut s, &patch).is_err());
        let ok = TtsComposerSettingsPatch {
            provider_id: Some("groq".into()),
            ..Default::default()
        };
        assert_eq!(apply_to_settings(&mut s, &ok).unwrap().provider_id, "groq");
    }

    #[test]
    fn the_acknowledgement_round_trips_through_the_app_settings() {
        let mut s = get_default_settings();
        let patch = TtsComposerSettingsPatch {
            command_path: Some("/bin/agent".into()),
            command_args: Some("--no-tools -m fast".into()),
            prompt_flag: Some("-p".into()),
            acknowledge_command: Some(true),
            ..Default::default()
        };
        apply_to_settings(&mut s, &patch).unwrap();
        assert_eq!(
            s.tts_composer_command_ack,
            "/bin/agent\0--no-tools -m fast\0-p"
        );
        let change = TtsComposerSettingsPatch {
            command_args: Some("-m other".into()),
            ..Default::default()
        };
        apply_to_settings(&mut s, &change).unwrap();
        assert!(!crate::managers::composer_config::command_ack_matches(
            &settings_from_app(&s)
        ));
    }
}
