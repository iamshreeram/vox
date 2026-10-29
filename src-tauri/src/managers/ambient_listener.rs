//! Dedicated ambient microphone capture and VAD-silence segmented transcription.
//!
//! Transcript content stays in RAM: it is never logged, serialized, or written
//! to disk. Audio segments are handed directly to the existing transcriber.
//!
//! Gating (Track W): capture runs only while `ambient_effective` holds (Experimental on and
//! the stored flag on) and the wake word is off. Every start, stop and disable bumps an
//! `AmbientGeneration`; a segment or a mic open that began under an older generation is
//! dropped, and a transcription result that comes back under an older generation never
//! reaches the coordinator.

use crate::audio_toolkit::vad::{
    frames_for_duration_ms, SmoothedVad, VAD_ONSET_MS, VAD_PREFILL_MS,
};
use crate::audio_toolkit::{list_input_devices, AudioRecorder, EarshotVad, VadPolicy};
use crate::helpers::clamshell;
use crate::managers::ambient::{AmbientCoordinator, Engagement};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{ambient_effective, get_settings, AppSettings};
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

/// True when ambient capture may run: the effective flag is on and no dictation holds the mic.
pub fn should_run_ambient_capture(ambient_effective: bool, is_recording: bool) -> bool {
    ambient_effective && !is_recording
}

/// Seam over the ambient microphone, so the gating rules run without hardware.
pub trait AmbientMic: Send {
    fn start_session(&mut self) -> Result<(), String>;
    fn stop_session(&mut self) -> Result<Vec<f32>, String>;
    fn close_mic(&mut self) -> Result<(), String>;
}

impl AmbientMic for AudioRecorder {
    fn start_session(&mut self) -> Result<(), String> {
        self.start(VadPolicy::Offline)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn stop_session(&mut self) -> Result<Vec<f32>, String> {
        self.stop().map_err(|error| error.to_string())
    }

    fn close_mic(&mut self) -> Result<(), String> {
        self.close().map_err(|error| error.to_string())
    }
}

/// Seam over the segment transcriber, so result admission runs without a model.
pub trait SegmentTranscriber {
    fn transcribe_segment(&self, audio: Vec<f32>) -> Result<String, String>;
}

impl SegmentTranscriber for TranscriptionManager {
    fn transcribe_segment(&self, audio: Vec<f32>) -> Result<String, String> {
        self.transcribe(audio).map_err(|error| error.to_string())
    }
}

/// Bumped on every start, stop and disable. Work tagged with an older value (a segment being
/// captured, a transcription in flight, a mic open) is stale and is never admitted.
#[derive(Default)]
pub struct AmbientGeneration(AtomicU64);

impl AmbientGeneration {
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// Pure: a result tagged `submitted` is admitted only while that generation is still current.
pub fn result_is_current(submitted: u64, current: u64) -> bool {
    submitted == current
}

/// What happens to a finished segment, or to buffered audio at a capture-off transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentDisposition {
    Submit,
    Discard,
}

/// Pure: a segment is submitted only while the listener is enabled, the feature is effective,
/// and the session began under the current generation. Otherwise its audio is dropped.
pub fn segment_disposition(
    listener_enabled: bool,
    effective: bool,
    session_is_current: bool,
) -> SegmentDisposition {
    if listener_enabled && effective && session_is_current {
        SegmentDisposition::Submit
    } else {
        SegmentDisposition::Discard
    }
}

/// Pure: only the first start claims the monitor thread, so repeated starts add no thread.
pub fn try_claim_monitor(claimed: &AtomicBool) -> bool {
    !claimed.swap(true, Ordering::SeqCst)
}

/// Runs one submitted segment. The text reaches `on_text` only while the listener is still on
/// the generation the segment was submitted under. A stale result is dropped here, before any
/// coordinator processing or addressed-request dispatch.
pub fn run_segment(
    transcriber: &dyn SegmentTranscriber,
    generation: &AmbientGeneration,
    submitted: u64,
    audio: Vec<f32>,
    on_text: &mut dyn FnMut(&str),
) {
    match transcriber.transcribe_segment(audio) {
        Ok(text) if !text.trim().is_empty() => {
            if result_is_current(submitted, generation.current()) {
                on_text(&text);
            } else {
                log::debug!("ambient result dropped: listener changed while transcribing");
            }
        }
        Ok(_) => {}
        Err(error) => warn!("Ambient transcription failed: {error}"),
    }
}

/// Installs a freshly opened microphone only if the listener is still wanted on the generation
/// it was opened for. The check and the install happen under the slot lock, and `stop` takes the
/// same lock before closing, so a late open cannot survive a disable. Returns whether it was
/// installed; a refused microphone is closed.
pub fn open_if_still_wanted(
    slot: &Mutex<Box<dyn AmbientMic>>,
    generation: &AmbientGeneration,
    expected: u64,
    still_wanted: &dyn Fn() -> bool,
    open: impl FnOnce() -> Result<Box<dyn AmbientMic>, String>,
) -> Result<bool, String> {
    let mut mic = open()?;
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if generation.current() == expected && still_wanted() {
        *guard = mic;
        return Ok(true);
    }
    drop(guard);
    if let Err(error) = mic.close_mic() {
        warn!("Failed to close ambient microphone: {error}");
    }
    Ok(false)
}

pub struct AmbientListener {
    app: AppHandle,
    recorder: Arc<Mutex<Box<dyn AmbientMic>>>,
    enabled: Arc<AtomicBool>,
    generation: Arc<AmbientGeneration>,
    last_speech_ms: Arc<AtomicU64>,
    monitor_started: AtomicBool,
}

impl AmbientListener {
    pub fn new(app: AppHandle) -> Self {
        let placeholder: Box<dyn AmbientMic> =
            Box::new(AudioRecorder::new().expect("AudioRecorder::new is infallible"));
        Self {
            app,
            recorder: Arc::new(Mutex::new(placeholder)),
            enabled: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AmbientGeneration::default()),
            last_speech_ms: Arc::new(AtomicU64::new(monotonic_ms())),
            monitor_started: AtomicBool::new(false),
        }
    }

    pub fn start(&self) {
        self.enabled.store(true, Ordering::SeqCst);
        self.generation.bump();
        if !try_claim_monitor(&self.monitor_started) {
            return;
        }
        let app = self.app.clone();
        let recorder = Arc::clone(&self.recorder);
        let enabled = Arc::clone(&self.enabled);
        let generation = Arc::clone(&self.generation);
        let last_speech_ms = Arc::clone(&self.last_speech_ms);
        let callback_last_speech_ms = Arc::clone(&last_speech_ms);
        std::thread::spawn(move || {
            let mut stream_open = false;
            let mut session_active = false;
            let mut session_gen = 0_u64;
            loop {
                std::thread::sleep(STATE_POLL_INTERVAL);
                let settings = get_settings(&app);
                let dictation_recording = app
                    .try_state::<Arc<AudioRecordingManager>>()
                    .is_some_and(|manager| manager.is_recording());
                let effective = ambient_effective(&settings);
                let listener_enabled = enabled.load(Ordering::SeqCst);
                // Settings mutual exclusion is authoritative; this extra check
                // guards the transition before both listener loops observe it.
                let should_capture = listener_enabled
                    && should_run_ambient_capture(effective, dictation_recording)
                    && !settings.wake_word_enabled;
                if !should_capture {
                    if session_active {
                        let audio = stop_session(&recorder);
                        session_active = false;
                        let session_is_current = session_gen == generation.current();
                        if segment_disposition(listener_enabled, effective, session_is_current)
                            == SegmentDisposition::Submit
                        {
                            submit_segment(&app, &generation, session_gen, audio);
                        }
                    }
                    if stream_open {
                        close_recorder(&recorder);
                        stream_open = false;
                    }
                    continue;
                }
                if !stream_open {
                    let expected = generation.current();
                    let still_wanted = || {
                        enabled.load(Ordering::SeqCst) && {
                            let fresh = get_settings(&app);
                            ambient_effective(&fresh) && !fresh.wake_word_enabled
                        }
                    };
                    let opened = open_if_still_wanted(
                        &recorder,
                        &generation,
                        expected,
                        &still_wanted,
                        || build_mic(&settings, Arc::clone(&callback_last_speech_ms)),
                    );
                    match opened {
                        Ok(true) => stream_open = true,
                        Ok(false) => continue,
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
                            session_gen = generation.current();
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
                    let session_is_current = session_gen == generation.current();
                    if segment_disposition(
                        enabled.load(Ordering::SeqCst),
                        effective,
                        session_is_current,
                    ) == SegmentDisposition::Submit
                    {
                        submit_segment(&app, &generation, session_gen, audio);
                    }
                    last_speech_ms.store(monotonic_ms(), Ordering::SeqCst);
                }
            }
        });
    }

    pub fn stop(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.generation.bump();
        let mut recorder = self
            .recorder
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Err(error) = recorder.close_mic() {
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

/// Opens a fresh ambient microphone with its VAD and speech-time callback. The caller decides
/// whether to install it (see `open_if_still_wanted`).
fn build_mic(
    settings: &AppSettings,
    last_speech_ms: Arc<AtomicU64>,
) -> Result<Box<dyn AmbientMic>, String> {
    let device = resolve_device(settings)?;
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
    Ok(Box::new(fresh_recorder))
}

fn start_session(recorder: &Mutex<Box<dyn AmbientMic>>) -> Result<(), String> {
    recorder
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .start_session()
}

fn stop_session(recorder: &Mutex<Box<dyn AmbientMic>>) -> Vec<f32> {
    let result = recorder
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .stop_session();
    match result {
        Ok(audio) => audio,
        Err(error) => {
            warn!("Unable to finish ambient audio segment: {error}");
            Vec::new()
        }
    }
}

fn close_recorder(recorder: &Mutex<Box<dyn AmbientMic>>) {
    let result = recorder
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .close_mic();
    if let Err(error) = result {
        warn!("Failed to close ambient microphone: {error}");
    }
}

fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Hands one finished segment to the transcriber, tagged with the generation it was captured
/// under. A result that returns after a disable or re-enable is dropped in `run_segment`.
fn submit_segment(
    app: &AppHandle,
    generation: &Arc<AmbientGeneration>,
    submitted: u64,
    audio: Vec<f32>,
) {
    if audio.is_empty() {
        return;
    }
    let app = app.clone();
    let generation = Arc::clone(generation);
    std::thread::spawn(move || {
        let Some(transcriber) = app.try_state::<Arc<TranscriptionManager>>() else {
            return;
        };
        let manager: &TranscriptionManager = transcriber.inner();
        run_segment(manager, &generation, submitted, audio, &mut |text| {
            if let Some(coordinator) = app.try_state::<Arc<AmbientCoordinator>>() {
                process_transcribed_segment(&app, &coordinator, text, wall_clock_ms());
            }
        });
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

    /// Microphone double: records whether it was closed.
    struct FakeMic {
        closed: Arc<AtomicBool>,
    }

    impl AmbientMic for FakeMic {
        fn start_session(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn stop_session(&mut self) -> Result<Vec<f32>, String> {
            Ok(Vec::new())
        }
        fn close_mic(&mut self) -> Result<(), String> {
            self.closed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn fake_mic(closed: &Arc<AtomicBool>) -> Box<dyn AmbientMic> {
        Box::new(FakeMic {
            closed: Arc::clone(closed),
        })
    }

    /// Transcriber double: runs `during` while "transcribing" (the test uses it to
    /// disable Experimental after submission, before the result comes back).
    struct FakeTranscriber {
        text: String,
        during: Box<dyn Fn()>,
    }

    impl SegmentTranscriber for FakeTranscriber {
        fn transcribe_segment(&self, _audio: Vec<f32>) -> Result<String, String> {
            (self.during)();
            Ok(self.text.clone())
        }
    }

    #[test]
    fn generation_moves_forward_on_every_bump() {
        let generation = AmbientGeneration::default();
        let first = generation.current();
        let bumped = generation.bump();
        assert_ne!(first, bumped);
        assert_eq!(bumped, generation.current());
        assert!(!result_is_current(first, generation.current()));
        assert!(result_is_current(bumped, generation.current()));
    }

    #[test]
    fn disable_during_transcription_rejects_the_result_before_the_coordinator() {
        // A-W4: submitted, then Experimental disabled, then released -> nothing is admitted.
        let generation = Arc::new(AmbientGeneration::default());
        let submitted = generation.current();
        let during = {
            let generation = Arc::clone(&generation);
            Box::new(move || {
                generation.bump();
            })
        };
        let transcriber = FakeTranscriber {
            text: "Vox, what time is it".to_string(),
            during,
        };
        let mut admitted = Vec::<String>::new();
        run_segment(
            &transcriber,
            &generation,
            submitted,
            vec![0.0; 16],
            &mut |text| admitted.push(text.to_string()),
        );
        assert!(
            admitted.is_empty(),
            "a stale result must never reach the coordinator"
        );
    }

    #[test]
    fn current_generation_result_is_admitted_once() {
        let generation = AmbientGeneration::default();
        let submitted = generation.current();
        let transcriber = FakeTranscriber {
            text: "hello there".to_string(),
            during: Box::new(|| {}),
        };
        let mut admitted = Vec::<String>::new();
        run_segment(
            &transcriber,
            &generation,
            submitted,
            vec![0.0; 16],
            &mut |text| admitted.push(text.to_string()),
        );
        assert_eq!(admitted, ["hello there"]);
    }

    #[test]
    fn blank_transcript_is_never_admitted() {
        let generation = AmbientGeneration::default();
        let transcriber = FakeTranscriber {
            text: "   ".to_string(),
            during: Box::new(|| {}),
        };
        let mut admitted = Vec::<String>::new();
        run_segment(
            &transcriber,
            &generation,
            generation.current(),
            vec![0.0; 16],
            &mut |text| admitted.push(text.to_string()),
        );
        assert!(admitted.is_empty());
    }

    #[test]
    fn buffered_segment_is_discarded_when_the_listener_is_disabled() {
        // Disabling (or a disabled/ineffective setting) never submits buffered audio.
        assert_eq!(
            segment_disposition(true, true, true),
            SegmentDisposition::Submit
        );
        assert_eq!(
            segment_disposition(false, true, true),
            SegmentDisposition::Discard
        );
        assert_eq!(
            segment_disposition(true, false, true),
            SegmentDisposition::Discard
        );
        // A session that began before a stop/start is never submitted under the new generation.
        assert_eq!(
            segment_disposition(true, true, false),
            SegmentDisposition::Discard
        );
    }

    #[test]
    fn open_that_finishes_after_disable_leaves_no_active_capture() {
        // A-W4: pause before open, disable during the open, release.
        let generation = AmbientGeneration::default();
        let expected = generation.current();
        let placeholder_closed = Arc::new(AtomicBool::new(false));
        let slot = Mutex::new(fake_mic(&placeholder_closed));
        let opened_closed = Arc::new(AtomicBool::new(false));
        let installed = open_if_still_wanted(&slot, &generation, expected, &|| true, || {
            generation.bump();
            Ok(fake_mic(&opened_closed))
        });
        assert_eq!(installed, Ok(false));
        assert!(
            opened_closed.load(Ordering::SeqCst),
            "the late mic is closed"
        );
        assert!(!placeholder_closed.load(Ordering::SeqCst));
    }

    #[test]
    fn open_that_is_no_longer_wanted_is_closed_not_installed() {
        let generation = AmbientGeneration::default();
        let placeholder_closed = Arc::new(AtomicBool::new(false));
        let slot = Mutex::new(fake_mic(&placeholder_closed));
        let opened_closed = Arc::new(AtomicBool::new(false));
        let installed =
            open_if_still_wanted(&slot, &generation, generation.current(), &|| false, || {
                Ok(fake_mic(&opened_closed))
            });
        assert_eq!(installed, Ok(false));
        assert!(opened_closed.load(Ordering::SeqCst));
    }

    #[test]
    fn open_that_is_still_wanted_is_installed() {
        let generation = AmbientGeneration::default();
        let placeholder_closed = Arc::new(AtomicBool::new(false));
        let slot = Mutex::new(fake_mic(&placeholder_closed));
        let opened_closed = Arc::new(AtomicBool::new(false));
        let installed =
            open_if_still_wanted(&slot, &generation, generation.current(), &|| true, || {
                Ok(fake_mic(&opened_closed))
            });
        assert_eq!(installed, Ok(true));
        assert!(!opened_closed.load(Ordering::SeqCst));
    }

    #[test]
    fn experimental_on_with_flag_starts_exactly_one_monitor() {
        let claimed = AtomicBool::new(false);
        assert!(try_claim_monitor(&claimed), "first start owns the monitor");
        assert!(
            !try_claim_monitor(&claimed),
            "a second start adds no thread"
        );
        assert!(!try_claim_monitor(&claimed));
    }

    #[test]
    fn capture_gate_uses_the_effective_flag_only() {
        let mut s = crate::settings::get_default_settings();
        s.ambient_mode_enabled = true;
        assert!(!should_run_ambient_capture(
            crate::settings::ambient_effective(&s),
            false
        ));
        s.experimental_enabled = true;
        assert!(should_run_ambient_capture(
            crate::settings::ambient_effective(&s),
            false
        ));
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
