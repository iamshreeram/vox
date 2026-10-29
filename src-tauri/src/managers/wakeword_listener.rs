//! Dedicated wake-word microphone capture and arbitration with dictation.

use crate::audio_toolkit::{list_input_devices, AudioRecorder, VadPolicy};
use crate::commands::converse::CONVERSE_ERROR_EVENT;
use crate::helpers::clamshell;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::converse_hooks;
use crate::managers::converse_platform::{DICTATION_BUSY, ENROLLMENT_BUSY};
use crate::managers::converse_runtime::{ConverseController, ALREADY_RUNNING};
use crate::managers::wakeword::{
    ensure_model_downloaded, model_readiness, wakeword_model_dir, BufferingWakeWordEngine,
    HttpModelDownloader, ModelDownloader, ModelReadiness, OpenWakeWordClassifier,
    WakeWordDetection, WakeWordEngine,
};
use crate::settings::{get_settings, AppSettings};
use log::warn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const STATE_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// True iff the wake-word engine should currently be fed audio frames and
/// allowed to classify them: the feature is enabled AND nothing else
/// currently owns the microphone/recording.
pub fn should_feed_wakeword_frame(wake_word_enabled: bool, is_recording: bool) -> bool {
    wake_word_enabled && !is_recording
}

/// True iff a wake-word detection firing right now should be handled (it then
/// starts a conversation). Detections are ignored while any recording owns the mic.
pub fn should_auto_start_recording_on_detection(is_recording: bool) -> bool {
    !is_recording
}

fn configured_engine(
    app_data_dir: &std::path::Path,
    model_name: &str,
    confidence_threshold: f32,
) -> Option<Box<dyn WakeWordEngine>> {
    match model_readiness(app_data_dir, model_name) {
        ModelReadiness::NotDownloaded => {
            warn!("Wake-word model '{model_name}' is not downloaded yet");
            None
        }
        ModelReadiness::Ready => {
            let model_dir = wakeword_model_dir(app_data_dir, model_name);
            match OpenWakeWordClassifier::load_from_dir(&model_dir) {
                Ok(classifier) => Some(Box::new(BufferingWakeWordEngine::new(
                    model_name.to_string(),
                    confidence_threshold,
                    classifier,
                ))),
                Err(error) => {
                    warn!("Failed to load wake-word model '{model_name}': {error}");
                    None
                }
            }
        }
    }
}

/// Download missing files and install a constructed engine into the listener's shared slot.
fn download_and_install_engine(
    app_data_dir: &std::path::Path,
    model_name: &str,
    confidence_threshold: f32,
    downloader: &dyn ModelDownloader,
    engine: &Mutex<Option<Box<dyn WakeWordEngine>>>,
    build_engine: impl FnOnce(&std::path::Path, &str, f32) -> Option<Box<dyn WakeWordEngine>>,
) -> Result<(), String> {
    ensure_model_downloaded(app_data_dir, model_name, downloader)
        .map_err(|error| format!("{error:?}"))?;
    let installed = build_engine(app_data_dir, model_name, confidence_threshold)
        .ok_or_else(|| format!("downloaded wake-word model '{model_name}' could not be loaded"))?;
    *engine.lock().unwrap() = Some(installed);
    Ok(())
}

/// Standalone listener with its own microphone recorder, separate from
/// `AudioRecordingManager`'s hotkey-dictation session recorder.
pub struct WakeWordListener {
    app: AppHandle,
    recorder: Arc<Mutex<AudioRecorder>>,
    engine: Arc<Mutex<Option<Box<dyn WakeWordEngine>>>>,
    enabled: Arc<AtomicBool>,
    monitor_started: AtomicBool,
}

impl WakeWordListener {
    pub fn new(app: AppHandle) -> Self {
        let settings = get_settings(&app);
        let engine = match app.path().app_data_dir() {
            Ok(dir) => configured_engine(
                &dir,
                &settings.wake_word_model_name,
                settings.wake_word_confidence_threshold,
            ),
            Err(error) => {
                warn!("Cannot resolve wake-word model directory: {error}");
                None
            }
        };
        // AudioRecorder::new currently performs no I/O and is infallible; if that changes,
        // handle construction failure explicitly rather than retrying and panicking.
        let recorder = AudioRecorder::new()
            .expect("AudioRecorder::new is currently infallible; see its implementation");
        Self {
            app,
            recorder: Arc::new(Mutex::new(recorder)),
            engine: Arc::new(Mutex::new(engine)),
            enabled: Arc::new(AtomicBool::new(false)),
            monitor_started: AtomicBool::new(false),
        }
    }

    /// Enable listening and launch the low-frequency state observer once.
    pub fn start(&self) {
        self.enabled.store(true, Ordering::SeqCst);
        if self.monitor_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.app.clone();
        let recorder = Arc::clone(&self.recorder);
        let engine = Arc::clone(&self.engine);
        let enabled = Arc::clone(&self.enabled);
        std::thread::spawn(move || {
            let mut download_attempted = false;
            let mut was_recording = false;
            let mut idle_after_recording = None;
            let mut stream_open = false;
            let mut cooldown_completed = false;
            loop {
                std::thread::sleep(STATE_POLL_INTERVAL);
                let feature_enabled =
                    enabled.load(Ordering::SeqCst) && get_settings(&app).wake_word_enabled;
                if !feature_enabled {
                    if stream_open {
                        close_recorder(&recorder);
                        stream_open = false;
                    }
                    if let Some(engine) = engine.lock().unwrap().as_mut() {
                        engine.pause();
                        engine.reset();
                    }
                    was_recording = false;
                    idle_after_recording = None;
                    cooldown_completed = false;
                    // Reset so turning the feature back on retries a previous
                    // download failure instead of staying stuck until the app
                    // restarts (this thread is spawned once for the app's
                    // entire lifetime, not per toggle-on).
                    download_attempted = false;
                    continue;
                }
                let is_recording = app
                    .try_state::<Arc<AudioRecordingManager>>()
                    .is_some_and(|manager| manager.is_recording());
                if is_recording {
                    if stream_open {
                        close_recorder(&recorder);
                        stream_open = false;
                    }
                    if let Some(engine) = engine.lock().unwrap().as_mut() {
                        engine.pause();
                        engine.reset();
                    }
                    was_recording = true;
                    idle_after_recording = None;
                    cooldown_completed = false;
                    continue;
                }

                if was_recording {
                    was_recording = false;
                    idle_after_recording = Some(std::time::Instant::now());
                }
                let settings = get_settings(&app);
                if engine.lock().unwrap().is_none() && !download_attempted {
                    download_attempted = true;
                    let result = app
                        .path()
                        .app_data_dir()
                        .map_err(|error| error.to_string())
                        .and_then(|app_data_dir| {
                            download_and_install_engine(
                                &app_data_dir,
                                &settings.wake_word_model_name,
                                settings.wake_word_confidence_threshold,
                                &HttpModelDownloader::default(),
                                &engine,
                                configured_engine,
                            )
                        });
                    if let Err(error) = result {
                        warn!(
                            "Failed to download/load wake-word model '{}': {error}",
                            settings.wake_word_model_name
                        );
                        let _ = app.emit(
                            "wakeword-model-download-failed",
                            serde_json::json!({ "model_name": settings.wake_word_model_name, "error": error }),
                        );
                    }
                    continue;
                }
                let engine_available = engine.lock().unwrap().is_some();
                if !engine_available {
                    continue;
                }
                if !cooldown_completed {
                    let ready_at = idle_after_recording.get_or_insert_with(std::time::Instant::now);
                    if ready_at.elapsed()
                        < Duration::from_millis(settings.wake_word_cooldown_ms as u64)
                    {
                        continue;
                    }
                    cooldown_completed = true;
                }
                if let Some(engine) = engine.lock().unwrap().as_mut() {
                    engine.resume();
                }
                if !stream_open {
                    if let Err(error) = open_recorder(&app, &recorder, &engine, settings) {
                        warn!("Unable to start wake-word microphone: {error}");
                        // Avoid enumerating/reopening continuously on unavailable devices.
                        std::thread::sleep(Duration::from_secs(2));
                    } else {
                        stream_open = true;
                    }
                }
            }
        });
    }

    /// Disable listening and synchronously close the dedicated input stream.
    /// Safe to call repeatedly, including before the first call to `start`.
    pub fn stop(&self) {
        stop_listener(&self.enabled, &self.recorder, &self.engine);
    }
}

fn stop_listener(
    enabled: &AtomicBool,
    recorder: &Mutex<AudioRecorder>,
    engine: &Mutex<Option<Box<dyn WakeWordEngine>>>,
) {
    enabled.store(false, Ordering::SeqCst);
    close_recorder(recorder);
    if let Some(engine) = engine.lock().unwrap().as_mut() {
        engine.pause();
        engine.reset();
    }
}

fn close_recorder(recorder: &Mutex<AudioRecorder>) {
    if let Err(error) = recorder.lock().unwrap().close() {
        warn!("Failed to close wake-word microphone: {error}");
    }
}

/// Called once, right before a fresh wake-word listening session's
/// microphone stream is opened (i.e. only from `open_recorder`, never from
/// the per-tick polling loop). Pre-consumes the classifier's cold-start
/// "zero guard" window (see `WakeWordEngine::warm_up`'s doc comment) on
/// synthetic silence so real microphone audio -- which may start arriving
/// mid-utterance due to real audio-hardware stream-reopen latency -- is
/// never subject to it. A no-op when no engine is loaded yet (model still
/// downloading/failed to load).
fn prepare_engine_for_fresh_listen(engine: &Mutex<Option<Box<dyn WakeWordEngine>>>) {
    if let Some(engine) = engine.lock().unwrap().as_mut() {
        engine.warm_up();
    }
}

pub(crate) fn resolve_device(settings: &AppSettings) -> Result<Option<cpal::Device>, String> {
    let selected = if let Some(name) = settings.clamshell_microphone.as_ref() {
        if clamshell::is_clamshell().unwrap_or(false) {
            Some(name)
        } else {
            settings.selected_microphone.as_ref()
        }
    } else {
        settings.selected_microphone.as_ref()
    };
    match selected {
        Some(name) => list_input_devices()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|device| &device.name == name)
            .map(|device| Some(device.device))
            .ok_or_else(|| format!("Configured microphone '{name}' is unavailable")),
        None => Ok(None),
    }
}

fn open_recorder(
    app: &AppHandle,
    recorder: &Mutex<AudioRecorder>,
    engine: &Arc<Mutex<Option<Box<dyn WakeWordEngine>>>>,
    settings: AppSettings,
) -> Result<(), String> {
    if !get_settings(app).wake_word_enabled {
        return Err("wake-word setting was disabled before opening microphone".to_string());
    }
    prepare_engine_for_fresh_listen(engine);
    let device = resolve_device(&settings)?;
    let app = app.clone();
    let engine = Arc::clone(engine);
    let detection_pending = Arc::new(AtomicBool::new(false));
    let callback_detection_pending = Arc::clone(&detection_pending);
    let callback_suppressed = Arc::new(AtomicBool::new(false));
    let audio_cb = move |frame: &[f32]| {
        let now_suppressed = wake_frames_suppressed(
            converse_hooks::session_active(),
            converse_hooks::ms_since_session_end(),
        );
        let was_suppressed = callback_suppressed.swap(now_suppressed, Ordering::SeqCst);
        if now_suppressed {
            // Drop the frame: a conversation session (and its tail) owns the audio.
            if should_reset_engine_on_suppression_edge(was_suppressed, now_suppressed) {
                if let Some(wakeword) = engine.lock().unwrap().as_mut() {
                    wakeword.reset();
                }
            }
            return;
        }
        if !should_feed_wakeword_frame(
            get_settings(&app).wake_word_enabled,
            app.try_state::<Arc<AudioRecordingManager>>()
                .is_some_and(|manager| manager.is_recording()),
        ) || callback_detection_pending.load(Ordering::SeqCst)
        {
            return;
        }
        let detection = engine
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|wakeword| wakeword.process_frame(frame));
        if let Some(detection) = detection {
            let recording = app
                .try_state::<Arc<AudioRecordingManager>>()
                .is_some_and(|manager| manager.is_recording());
            if should_auto_start_recording_on_detection(recording)
                && !callback_detection_pending.swap(true, Ordering::SeqCst)
            {
                handle_detection(&app, detection);
                let detection_pending = Arc::clone(&callback_detection_pending);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(3));
                    detection_pending.store(false, Ordering::SeqCst);
                });
            }
        }
    };
    let mut recorder = recorder.lock().unwrap();
    *recorder = AudioRecorder::new()
        .map_err(|error| error.to_string())?
        .with_audio_callback(audio_cb);
    recorder.open(device).map_err(|error| error.to_string())?;
    // `open()` only sets up the cpal stream; it does NOT start draining it
    // into the audio callback -- that requires a separate `start()` call
    // (see `AudioRecorder::start`'s own doc comment). Without this, the
    // real root cause of "wake word never fires": the stream opens
    // successfully (confirmed in logs), the OS happily delivers audio into
    // an internal ring buffer, but nothing ever reads that buffer out to
    // `audio_cb` -- `process_frame` is never called even once, no matter
    // what's said into the microphone. `VadPolicy::Disabled` because this
    // engine does its own windowing/classification on every raw frame; the
    // recorder's own VAD is a filter for a different consumer (dictation)
    // and would silently drop frames this engine needs to see.
    recorder
        .start(VadPolicy::Disabled)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// True while a conversation session is active: a wake-word detection must
/// then neither emit its event nor start a recording (DESIGN.md section 2).
#[cfg_attr(not(test), allow(dead_code))]
fn should_ignore_detection() -> bool {
    crate::managers::converse_hooks::session_active()
}

/// How long after a conversation session ends the detector stays muted, so the
/// assistant's own last words cannot re-trigger the wake word.
pub const WAKE_SESSION_TAIL_MS: u64 = 1000;

/// What one wake-word detection should do. A detection always starts a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeDetectionRoute {
    Ignore,
    StartConversation,
}

/// Pure: a live session always wins; otherwise every detection starts a conversation.
pub fn route_detection(session_active: bool) -> WakeDetectionRoute {
    if session_active {
        return WakeDetectionRoute::Ignore;
    }
    WakeDetectionRoute::StartConversation
}

/// Pure: true while detector input must be dropped -- during a session and for
/// `WAKE_SESSION_TAIL_MS` after it ended.
pub fn wake_frames_suppressed(session_active: bool, ms_since_session_end: Option<u64>) -> bool {
    session_active || matches!(ms_since_session_end, Some(ms) if ms < WAKE_SESSION_TAIL_MS)
}

/// Pure: the detector is reset exactly once, on the rising edge of suppression.
pub fn should_reset_engine_on_suppression_edge(was_suppressed: bool, now_suppressed: bool) -> bool {
    !was_suppressed && now_suppressed
}

/// Start-only seam over `ConverseController::start`, so the policy below is testable.
pub trait ConversationStarter {
    fn start(&self) -> Result<u64, String>;
}

/// Starts a conversation for a wake detection. Calls `starter.start()` exactly once.
/// Busy refusals are silent (a spurious wake must not nag); any other refusal is
/// reported once through `notify_error`.
pub fn start_conversation_from_wake(
    starter: &dyn ConversationStarter,
    notify_error: &mut dyn FnMut(&str),
) -> Option<u64> {
    match starter.start() {
        Ok(id) => Some(id),
        Err(message) => {
            let busy =
                [ALREADY_RUNNING, DICTATION_BUSY, ENROLLMENT_BUSY].contains(&message.as_str());
            if !busy {
                notify_error(&message);
            }
            None
        }
    }
}

/// Real starter: the app's conversation controller. Never toggles, so a racing
/// session can never be stopped by a wake detection.
struct AppConversationStarter(AppHandle);

impl ConversationStarter for AppConversationStarter {
    fn start(&self) -> Result<u64, String> {
        self.0.state::<Arc<ConverseController>>().start()
    }
}

/// Starts the conversation on a short-lived thread so the audio callback never blocks.
fn start_conversation_in_background(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("wake-converse-start".into())
        .spawn(move || {
            let starter = AppConversationStarter(app.clone());
            let mut notify = |message: &str| {
                if app.emit(CONVERSE_ERROR_EVENT, message.to_string()).is_err() {
                    log::debug!("wake-word: converse error event not delivered");
                }
            };
            let _ = start_conversation_from_wake(&starter, &mut notify);
        });
    if let Err(error) = spawned {
        warn!("Failed to spawn wake-word conversation start thread: {error}");
    }
}

fn handle_detection(app: &AppHandle, detection: WakeWordDetection) {
    match route_detection(converse_hooks::session_active()) {
        WakeDetectionRoute::Ignore => {
            log::debug!("wake-word detection ignored: conversation session active");
        }
        WakeDetectionRoute::StartConversation => {
            if let Err(error) = crate::commands::wakeword::emit_wakeword_detection(app, &detection)
            {
                warn!("Failed to emit wake-word detection event: {error}");
            }
            start_conversation_in_background(app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_ignored_only_while_a_session_is_active() {
        let _g = crate::managers::converse_hooks::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::managers::converse_hooks::set_session_active(true);
        assert!(should_ignore_detection());
        crate::managers::converse_hooks::set_session_active(false);
        assert!(!should_ignore_detection());
    }

    #[test]
    fn disabled_and_idle_does_not_feed_frames() {
        assert!(!should_feed_wakeword_frame(false, false));
    }

    #[test]
    fn disabled_while_recording_does_not_feed_frames() {
        assert!(!should_feed_wakeword_frame(false, true));
    }

    #[test]
    fn enabled_while_recording_does_not_feed_frames() {
        assert!(!should_feed_wakeword_frame(true, true));
    }

    #[test]
    fn enabled_and_idle_feeds_frames() {
        assert!(should_feed_wakeword_frame(true, false));
    }

    #[test]
    fn detection_during_recording_does_not_start_another_recording() {
        assert!(!should_auto_start_recording_on_detection(true));
    }

    #[test]
    fn detection_while_idle_starts_a_recording() {
        assert!(should_auto_start_recording_on_detection(false));
    }

    struct TestEngine;

    impl WakeWordEngine for TestEngine {
        fn process_frame(&mut self, _samples: &[f32]) -> Option<WakeWordDetection> {
            None
        }
        fn reset(&mut self) {}
        fn pause(&mut self) {}
        fn resume(&mut self) {}
    }

    /// Tracks `warm_up()` call count without needing a real model/classifier.
    struct SpyWarmUpEngine {
        warm_up_calls: Arc<Mutex<u32>>,
    }

    impl WakeWordEngine for SpyWarmUpEngine {
        fn process_frame(&mut self, _samples: &[f32]) -> Option<WakeWordDetection> {
            None
        }
        fn reset(&mut self) {}
        fn pause(&mut self) {}
        fn resume(&mut self) {}
        fn warm_up(&mut self) {
            *self.warm_up_calls.lock().unwrap() += 1;
        }
    }

    #[test]
    fn prepare_engine_for_fresh_listen_calls_warm_up_exactly_once() {
        let calls = Arc::new(Mutex::new(0u32));
        let engine: Mutex<Option<Box<dyn WakeWordEngine>>> =
            Mutex::new(Some(Box::new(SpyWarmUpEngine {
                warm_up_calls: Arc::clone(&calls),
            })));
        prepare_engine_for_fresh_listen(&engine);
        assert_eq!(
            *calls.lock().unwrap(),
            1,
            "opening a fresh wake-word listening session must warm up the engine exactly once"
        );
    }

    #[test]
    fn prepare_engine_for_fresh_listen_is_a_noop_when_engine_not_yet_loaded() {
        // Model still downloading / failed to load -- must not panic.
        let engine: Mutex<Option<Box<dyn WakeWordEngine>>> = Mutex::new(None);
        prepare_engine_for_fresh_listen(&engine);
    }

    struct FakeModelDownloader;

    impl ModelDownloader for FakeModelDownloader {
        fn download_file(
            &self,
            _model_name: &str,
            _file_name: &str,
            destination: &std::path::Path,
        ) -> Result<(), crate::managers::wakeword::ModelDownloadError> {
            std::fs::write(destination, b"fake model").map_err(|error| {
                crate::managers::wakeword::ModelDownloadError::Io(error.to_string())
            })
        }
    }

    #[test]
    fn missing_model_download_installs_engine_without_reconstructing_listener() {
        let dir = tempfile::tempdir().unwrap();
        let engine: Mutex<Option<Box<dyn WakeWordEngine>>> = Mutex::new(None);
        download_and_install_engine(
            dir.path(),
            "hey_jarvis",
            0.5,
            &FakeModelDownloader,
            &engine,
            |_, _, _| Some(Box::new(TestEngine)),
        )
        .unwrap();
        assert!(engine.lock().unwrap().is_some());
        assert_eq!(
            model_readiness(dir.path(), "hey_jarvis"),
            ModelReadiness::Ready
        );
    }

    #[test]
    fn stop_is_idempotent_without_ever_starting() {
        let enabled = AtomicBool::new(false);
        let recorder = Mutex::new(AudioRecorder::new().unwrap());
        let engine: Mutex<Option<Box<dyn WakeWordEngine>>> = Mutex::new(Some(Box::new(TestEngine)));
        stop_listener(&enabled, &recorder, &engine);
        stop_listener(&enabled, &recorder, &engine);
        assert!(!enabled.load(Ordering::SeqCst));
        assert!(engine.lock().unwrap().is_some());
    }

    #[test]
    fn unavailable_model_is_a_non_fatal_absent_engine() {
        let temp_dir = tempfile::tempdir().unwrap();
        assert!(configured_engine(temp_dir.path(), "not-installed", 0.5).is_none());
    }
}
