//! Phonon voice: the settings commands for the Phonon peer engine (DESIGN.md Rev 4, P4b).
//!
//! Mirrors `natural_voice` (Kokoro) with its own state: `PhononCoordinator` and `PhononHealth`
//! are managed on macOS only, so off macOS the status reports unsupported and not installed,
//! and every other command refuses. The platform-neutral cores below hold the logic and are
//! unit tested; the Tauri wrappers only look up state and spawn the install. Events:
//! `phonon-voice-state` (progress, OK finish, cancel) and `phonon-voice-error` (other failures).
//! Never log speech text, URLs or error bodies.

use crate::commands::natural_voice::{
    build_status, error_message, phonon_failure_message, NaturalVoiceDownloadFinished,
    NaturalVoiceDownloadProgress, NaturalVoiceStatus,
};
use crate::managers::auto_lang::pick_lid_model;
use crate::managers::indic_stt_core::{indic_pack_status, pack_dir, IndicPackStatus};
use crate::managers::model::{ModelInfo, ModelManager};
use crate::managers::natural_engine::{FailureReason, HealthState, NaturalHealth};
use crate::managers::natural_model::{DownloadProgress, ModelCoordinator, ModelError, ModelState};
use crate::managers::natural_tamil::{
    tamil_failure_message, tamil_voice_status_for, TamilVoiceStatus,
};
use crate::managers::transcription::{lid_candidates_from, TranscriptionManager};
use crate::managers::tts::TtsManager;
use crate::settings::get_settings;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event for progress ticks, the OK finish and a cancel (a state change, not an error).
pub const STATE_EVENT: &str = "phonon-voice-state";
/// Event for every other failure. Its payload is the same finished shape with `ok: false`.
pub const ERROR_EVENT: &str = "phonon-voice-error";

const UNAVAILABLE: &str = "Phonon voice is not available";

/// Event sink: `(event name, JSON payload)`.
pub type PhononEmit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Tauri state: the Phonon model coordinator (macOS only).
pub struct PhononCoordinator(pub Arc<ModelCoordinator>);

/// Tauri state: the Phonon engine health. Its own type, because `Arc<NaturalHealth>` is
/// already managed for Kokoro and Tauri state is keyed by type.
pub struct PhononHealth(pub Arc<NaturalHealth>);

/// Tauri state: the Tamil engine health. Its own type, like `PhononHealth`: the router's Tamil
/// route holds the same handle.
pub struct TamilHealth(pub Arc<NaturalHealth>);

/// Tauri state: the folder of the Tamil pack (`tts_models/pocket-tts-tamil-int4`).
pub struct TamilModelDir(pub PathBuf);

fn is_busy(state: ModelState) -> bool {
    !matches!(state, ModelState::NotInstalled | ModelState::Ready)
}

fn finished_payload(ok: bool, message: Option<String>) -> Value {
    serde_json::to_value(NaturalVoiceDownloadFinished { ok, message }).unwrap_or(Value::Null)
}

fn progress_payload(progress: DownloadProgress) -> Value {
    serde_json::to_value(NaturalVoiceDownloadProgress {
        downloaded: progress.downloaded,
        total: progress.total,
    })
    .unwrap_or(Value::Null)
}

/// Sink for the Phonon health (`NaturalHealth::set_event_sink`). Each applied failure emits one
/// `phonon-voice-error` whose payload is the finished shape with `ok: false` and a user-safe
/// message. Only Phonon failures reach this sink; Kokoro keeps its own sink.
pub fn phonon_failure_sink(emit: PhononEmit) -> Box<dyn Fn(FailureReason) + Send + Sync> {
    Box::new(move |reason| {
        let message = phonon_failure_message(reason);
        emit(ERROR_EVENT, finished_payload(false, Some(message)));
    })
}

/// Pure status for the Phonon row: model state and progress from the coordinator, health from
/// the Phonon health.
pub fn phonon_voice_status_core(
    supported: bool,
    coordinator: &ModelCoordinator,
    health: &NaturalHealth,
) -> NaturalVoiceStatus {
    build_status(
        supported,
        coordinator.state(),
        coordinator.progress(),
        health.state(),
    )
}

/// Installs the Phonon model and reports through `emit`. A busy coordinator is refused with
/// nothing emitted. A success bumps the Phonon health once. A cancel is reported on the state
/// event; every other failure on the error event.
pub async fn phonon_voice_install_core(
    coordinator: Arc<ModelCoordinator>,
    health: Arc<NaturalHealth>,
    emit: PhononEmit,
) -> Result<(), String> {
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let progress_emit = emit.clone();
    let on_progress = move |progress: DownloadProgress| {
        progress_emit(STATE_EVENT, progress_payload(progress));
    };
    match coordinator.download(&on_progress).await {
        Ok(()) => {
            health.bump();
            emit(STATE_EVENT, finished_payload(true, None));
            Ok(())
        }
        Err(error) => {
            let message = error_message(&error);
            log::warn!("phonon voice: model download did not complete");
            let event = if matches!(error, ModelError::Cancelled) {
                STATE_EVENT
            } else {
                ERROR_EVENT
            };
            emit(event, finished_payload(false, Some(message.clone())));
            Err(message)
        }
    }
}

/// Installs the Phonon model from a local folder and reports through `emit`. A busy coordinator
/// is refused with nothing emitted. A success emits the finished OK state and bumps the Phonon
/// health once. A failure is reported on the error event with the user text. The source path is
/// never put in a payload or a log line.
pub fn phonon_voice_import_core(
    coordinator: Arc<ModelCoordinator>,
    health: Arc<NaturalHealth>,
    path: &Path,
    emit: PhononEmit,
) -> Result<(), String> {
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    match coordinator.import_folder(path) {
        Ok(()) => {
            health.bump();
            emit(STATE_EVENT, finished_payload(true, None));
            Ok(())
        }
        Err(error) => {
            let message = error_message(&error);
            log::warn!("phonon voice: folder import did not complete");
            emit(ERROR_EVENT, finished_payload(false, Some(message.clone())));
            Err(message)
        }
    }
}

/// Cancels a running Phonon download. Always Ok.
pub fn phonon_voice_cancel_core(coordinator: &ModelCoordinator) -> Result<(), String> {
    coordinator.cancel();
    Ok(())
}

/// Stops speech, removes the Phonon model, then bumps the Phonon health once.
pub fn phonon_voice_delete_core(
    coordinator: &ModelCoordinator,
    health: &NaturalHealth,
    stop_speech: &dyn Fn(),
) -> Result<(), String> {
    stop_speech();
    coordinator.delete().map_err(|e| error_message(&e))?;
    health.bump();
    Ok(())
}

fn coordinator(app: &AppHandle) -> Option<Arc<ModelCoordinator>> {
    app.try_state::<PhononCoordinator>()
        .map(|state| state.inner().0.clone())
}

fn health(app: &AppHandle) -> Option<Arc<NaturalHealth>> {
    app.try_state::<PhononHealth>()
        .map(|state| state.inner().0.clone())
}

#[tauri::command]
#[specta::specta]
pub fn phonon_voice_status(app: AppHandle) -> Result<NaturalVoiceStatus, String> {
    let supported = cfg!(target_os = "macos");
    match (coordinator(&app), health(&app)) {
        (Some(coordinator), Some(health)) => {
            Ok(phonon_voice_status_core(supported, &coordinator, &health))
        }
        _ => Ok(build_status(
            false,
            ModelState::NotInstalled,
            DownloadProgress::default(),
            HealthState::Ready,
        )),
    }
}

/// Starts the Phonon download in the background and returns at once. Progress and the result
/// arrive as events.
#[tauri::command]
#[specta::specta]
pub async fn phonon_voice_install(app: AppHandle) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(UNAVAILABLE.to_string());
    }
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let emit_app = app.clone();
    let emit: PhononEmit = Arc::new(move |name: &str, payload: Value| {
        let _ = emit_app.emit(name, payload);
    });
    tauri::async_runtime::spawn(async move {
        let _ = phonon_voice_install_core(coordinator, health, emit).await;
    });
    Ok(())
}

/// Starts the Phonon import from `path` (a folder picked by the user) on a blocking task and
/// returns at once. The result arrives as events, like the install.
#[tauri::command]
#[specta::specta]
pub fn phonon_voice_import_folder(app: AppHandle, path: String) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(UNAVAILABLE.to_string());
    }
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    if is_busy(coordinator.state()) {
        return Err(error_message(&ModelError::Busy));
    }
    let emit_app = app.clone();
    let emit: PhononEmit = Arc::new(move |name: &str, payload: Value| {
        let _ = emit_app.emit(name, payload);
    });
    let source = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        let _ = phonon_voice_import_core(coordinator, health, &source, emit);
    });
    Ok(())
}

/// Status of the Tamil voice for the settings row. The pack is checked by stat only, the engine
/// health comes from managed state, and `enabled` is the live setting. Without managed state the
/// status reports the pack as not installed.
#[tauri::command]
#[specta::specta]
pub fn tamil_voice_status(app: AppHandle) -> TamilVoiceStatus {
    let dir = app
        .try_state::<TamilModelDir>()
        .map(|state| state.inner().0.clone())
        .unwrap_or_default();
    let engine_error =
        app.try_state::<TamilHealth>()
            .and_then(|state| match state.inner().0.state() {
                HealthState::Ready => None,
                HealthState::Failed(reason) => Some(tamil_failure_message(reason)),
            });
    let enabled = get_settings(&app).tts_tamil_enabled;
    tamil_voice_status_for(&dir, enabled, engine_error)
}

/// Status of the Tamil speech recognition pack for the settings row (the `tamil_stt_status`
/// command). `ready` is the pack verified and no engine failure; `enabled` is the
/// `stt_tamil_enabled` setting. The UI combines the two.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq, specta::Type)]
pub struct TamilSttStatus {
    /// The four pack files are present with their exact sizes.
    pub installed: bool,
    /// Installed and the Tamil speech engine has not failed.
    pub ready: bool,
    /// The `stt_tamil_enabled` setting.
    pub enabled: bool,
    /// The pack folder.
    pub dir: String,
    /// File names absent from the pack (empty when installed).
    pub missing: Vec<String>,
    /// The first file present with the wrong size, or none.
    pub wrong_size: Option<String>,
    /// User-safe message of the last engine failure, or none.
    pub engine_error: Option<String>,
}

/// Pure status of the Tamil speech pack under `app_data_dir` (stat only), the live `enabled`
/// setting and the engine failure message, if any.
pub fn tamil_stt_status_for(
    app_data_dir: &Path,
    enabled: bool,
    engine_error: Option<String>,
) -> TamilSttStatus {
    let dir = pack_dir(app_data_dir);
    let (installed, missing, wrong_size) = match indic_pack_status(&dir) {
        IndicPackStatus::Ready => (true, Vec::new(), None),
        IndicPackStatus::Missing(names) => (false, names, None),
        IndicPackStatus::WrongSize { file, .. } => (false, Vec::new(), Some(file)),
    };
    TamilSttStatus {
        installed,
        ready: installed && engine_error.is_none(),
        enabled,
        dir: dir.display().to_string(),
        missing,
        wrong_size,
        engine_error,
    }
}

/// Status of the Tamil speech recognition pack for the settings row. The pack is checked by
/// stat only; the engine failure comes from the transcription manager and `enabled` is the live
/// setting.
#[tauri::command]
#[specta::specta]
pub fn tamil_stt_status(app: AppHandle) -> TamilSttStatus {
    let app_data = crate::portable::app_data_dir(&app).unwrap_or_default();
    let engine_error = app
        .try_state::<Arc<TranscriptionManager>>()
        .and_then(|manager| manager.tamil_stt_engine_error());
    let enabled = get_settings(&app).stt_tamil_enabled;
    tamil_stt_status_for(&app_data, enabled, engine_error)
}

/// Status of the auto language route for the settings row (the `auto_language_status` command).
/// `enabled` is the opt-in `stt_auto_language_enabled` setting. `lid_model_id` and
/// `lid_model_name` name the multilingual Whisper the route would use (none when no eligible
/// model is installed). `lid_loaded` is whether that model is loaded now. `tamil_pack_ready` is
/// `stt_tamil_enabled` with the Tamil speech pack ready.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct AutoLanguageStatus {
    /// The `stt_auto_language_enabled` setting.
    pub enabled: bool,
    /// Id of the multilingual Whisper the route would use, or none.
    pub lid_model_id: Option<String>,
    /// Display name of that model, or none.
    pub lid_model_name: Option<String>,
    /// The language-check model is loaded.
    pub lid_loaded: bool,
    /// `stt_tamil_enabled` and the Tamil speech pack is ready.
    pub tamil_pack_ready: bool,
}

/// Pure status of the auto language route. The model used is the one `pick_lid_model` chooses
/// from the eligible candidates of `models` (the active model is skipped, as at run time).
fn auto_language_status_from(
    enabled: bool,
    models: &[ModelInfo],
    active_id: &str,
    lid_loaded: bool,
    tamil_pack_ready: bool,
) -> AutoLanguageStatus {
    let chosen = pick_lid_model(&lid_candidates_from(models, active_id));
    let lid_model_name = chosen
        .as_ref()
        .and_then(|id| models.iter().find(|model| &model.id == id))
        .map(|model| model.name.clone());
    AutoLanguageStatus {
        enabled,
        lid_model_id: chosen,
        lid_model_name,
        lid_loaded,
        tamil_pack_ready,
    }
}

/// Status of the auto language route for the settings row. Reads the settings, the installed
/// models, the language-check model state and the Tamil pack. Read only.
#[tauri::command]
#[specta::specta]
pub fn auto_language_status(
    app: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    transcription: State<'_, Arc<TranscriptionManager>>,
) -> AutoLanguageStatus {
    let settings = get_settings(&app);
    let app_data = crate::portable::app_data_dir(&app).unwrap_or_default();
    let tamil = tamil_stt_status_for(
        &app_data,
        settings.stt_tamil_enabled,
        transcription.tamil_stt_engine_error(),
    );
    auto_language_status_from(
        settings.stt_auto_language_enabled,
        &model_manager.get_available_models(),
        &settings.selected_model,
        transcription.lid_loaded(),
        settings.stt_tamil_enabled && tamil.ready,
    )
}

#[tauri::command]
#[specta::specta]
pub fn phonon_voice_cancel(app: AppHandle) -> Result<(), String> {
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    phonon_voice_cancel_core(&coordinator)
}

/// Stops speech (through the shared TTS manager), removes the Phonon model and bumps health.
#[tauri::command]
#[specta::specta]
pub fn phonon_voice_delete(app: AppHandle) -> Result<(), String> {
    let coordinator = coordinator(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let health = health(&app).ok_or_else(|| UNAVAILABLE.to_string())?;
    let stop_speech = || {
        if let Some(tts) = app.try_state::<Arc<TtsManager>>() {
            let _ = tts.stop();
        }
    };
    phonon_voice_delete_core(&coordinator, &health, &stop_speech)
}

#[cfg(test)]
mod stt_status_tests {
    use super::{tamil_stt_status_for, TamilSttStatus};
    use crate::managers::indic_stt_core::pack_dir;
    use std::fs::{self, File};
    use std::path::Path;
    use tempfile::TempDir;

    /// Pinned pack files and sizes (the same values `indic_stt_core` checks).
    const PACK: [(&str, u64); 4] = [
        ("encoder.onnx", 652_623_111),
        ("ctc_decoder.onnx", 23_095_918),
        ("vocab.json", 96_743),
        ("language_masks.json", 862_011),
    ];

    fn write_pack(dir: &Path) {
        fs::create_dir_all(dir).expect("create pack dir");
        for (name, size) in PACK {
            let file = File::create(dir.join(name)).expect("create pack file");
            file.set_len(size).expect("set size");
        }
    }

    fn all_names() -> Vec<String> {
        PACK.iter().map(|(name, _)| name.to_string()).collect()
    }

    #[test]
    fn ready_pack_with_no_engine_error_is_ready() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = pack_dir(tmp.path());
        write_pack(&dir);
        let status = tamil_stt_status_for(tmp.path(), true, None);
        assert!(status.installed);
        assert!(status.ready);
        assert!(status.enabled);
        assert!(status.missing.is_empty());
        assert_eq!(status.wrong_size, None);
        assert_eq!(status.engine_error, None);
    }

    #[test]
    fn empty_folder_reports_every_file_missing() {
        let tmp = TempDir::new().expect("temp dir");
        let status = tamil_stt_status_for(tmp.path(), true, None);
        assert!(!status.installed);
        assert!(!status.ready);
        assert_eq!(status.missing, all_names());
        assert_eq!(status.wrong_size, None);
    }

    #[test]
    fn wrong_size_file_is_named_and_not_installed() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = pack_dir(tmp.path());
        write_pack(&dir);
        let encoder = File::create(dir.join("encoder.onnx")).expect("truncate encoder");
        encoder.set_len(10).expect("set short size");
        let status = tamil_stt_status_for(tmp.path(), true, None);
        assert!(!status.installed);
        assert!(!status.ready);
        assert!(status.missing.is_empty());
        assert_eq!(status.wrong_size.as_deref(), Some("encoder.onnx"));
    }

    #[test]
    fn disabled_setting_is_reported_and_does_not_change_ready() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = pack_dir(tmp.path());
        write_pack(&dir);
        let status = tamil_stt_status_for(tmp.path(), false, None);
        assert!(!status.enabled);
        assert!(status.ready);
        assert!(status.installed);
    }

    #[test]
    fn engine_error_makes_an_installed_pack_not_ready() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = pack_dir(tmp.path());
        write_pack(&dir);
        let status =
            tamil_stt_status_for(tmp.path(), true, Some("Tamil speech pack failed".into()));
        assert!(status.installed);
        assert!(!status.ready);
        assert_eq!(
            status.engine_error.as_deref(),
            Some("Tamil speech pack failed")
        );
    }

    #[test]
    fn status_reports_the_pack_folder() {
        let tmp = TempDir::new().expect("temp dir");
        let dir = pack_dir(tmp.path());
        let status: TamilSttStatus = tamil_stt_status_for(tmp.path(), true, None);
        assert_eq!(status.dir, dir.display().to_string());
    }
}

#[cfg(test)]
mod auto_language_status_tests {
    use super::{auto_language_status_from, AutoLanguageStatus};
    use crate::managers::auto_lang::pick_lid_model;
    use crate::managers::model::{EngineType, ModelInfo, ModelSource};
    use crate::managers::transcription::lid_candidates_from;

    fn model(
        id: &str,
        filename: &str,
        engine: EngineType,
        downloaded: bool,
        languages: &[&str],
        detects: bool,
    ) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: format!("Display {id}"),
            description: String::new(),
            filename: filename.to_string(),
            source: ModelSource::Local,
            size_mb: 100,
            is_downloaded: downloaded,
            is_downloading: false,
            partial_size: 0,
            is_directory: false,
            engine_type: engine,
            accuracy_score: 0.5,
            speed_score: 0.5,
            supports_translation: false,
            is_recommended: false,
            supported_languages: languages.iter().map(|s| s.to_string()).collect(),
            supports_language_selection: true,
            is_custom: false,
            supports_streaming: false,
            supports_language_detection: detects,
        }
    }

    /// A downloaded, multilingual Whisper of the given size.
    fn whisper(id: &str, size_mb: u64) -> ModelInfo {
        let mut m = model(
            id,
            &format!("ggml-{id}.bin"),
            EngineType::TranscribeCpp,
            true,
            &["en", "hi", "ta"],
            true,
        );
        m.size_mb = size_mb;
        m
    }

    #[test]
    fn no_models_means_no_lid_model() {
        let status = auto_language_status_from(true, &[], "", false, false);
        assert_eq!(status.lid_model_id, None);
        assert_eq!(status.lid_model_name, None);
    }

    #[test]
    fn one_eligible_whisper_is_chosen_with_its_display_name() {
        let models = [whisper("whisper-small", 466)];
        let status = auto_language_status_from(true, &models, "other-model", false, false);
        assert_eq!(status.lid_model_id.as_deref(), Some("whisper-small"));
        assert_eq!(
            status.lid_model_name.as_deref(),
            Some("Display whisper-small")
        );
    }

    #[test]
    fn two_eligible_whispers_pick_the_same_one_as_pick_lid_model() {
        let models = [whisper("whisper-small", 466), whisper("whisper-tiny", 75)];
        let expected = pick_lid_model(&lid_candidates_from(&models, ""));
        let status = auto_language_status_from(true, &models, "", false, false);
        assert_eq!(status.lid_model_id, expected);
        assert_eq!(status.lid_model_id.as_deref(), Some("whisper-tiny"));
        assert_eq!(
            status.lid_model_name.as_deref(),
            Some("Display whisper-tiny")
        );
    }

    #[test]
    fn active_model_as_only_whisper_follows_pick_lid_model_and_is_not_chosen() {
        // pick_lid_model skips the active candidate (it already detects the language), so the
        // status must match that: no LID model is reported.
        let models = [whisper("whisper-small", 466)];
        let expected = pick_lid_model(&lid_candidates_from(&models, "whisper-small"));
        assert_eq!(expected, None);
        let status = auto_language_status_from(true, &models, "whisper-small", false, false);
        assert_eq!(status.lid_model_id, None);
        assert_eq!(status.lid_model_name, None);
    }

    #[test]
    fn enabled_loaded_and_pack_flags_pass_through_verbatim() {
        let models = [whisper("whisper-small", 466)];
        let status = auto_language_status_from(false, &models, "", true, true);
        assert!(!status.enabled);
        assert!(status.lid_loaded);
        assert!(status.tamil_pack_ready);

        let status = auto_language_status_from(true, &models, "", false, false);
        assert!(status.enabled);
        assert!(!status.lid_loaded);
        assert!(!status.tamil_pack_ready);
    }

    #[test]
    fn english_only_parakeet_and_not_downloaded_models_are_excluded() {
        let english_only = model(
            "whisper-base.en",
            "ggml-whisper-base.en.bin",
            EngineType::TranscribeCpp,
            true,
            &["en"],
            true,
        );
        let parakeet = model(
            "parakeet-tdt",
            "parakeet.onnx",
            EngineType::Parakeet,
            true,
            &["en", "hi"],
            true,
        );
        let not_downloaded = model(
            "whisper-medium",
            "ggml-whisper-medium.bin",
            EngineType::TranscribeCpp,
            false,
            &["en", "hi", "ta"],
            true,
        );
        let models = [english_only, parakeet, not_downloaded];
        let status = auto_language_status_from(true, &models, "", false, false);
        assert_eq!(status.lid_model_id, None);
        assert_eq!(status.lid_model_name, None);
    }

    #[test]
    fn json_keys_match_the_bindings_names() {
        let models = [whisper("whisper-small", 466)];
        let status: AutoLanguageStatus = auto_language_status_from(true, &models, "", true, false);
        let value = serde_json::to_value(&status).expect("serialise status");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("status is an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "enabled",
                "lid_loaded",
                "lid_model_id",
                "lid_model_name",
                "tamil_pack_ready"
            ]
        );
        assert_eq!(value["lid_model_id"], "whisper-small");
        assert_eq!(value["lid_model_name"], "Display whisper-small");
        assert_eq!(value["lid_loaded"], true);
    }
}
