//! Dedicated wake-word microphone capture and arbitration with dictation.

use crate::audio_toolkit::{list_input_devices, AudioRecorder, VadPolicy};
use crate::helpers::clamshell;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::wakeword::{
    ensure_model_downloaded, model_readiness, wakeword_model_dir, BufferingWakeWordEngine,
    HttpModelDownloader, ModelDownloader, ModelReadiness, OpenWakeWordClassifier,
    WakeWordDetection, WakeWordEngine,
};
use crate::settings::{get_settings, AppSettings};
use log::warn;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// True iff a wake-word detection firing right now should actually trigger
/// auto-starting a dictation recording.
pub fn should_auto_start_recording_on_detection(is_recording: bool) -> bool {
    !is_recording
}

/// True while a wake-word-triggered recording is in progress and hasn't
/// yet been auto-finished by sustained silence. Reset at the start of
/// EVERY recording regardless of trigger (see `on_recording_started`), so
/// it can never leak into an unrelated later session (e.g. a manual
/// hotkey recording that happens to start right after a wake-word one
/// ends).
static SILENCE_WATCH_ACTIVE: AtomicBool = AtomicBool::new(false);
/// True once at least one non-silent mic-level sample has been observed
/// during the currently-active watch -- i.e. the user has actually
/// started talking. Before this flips true, `INITIAL_GRACE_MS` (not the
/// shorter configured post-speech threshold) applies: a natural pause
/// between finishing the wake phrase and beginning the actual sentence
/// must never be mistaken for "already done talking".
static HAS_VOICED: AtomicBool = AtomicBool::new(false);
/// Epoch ms when the currently-active watch's recording started.
static RECORDING_STARTED_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Epoch ms when the mic was last observed as NOT silent during the
/// currently-active watch. Only meaningful once `HAS_VOICED` is true.
static LAST_VOICED_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Threshold (ms) for the currently-active watch's post-speech silence
/// rule, copied in once at `on_recording_started` time so the hot
/// per-frame level callback never needs to touch settings.
static SILENCE_TIMEOUT_MS: AtomicU64 = AtomicU64::new(0);

/// How long to wait for the user to say ANYTHING at all after a
/// wake-word-triggered recording starts, before giving up even though no
/// speech was ever heard (safety net for a false wake-word trigger where
/// nobody actually talks). Deliberately longer than the configured
/// post-speech pause threshold: reacting to the wake-word confirmation
/// and starting to speak takes longer than a normal mid-sentence breath.
const INITIAL_GRACE_MS: u64 = 4000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Called once at the very start of every recording, regardless of how it
/// was triggered. Only a wake-word-triggered start (`shortcut_str ==
/// "wake_word"`, the exact source string `handle_detection` passes to
/// `send_transcription_input`) arms the silence watch; every other
/// trigger (hotkey, CLI, SIGUSR2, ...) explicitly disarms it. This is the
/// single place that decides "is this session eligible for
/// silence-auto-finish" -- calling it unconditionally on every start (not
/// just wake-word ones) is what guarantees a stale armed watch can never
/// bleed into a later, unrelated manual recording.
pub fn on_recording_started(shortcut_str: &str, silence_timeout_ms: u32) {
    if shortcut_str == "wake_word" && silence_timeout_ms > 0 {
        SILENCE_TIMEOUT_MS.store(silence_timeout_ms as u64, Ordering::Relaxed);
        RECORDING_STARTED_AT_MS.store(now_ms(), Ordering::Relaxed);
        HAS_VOICED.store(false, Ordering::Relaxed);
        LAST_VOICED_AT_MS.store(0, Ordering::Relaxed);
        SILENCE_WATCH_ACTIVE.store(true, Ordering::Relaxed);
    } else {
        SILENCE_WATCH_ACTIVE.store(false, Ordering::Relaxed);
    }
}

/// A mic-level reading counts as silence for auto-finish purposes when
/// every bucket is at or below this floor -- mirrors the near-zero bars a
/// human would visually read as silence in the overlay's own waveform.
/// An empty slice (no buckets at all) also counts as silent.
const SILENCE_LEVEL_FLOOR: f32 = 0.02;

fn is_silent_mic_level(levels: &[f32]) -> bool {
    levels.iter().all(|&v| v <= SILENCE_LEVEL_FLOOR)
}

/// Pure: two-phase auto-finish decision. Before any speech has been heard
/// (`has_voiced == false`), the clock is measured from recording start
/// against `initial_grace_ms`. Once speech has been heard, the clock
/// resets to measure from the last-voiced sample against the (normally
/// shorter) `post_speech_silence_ms`. Either threshold being `0` disables
/// that phase's ability to fire.
fn should_auto_finish(
    has_voiced: bool,
    now_ms: u64,
    recording_started_at_ms: u64,
    last_voiced_at_ms: u64,
    initial_grace_ms: u64,
    post_speech_silence_ms: u64,
) -> bool {
    let (reference, threshold) = if has_voiced {
        (last_voiced_at_ms, post_speech_silence_ms)
    } else {
        (recording_started_at_ms, initial_grace_ms)
    };
    threshold > 0 && now_ms.saturating_sub(reference) >= threshold
}

/// Called on every mic-level sample while ANY recording is active (cheap
/// no-op via the atomic load unless a wake-word silence watch is
/// currently armed -- zero behavior change for hotkey recordings). Once
/// enough silence has elapsed (per the two-phase `should_auto_finish`
/// policy), re-sends the exact same external "press" signal a real
/// second hotkey press would send to end a locked wake-word session --
/// this reuses the already-correct, already-tested stop path in
/// `TranscriptionCoordinator` instead of touching recording state
/// directly.
pub fn check_silence_and_maybe_finish(app: &AppHandle, levels: &[f32]) {
    if !SILENCE_WATCH_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let now = now_ms();
    if is_silent_mic_level(levels) {
        let has_voiced = HAS_VOICED.load(Ordering::Relaxed);
        let finish = should_auto_finish(
            has_voiced,
            now,
            RECORDING_STARTED_AT_MS.load(Ordering::Relaxed),
            LAST_VOICED_AT_MS.load(Ordering::Relaxed),
            INITIAL_GRACE_MS,
            SILENCE_TIMEOUT_MS.load(Ordering::Relaxed),
        );
        if finish {
            // Disarm first so a slow stop can't cause a double-fire from a
            // later call on this same still-silent recording.
            SILENCE_WATCH_ACTIVE.store(false, Ordering::Relaxed);
            crate::signal_handle::send_transcription_input(
                app,
                "transcribe",
                "wake_word_silence_timeout",
            );
        }
    } else {
        HAS_VOICED.store(true, Ordering::Relaxed);
        LAST_VOICED_AT_MS.store(now, Ordering::Relaxed);
    }
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

fn resolve_device(settings: &AppSettings) -> Result<Option<cpal::Device>, String> {
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
    let audio_cb = move |frame: &[f32]| {
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

fn handle_detection(app: &AppHandle, detection: WakeWordDetection) {
    if let Err(error) = crate::commands::wakeword::emit_wakeword_detection(app, &detection) {
        warn!("Failed to emit wake-word detection event: {error}");
    }
    crate::signal_handle::send_transcription_input(app, "transcribe", "wake_word");
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let engine: Mutex<Option<Box<dyn WakeWordEngine>>> = Mutex::new(Some(Box::new(
            SpyWarmUpEngine {
                warm_up_calls: Arc::clone(&calls),
            },
        )));
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

    // ---- wake-word silence auto-finish (pure helpers) ----

    #[test]
    fn silence_all_buckets_at_or_below_floor_is_silent() {
        assert!(is_silent_mic_level(&[0.0, 0.02, 0.0, 0.01]));
    }

    #[test]
    fn silence_empty_levels_counts_as_silent() {
        assert!(is_silent_mic_level(&[]));
    }

    #[test]
    fn silence_any_bucket_above_floor_is_not_silent() {
        assert!(!is_silent_mic_level(&[0.0, 0.0, 0.03, 0.0]));
    }

    #[test]
    fn auto_finish_before_post_speech_threshold_is_false() {
        assert!(!should_auto_finish(true, 1499, 0, 0, 4000, 1500));
    }

    #[test]
    fn auto_finish_at_post_speech_threshold_is_true() {
        assert!(should_auto_finish(true, 1500, 0, 0, 4000, 1500));
    }

    #[test]
    fn auto_finish_past_post_speech_threshold_is_true() {
        assert!(should_auto_finish(true, 5000, 0, 0, 4000, 1500));
    }

    #[test]
    fn auto_finish_is_always_false_when_post_speech_threshold_is_zero_sentinel() {
        // threshold_ms == 0 means the feature is disabled -- never fires,
        // even with a very long silence.
        assert!(!should_auto_finish(true, 999_999, 0, 0, 4000, 0));
    }

    #[test]
    fn auto_finish_waiting_for_first_speech_uses_initial_grace_not_post_speech_threshold() {
        // Not yet voiced, 1600ms since recording start -- that's PAST the
        // 1500ms post-speech threshold but well under the 4000ms initial
        // grace. Must NOT finish: this is the exact regression being
        // fixed, where a brief natural pause before the user starts
        // talking was wrongly treated as "already done talking".
        assert!(!should_auto_finish(false, 1600, 0, 0, 4000, 1500));
    }

    #[test]
    fn auto_finish_fires_after_initial_grace_elapses_with_no_speech_ever() {
        // Safety net for a false wake-word trigger where nobody actually
        // speaks: still finishes eventually, on the longer grace clock.
        assert!(should_auto_finish(false, 4000, 0, 0, 4000, 1500));
    }

    #[test]
    fn auto_finish_uses_last_voiced_as_reference_once_speech_has_happened() {
        // Recording started at t=0, first (and last) voice heard at
        // t=10_000 (a long intro before the pause) -- the post-speech
        // clock must count from the last-voiced time, not from recording
        // start (which would have already exceeded the threshold long
        // ago).
        assert!(!should_auto_finish(true, 11_000, 0, 10_000, 4000, 1500)); // only 1000ms since last voice
        assert!(should_auto_finish(true, 11_500, 0, 10_000, 4000, 1500)); // 1500ms since last voice
    }
}
