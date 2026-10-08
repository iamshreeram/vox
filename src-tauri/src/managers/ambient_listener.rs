//! Dedicated ambient microphone capture and VAD-silence segmented transcription.
//!
//! Transcript content stays in RAM: it is never logged, serialized, or written
//! to disk. Audio segments are handed directly to the existing transcriber.

use crate::audio_toolkit::vad::{
    frames_for_duration_ms, SmoothedVad, VAD_ONSET_MS, VAD_PREFILL_MS,
};
use crate::audio_toolkit::{list_input_devices, AudioRecorder, EarshotVad, VadPolicy};
use crate::helpers::clamshell;
use crate::managers::ambient::{AmbientCoordinator, Engagement};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, AppSettings};
use log::warn;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const STATE_POLL_INTERVAL: Duration = Duration::from_millis(150);
// VAD-approved speech frames separated by 550 ms of silence form a segment.
const SEGMENT_SILENCE_MS: u64 = 550;
const VAD_HANGOVER_MS: u64 = 320;
const VAD_THRESHOLD: f32 = 0.5;

pub fn should_run_ambient_capture(ambient_mode_enabled: bool, is_recording: bool) -> bool {
    ambient_mode_enabled && !is_recording
}

pub struct AmbientListener {
    app: AppHandle,
    recorder: Arc<Mutex<AudioRecorder>>,
    enabled: Arc<AtomicBool>,
    last_speech_ms: Arc<AtomicU64>,
    monitor_started: AtomicBool,
}

impl AmbientListener {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            recorder: Arc::new(Mutex::new(
                AudioRecorder::new().expect("AudioRecorder::new is infallible"),
            )),
            enabled: Arc::new(AtomicBool::new(false)),
            last_speech_ms: Arc::new(AtomicU64::new(monotonic_ms())),
            monitor_started: AtomicBool::new(false),
        }
    }

    pub fn start(&self) {
        self.enabled.store(true, Ordering::SeqCst);
        if self.monitor_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.app.clone();
        let recorder = Arc::clone(&self.recorder);
        let enabled = Arc::clone(&self.enabled);
        let last_speech_ms = Arc::clone(&self.last_speech_ms);
        let callback_last_speech_ms = Arc::clone(&last_speech_ms);
        std::thread::spawn(move || {
            let mut stream_open = false;
            let mut session_active = false;
            loop {
                std::thread::sleep(STATE_POLL_INTERVAL);
                let settings = get_settings(&app);
                let dictation_recording = app
                    .try_state::<Arc<AudioRecordingManager>>()
                    .is_some_and(|manager| manager.is_recording());
                // Settings mutual exclusion is authoritative; this extra check
                // guards the transition before both listener loops observe it.
                let should_capture = enabled.load(Ordering::SeqCst)
                    && should_run_ambient_capture(
                        settings.ambient_mode_enabled,
                        dictation_recording,
                    )
                    && !settings.wake_word_enabled;
                if !should_capture {
                    if session_active {
                        let audio = stop_session(&recorder);
                        session_active = false;
                        transcribe_segment(&app, audio);
                    }
                    if stream_open {
                        close_recorder(&recorder);
                        stream_open = false;
                    }
                    continue;
                }
                if !stream_open {
                    match open_recorder(
                        &app,
                        &recorder,
                        settings,
                        Arc::clone(&callback_last_speech_ms),
                    ) {
                        Ok(()) => stream_open = true,
                        Err(error) => {
                            warn!("Unable to start ambient microphone: {error}");
                            std::thread::sleep(Duration::from_secs(2));
                            continue;
                        }
                    }
                }
                if !session_active {
                    match start_session(&recorder) {
                        Ok(()) => {
                            session_active = true;
                            last_speech_ms.store(monotonic_ms(), Ordering::SeqCst);
                        }
                        Err(error) => warn!("Unable to start ambient VAD session: {error}"),
                    }
                }
                if session_active
                    && monotonic_ms().saturating_sub(last_speech_ms.load(Ordering::SeqCst))
                        >= SEGMENT_SILENCE_MS
                {
                    let audio = stop_session(&recorder);
                    session_active = false;
                    transcribe_segment(&app, audio);
                    last_speech_ms.store(monotonic_ms(), Ordering::SeqCst);
                }
            }
        });
    }

    pub fn stop(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        let mut recorder = self.recorder.lock().unwrap();
        if let Err(error) = recorder.close() {
            warn!("Failed to close ambient microphone: {error}");
        }
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
    settings: AppSettings,
    last_speech_ms: Arc<AtomicU64>,
) -> Result<(), String> {
    let current = get_settings(app);
    if !current.ambient_mode_enabled || current.wake_word_enabled {
        return Err("ambient mode is disabled or wake-word mode is enabled".to_string());
    }
    let device = resolve_device(&settings)?;
    let vad = SmoothedVad::new(
        Box::new(EarshotVad::new(VAD_THRESHOLD).map_err(|error| error.to_string())?),
        frames_for_duration_ms(VAD_PREFILL_MS.min(160), 256),
        frames_for_duration_ms(VAD_HANGOVER_MS, 256),
        frames_for_duration_ms(VAD_ONSET_MS, 256),
    );
    let mut fresh_recorder = AudioRecorder::new()
        .map_err(|error| error.to_string())?
        .with_vad(
            Box::new(vad),
            frames_for_duration_ms(VAD_HANGOVER_MS, 256),
            frames_for_duration_ms(VAD_HANGOVER_MS, 256),
        )
        .with_audio_callback(move |_| {
            last_speech_ms.store(monotonic_ms(), Ordering::SeqCst);
        });
    fresh_recorder
        .open(device)
        .map_err(|error| error.to_string())?;
    *recorder.lock().unwrap() = fresh_recorder;
    Ok(())
}

fn start_session(recorder: &Mutex<AudioRecorder>) -> Result<(), String> {
    recorder
        .lock()
        .unwrap()
        .start(VadPolicy::Offline)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn stop_session(recorder: &Mutex<AudioRecorder>) -> Vec<f32> {
    match recorder.lock().unwrap().stop() {
        Ok(audio) => audio,
        Err(error) => {
            warn!("Unable to finish ambient audio segment: {error}");
            Vec::new()
        }
    }
}

fn close_recorder(recorder: &Mutex<AudioRecorder>) {
    if let Err(error) = recorder.lock().unwrap().close() {
        warn!("Failed to close ambient microphone: {error}");
    }
}

fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn transcribe_segment(app: &AppHandle, audio: Vec<f32>) {
    if audio.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(transcriber) = app.try_state::<Arc<TranscriptionManager>>() else {
            return;
        };
        match transcriber.transcribe(audio) {
            Ok(text) if !text.trim().is_empty() => {
                if let Some(coordinator) = app.try_state::<Arc<AmbientCoordinator>>() {
                    process_transcribed_segment(&app, &coordinator, &text, wall_clock_ms());
                }
            }
            Ok(_) => {}
            Err(error) => warn!("Ambient transcription failed: {error}"),
        }
    });
}

fn wall_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// Phase 4 TTS is absent; this deliberately always-false signal has no TTS dependency.
fn tts_is_speaking() -> bool {
    false
}

fn evaluate_ambient_segment(
    coordinator: &AmbientCoordinator,
    text: &str,
    now_ms: u64,
) -> Option<String> {
    let during_tts = coordinator.echo.is_during_tts(tts_is_speaking());
    coordinator
        .transcript
        .add_segment(text, now_ms, now_ms, 1.0, during_tts);
    let recent = coordinator.transcript.recent_segments(now_ms, Some(5_000));
    if recent.iter().any(|segment| segment.during_tts) {
        return None;
    }
    if let Engagement::Addressed { extracted_request } = coordinator.judge.evaluate(&recent) {
        coordinator.transcript.clear();
        Some(extracted_request)
    } else {
        None
    }
}

fn process_transcribed_segment(
    app: &AppHandle,
    coordinator: &AmbientCoordinator,
    text: &str,
    now_ms: u64,
) {
    if let Some(extracted_request) = evaluate_ambient_segment(coordinator, text, now_ms) {
        let _ = crate::commands::ambient::emit_ambient_addressed(app, &extracted_request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::ambient::{EchoSuppressor, EngagementJudge, RollingTranscript};

    #[test]
    fn ambient_capture_gating_matches_feature_and_dictation_state() {
        assert!(!should_run_ambient_capture(false, false));
        assert!(!should_run_ambient_capture(false, true));
        assert!(!should_run_ambient_capture(true, true));
        assert!(should_run_ambient_capture(true, false));
    }

    fn test_pipeline(segments: &[&str]) -> Vec<String> {
        let coordinator = AmbientCoordinator::new(
            RollingTranscript::new(90_000),
            EngagementJudge::new(vec!["vox".to_string()]),
            EchoSuppressor::new(),
        );
        segments
            .iter()
            .enumerate()
            .filter_map(|(index, text)| evaluate_ambient_segment(&coordinator, text, index as u64))
            .collect()
    }

    #[test]
    fn addressed_pipeline_emits_one_ambient_event_shaped_request() {
        assert_eq!(
            test_pipeline(&["Vox, what time is it"]),
            ["what time is it"]
        );
    }

    #[test]
    fn unrelated_pipeline_emits_no_ambient_detection() {
        assert!(test_pipeline(&["what time is it", "I was telling Vox about it"]).is_empty());
    }
}
