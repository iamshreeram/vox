//! Dedicated wake-word microphone capture and arbitration with dictation.

use crate::audio_toolkit::{list_input_devices, AudioRecorder};
use crate::helpers::clamshell;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::wakeword::{
    model_readiness, wakeword_model_dir, BufferingWakeWordEngine, ModelReadiness,
    OpenWakeWordClassifier, WakeWordDetection, WakeWordEngine,
};
use crate::settings::{get_settings, AppSettings};
use log::warn;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};

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

fn configured_engine(
    app_data_dir: &std::path::Path,
    model_name: &str,
    confidence_threshold: f32,
) -> Option<Box<dyn WakeWordEngine>> {
    match model_readiness(app_data_dir, model_name) {
        ModelReadiness::NotDownloaded => {
            warn!("Wake-word model '{model_name}' is not downloaded; wake-word listening will remain unavailable until the model is installed");
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
                if !cooldown_completed {
                    let ready_at = idle_after_recording.get_or_insert_with(std::time::Instant::now);
                    if ready_at.elapsed()
                        < Duration::from_millis(settings.wake_word_cooldown_ms as u64)
                    {
                        continue;
                    }
                    cooldown_completed = true;
                }
                let engine_available = engine.lock().unwrap().is_some();
                if !engine_available {
                    continue;
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
