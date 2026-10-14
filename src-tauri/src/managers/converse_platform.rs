//! Real `Platform` for the conversation session: wires the pure session
//! runtime (`converse_runtime`) to the app's recorder, transcriber, speech,
//! paste, dictation command and reply-backend code. Contract: DESIGN.md
//! sections 2, 3.5 and 3.6 (+ AMENDMENTS).
//!
//! Privacy: nothing here logs transcripts, prompts, replies or paste text.
//! Session audio is never saved and never written to history.

use crate::audio_toolkit::{is_microphone_access_denied, is_no_input_device_error, VadPolicy};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::composer_config::{settings_from_app, TtsComposerMode};
use crate::managers::converse::{
    endpoint_config, BoxFuture, Brain, Ears, Hands, ListenOutcome, Mouth, PhraseOutcome,
    QuietOutcome, SayOutcome, SessionConfig, SpeechQueue,
};
use crate::managers::converse_audio::{wait_for_speech_end, SpeechProbe, WaitOutcome};
use crate::managers::converse_capture::{
    CaptureHandle, CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure,
    Transcriber,
};
use crate::managers::converse_overlay::{OverlayDispatcher, ReadyGate};
use crate::managers::converse_plan::{
    build_plan_prompt, parse_plan, PlanError, PlanInput, TurnPlan,
};
use crate::managers::converse_queue::TtsQueueSession;
use crate::managers::converse_runtime::{AdmissionGuard, Platform, SessionParts, ALREADY_RUNNING};
use crate::managers::emotion::Emotion;
use crate::managers::model::ModelManager;
use crate::managers::ownership::{self, Busy, Lease, Owner};
use crate::managers::speech_backends::{complete, complete_streaming, effective_deadline};
use crate::managers::speech_compose::ComposeError;
use crate::managers::transcription::TranscriptionManager;
use crate::managers::tts::{TtsError, TtsManager};
use crate::overlay::{RealOverlayExecutor, RealOverlaySink};
use crate::settings::{get_settings, AppSettings};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Binding id this session records under (the only binding it ever starts).
const CONVERSE_BINDING: &str = "converse";

/// VAD policy for a conversation listen. Always `Offline`: a conversation
/// listen always needs the VAD to drop non-speech frames, because end-of-speech
/// is detected from the VAD-kept frames. The dictation vad setting does not
/// apply here.
fn conversation_vad_policy(_vad_enabled: bool) -> VadPolicy {
    VadPolicy::Offline
}
/// Tauri event carrying `ConverseStateEvent`.
pub const CONVERSE_STATE_EVENT: &str = "converse-state";
/// Refusal when dictation owns the microphone or is still processing.
pub const DICTATION_BUSY: &str = "Finish the current dictation before starting a conversation.";

/// The lease is the admission guard the controller holds for a session.
impl AdmissionGuard for Lease {}

/// Process-wide readiness gate shared by the recorder port (arms a token per
/// listen, invalidates on stop/cancel/start) and the ordered overlay path.
fn ready_gate() -> &'static Arc<ReadyGate> {
    static GATE: OnceLock<Arc<ReadyGate>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(ReadyGate::new()))
}

/// Overlay dispatcher over the real main-thread executor and overlay sink.
fn overlay_dispatcher(app: &AppHandle) -> OverlayDispatcher {
    OverlayDispatcher::new(
        Arc::new(RealOverlayExecutor::new(app.clone())),
        Arc::new(RealOverlaySink::new(app.clone())),
        ready_gate().clone(),
    )
}

/// Payload of the `converse-state` event (`state` and `end` are static labels).
#[derive(Serialize, Deserialize, Clone, Debug, specta::Type)]
pub struct ConverseStateEvent {
    pub state: String,
    pub end: Option<String>,
}

// ---------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------

/// Why a session cannot start, checked in this order. Messages are shown to
/// the user as-is.
pub fn preflight_check(settings: &AppSettings) -> Result<(), String> {
    if !settings.tts_enabled {
        return Err("Turn on spoken replies in the speech settings first.".to_string());
    }
    if settings.exp_rt_enabled {
        crate::managers::realtime_config::preflight_realtime(settings).map_err(str::to_string)?;
    }
    // The direct endpoint is itself the reply backend, so the composer is not required.
    let direct =
        crate::managers::realtime_config::RealtimeConfig::from_settings(settings).direct_endpoint;
    if !direct && settings_from_app(settings).mode == TtsComposerMode::Off {
        return Err("Set up a reply backend in the speech settings first.".to_string());
    }
    if settings.wake_word_enabled || settings.ambient_mode_enabled {
        return Err(
            "Turn off wake word and ambient listening before starting a conversation.".to_string(),
        );
    }
    Ok(())
}

/// Maps the `Err(String)` of `AudioRecordingManager::try_start_recording` to
/// a `StartError`. Real messages (managers/audio.rs):
/// - "Already recording" (state not Idle; the only busy signal, it is an Err)
/// - "Failed to open microphone stream: {e}" / "Failed to start recorder: {e}"
/// - "Recorder not available"
///
/// Permission and missing-device checks reuse the existing toolkit classifiers.
pub fn classify_start_error(message: &str) -> StartError {
    if is_microphone_access_denied(message) {
        StartError::Permission
    } else if is_no_input_device_error(message) {
        StartError::NoMicrophone
    } else if message.contains("Already recording") {
        StartError::Busy
    } else {
        StartError::Other
    }
}

/// Planner errors as the session sees them. Only the three configuration
/// errors are "not configured"; everything else is a backend failure.
pub fn map_compose_error(err: ComposeError) -> PlanError {
    match err {
        ComposeError::NotConfigured
        | ComposeError::NeedsAcknowledgement
        | ComposeError::BadConfig => PlanError::NotConfigured,
        ComposeError::Timeout => PlanError::Timeout,
        ComposeError::Backend
        | ComposeError::OutputTooLarge
        | ComposeError::Empty
        | ComposeError::Unparseable => PlanError::Backend,
    }
}

/// Upper bound on how long one spoken reply may take: max(10 s, 0.8 s per
/// word + 5 s).
pub fn speech_deadline(text: &str) -> Duration {
    let words = text.split_whitespace().count() as u64;
    let estimate_ms = words.saturating_mul(800).saturating_add(5_000);
    Duration::from_millis(estimate_ms.max(10_000))
}

// ---------------------------------------------------------------------
// Hardware / engine ports
// ---------------------------------------------------------------------

/// Recorder port over the shared recorder, always under binding "converse".
struct AudioRecorderPort {
    app: AppHandle,
}

impl RecorderPort for AudioRecorderPort {
    fn start(&self) -> Result<(), StartError> {
        // A new listen supersedes any readiness still armed for the last one.
        ready_gate().invalidate();
        let settings = get_settings(&self.app);
        // Same policy TranscribeAction::start picks for a non-streaming
        // manual recording. Streaming is never used by a session.
        let policy = conversation_vad_policy(settings.vad_enabled);
        // Mirror TranscribeAction::start: kick off the model load and the VAD
        // preload before the microphone opens.
        let tm = self.app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();
        // Don't open the mic if nothing can transcribe the recording.
        if !tm.is_model_loaded() {
            let selected_model = settings.selected_model.clone();
            if self
                .app
                .state::<Arc<ModelManager>>()
                .get_model_path(&selected_model)
                .is_err()
            {
                return Err(StartError::Other);
            }
        }
        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        let preload = Arc::clone(&rm);
        std::thread::spawn(move || {
            if let Err(e) = preload.preload_vad() {
                log::debug!("converse: VAD pre-load failed: {e}");
            }
        });
        match rm.try_start_recording(CONVERSE_BINDING, policy) {
            Ok(readiness) => {
                // Ready fires for THIS listen only, on the first real
                // microphone samples, through the ordered overlay path.
                let token = ready_gate().arm();
                let dispatcher = overlay_dispatcher(&self.app);
                std::thread::spawn(move || {
                    if readiness.wait() {
                        dispatcher.deliver_ready(token);
                    }
                });
                Ok(())
            }
            Err(message) => Err(classify_start_error(&message)),
        }
    }

    fn stop(&self) -> Option<Vec<f32>> {
        ready_gate().invalidate();
        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        rm.stop_recording(CONVERSE_BINDING, rm.cancel_generation())
    }

    fn cancel(&self) {
        ready_gate().invalidate();
        let rm = self.app.state::<Arc<AudioRecordingManager>>();
        let _ = rm.cancel_recording_if_binding(CONVERSE_BINDING);
    }
}

/// Transcriber: the non-streaming path of `TranscribeAction::stop`. Writes no
/// WAV, no history, and logs no text.
struct AppTranscriber {
    app: AppHandle,
}

impl Transcriber for AppTranscriber {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeFailure> {
        let tm = self.app.state::<Arc<TranscriptionManager>>();
        tm.transcribe(samples).map_err(|_| TranscribeFailure)
    }
}

// ---------------------------------------------------------------------
// Session collaborators
// ---------------------------------------------------------------------

struct RealEars {
    app: AppHandle,
    capture: CaptureHandle,
}

impl Ears for RealEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        Box::pin(async move {
            let cfg = endpoint_config(&get_settings(&self.app));
            self.capture.listen(cfg).await
        })
    }
}

/// Adapter so `wait_for_speech_end` can poll the shared TTS manager.
struct TtsProbe<'a>(&'a TtsManager);

impl SpeechProbe for TtsProbe<'_> {
    fn is_current(&self, epoch: u64) -> bool {
        self.0.is_current(epoch)
    }
    fn is_speaking(&self) -> bool {
        self.0.is_speaking()
    }
}

struct RealMouth {
    app: AppHandle,
    tts: Arc<TtsManager>,
}

impl RealMouth {
    /// Opens a queue session with the voice and rate `say_inner` uses; `None` when speech is
    /// off or the session cannot open right now.
    fn open_queue_inner(&self) -> Option<Arc<dyn SpeechQueue>> {
        let settings = get_settings(&self.app);
        if !settings.tts_enabled {
            return None;
        }
        let voice = settings.tts_voice.clone();
        let rate = Some(settings.tts_rate_wpm);
        let session = TtsQueueSession::open(self.tts.clone(), voice, rate)?;
        Some(session)
    }

    async fn say_inner(&self, text: &str, emotion: Emotion) -> SayOutcome {
        let settings = get_settings(&self.app);
        if !settings.tts_enabled {
            return SayOutcome::Failed;
        }
        let epoch = match self.tts.begin_utterance() {
            Ok(epoch) => epoch,
            Err(TtsError::Recording) => return SayOutcome::Interrupted,
            Err(_) => return SayOutcome::Failed,
        };
        // Voice and rate exactly as the spoken-reply path (speech_pipeline.rs).
        let voice = settings.tts_voice.as_deref();
        let rate = Some(settings.tts_rate_wpm);
        match self
            .tts
            .speak_styled_if_current(epoch, text, voice, rate, emotion)
        {
            Ok(()) => {}
            Err(TtsError::Superseded | TtsError::Recording) => return SayOutcome::Interrupted,
            Err(_) => return SayOutcome::Failed,
        }
        let probe = TtsProbe(&self.tts);
        match wait_for_speech_end(
            &probe,
            epoch,
            speech_deadline(text),
            Duration::from_millis(50),
        )
        .await
        {
            WaitOutcome::Done => SayOutcome::Done,
            WaitOutcome::Interrupted => SayOutcome::Interrupted,
            WaitOutcome::TimedOut => {
                self.tts.stop();
                SayOutcome::Failed
            }
        }
    }

    async fn wait_quiet_inner(&self, max: Duration) -> QuietOutcome {
        let started = Instant::now();
        loop {
            if !self.tts.is_speaking() {
                return QuietOutcome::Quiet;
            }
            if started.elapsed() >= max {
                return QuietOutcome::StillSpeaking;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Mouth for RealMouth {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async move { self.say_inner(text, emotion).await })
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        self.open_queue_inner()
    }

    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async move { self.wait_quiet_inner(max).await })
    }

    fn stop(&self) {
        let _ = self.tts.stop();
    }
}

struct RealHands {
    app: AppHandle,
}

impl Hands for RealHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        // Same main-thread hop the dictation path uses for `utils::paste`.
        let (tx, rx) = std::sync::mpsc::channel();
        // Set when the caller gives up waiting, so a closure still queued on
        // the main thread does not paste later.
        let abandoned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let abandoned_on_main = abandoned.clone();
        let app = self.app.clone();
        let app_for_paste = self.app.clone();
        let text = text.to_string();
        app.run_on_main_thread(move || {
            let result = if abandoned_on_main.load(std::sync::atomic::Ordering::SeqCst) {
                Err("paste abandoned".to_string())
            } else {
                crate::clipboard::paste_generated(&text, &app_for_paste)
            };
            let _ = tx.send(result);
        })
        .map_err(|_| "paste failed".to_string())?;
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                abandoned.store(true, std::sync::atomic::Ordering::SeqCst);
                Err("paste timed out".to_string())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("paste failed".to_string()),
        }
    }

    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async move {
            let app = self.app.clone();
            let settings = get_settings(&app);
            let phrase = phrase.to_string();
            let joined = tauri::async_runtime::spawn_blocking(move || {
                crate::actions::run_user_phrase_blocking(&app, &settings, &phrase)
            })
            .await;
            joined.unwrap_or(PhraseOutcome::Failed)
        })
    }

    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async move {
            let app = self.app.clone();
            let settings = get_settings(&app);
            let phrase = phrase.to_string();
            let joined = tauri::async_runtime::spawn_blocking(move || {
                crate::actions::run_model_command_blocking(&app, &settings, &phrase)
            })
            .await;
            joined.unwrap_or(PhraseOutcome::Failed)
        })
    }
}

struct RealBrain {
    app: AppHandle,
}

impl Brain for RealBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async move {
            let settings = get_settings(&self.app);
            let deadline = effective_deadline(&settings);
            let prompt = build_plan_prompt(&input);
            let raw = complete(&settings, &prompt.system, &prompt.combined, deadline)
                .await
                .map_err(map_compose_error)?;
            parse_plan(&raw, &prompt.combined)
        })
    }

    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async move {
            let settings = get_settings(&self.app);
            let deadline = effective_deadline(&settings);
            let prompt = build_plan_prompt(&input);
            let raw = complete_streaming(
                &settings,
                &prompt.system,
                &prompt.combined,
                deadline,
                on_text,
            )
            .await
            .map_err(map_compose_error)?;
            parse_plan(&raw, &prompt.combined)
        })
    }
}

// ---------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------

/// The app-backed `Platform` used by `ConverseController`.
pub struct RealPlatform {
    pub app: AppHandle,
}

impl Platform for RealPlatform {
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        match ownership::global().try_acquire(Owner::Conversation) {
            Ok(lease) => Ok(Box::new(lease)),
            Err(Busy(Owner::Dictation)) => Err(DICTATION_BUSY.to_string()),
            Err(Busy(Owner::Conversation)) => Err(ALREADY_RUNNING.to_string()),
        }
    }

    fn preflight(&self) -> Result<(), String> {
        preflight_check(&get_settings(&self.app))
    }

    fn capture(&self) -> CaptureHandle {
        CaptureWorker::spawn(
            Arc::new(AudioRecorderPort {
                app: self.app.clone(),
            }),
            Arc::new(AppTranscriber {
                app: self.app.clone(),
            }),
            CaptureTimings::default(),
        )
    }

    fn parts(&self, capture: &CaptureHandle) -> SessionParts {
        let settings = get_settings(&self.app);
        crate::managers::realtime_complete::spawn_prewarm(&settings);
        let tts = self.app.state::<Arc<TtsManager>>().inner().clone();
        SessionParts {
            ears: Box::new(RealEars {
                app: self.app.clone(),
                capture: capture.clone(),
            }),
            mouth: Box::new(RealMouth {
                app: self.app.clone(),
                tts,
            }),
            hands: Box::new(RealHands {
                app: self.app.clone(),
            }),
            brain: Box::new(RealBrain {
                app: self.app.clone(),
            }),
            cfg: SessionConfig::from_settings(&settings),
        }
    }

    fn on_state(&self, state: &'static str, end: Option<&'static str>) {
        let payload = ConverseStateEvent {
            state: state.to_string(),
            end: end.map(str::to_string),
        };
        if self.app.emit(CONVERSE_STATE_EVENT, payload).is_err() {
            log::debug!("converse: state event not delivered");
        }
        overlay_dispatcher(&self.app).apply_state(state);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn conversation_always_uses_the_offline_vad_even_if_dictation_vad_is_off() {
        use crate::audio_toolkit::VadPolicy as P;
        assert_eq!(super::conversation_vad_policy(true), P::Offline);
        assert_eq!(super::conversation_vad_policy(false), P::Offline);
    }

    use super::*;
    use crate::managers::composer_config::TtsComposerMode;

    fn base_settings() -> AppSettings {
        let mut s = crate::settings::get_default_settings();
        s.tts_enabled = true;
        s.tts_composer_mode = TtsComposerMode::Model;
        s.wake_word_enabled = false;
        s.ambient_mode_enabled = false;
        s
    }

    #[test]
    fn preflight_requires_spoken_replies() {
        let mut s = base_settings();
        s.tts_enabled = false;
        assert_eq!(
            preflight_check(&s),
            Err("Turn on spoken replies in the speech settings first.".to_string())
        );
    }

    #[test]
    fn preflight_requires_a_reply_backend() {
        let mut s = base_settings();
        s.tts_composer_mode = TtsComposerMode::Off;
        assert_eq!(
            preflight_check(&s),
            Err("Set up a reply backend in the speech settings first.".to_string())
        );
    }

    #[test]
    fn preflight_refuses_wake_word() {
        let mut s = base_settings();
        s.wake_word_enabled = true;
        assert_eq!(
            preflight_check(&s),
            Err(
                "Turn off wake word and ambient listening before starting a conversation."
                    .to_string()
            )
        );
    }

    #[test]
    fn preflight_refuses_ambient() {
        let mut s = base_settings();
        s.ambient_mode_enabled = true;
        assert_eq!(
            preflight_check(&s),
            Err(
                "Turn off wake word and ambient listening before starting a conversation."
                    .to_string()
            )
        );
    }

    #[test]
    fn preflight_reports_the_first_failing_check() {
        let mut s = base_settings();
        s.tts_enabled = false;
        s.tts_composer_mode = TtsComposerMode::Off;
        s.wake_word_enabled = true;
        assert_eq!(
            preflight_check(&s),
            Err("Turn on spoken replies in the speech settings first.".to_string())
        );
    }

    #[test]
    fn preflight_ok_when_everything_is_set() {
        assert_eq!(preflight_check(&base_settings()), Ok(()));
    }

    #[test]
    fn already_recording_is_busy() {
        assert_eq!(classify_start_error("Already recording"), StartError::Busy);
    }

    #[test]
    fn microphone_permission_denied_is_permission() {
        assert_eq!(
            classify_start_error("Failed to start recorder: permission denied"),
            StartError::Permission
        );
        assert_eq!(
            classify_start_error(
                "Failed to open microphone stream: Access is denied. (0x80070005)"
            ),
            StartError::Permission
        );
    }

    #[test]
    fn missing_input_device_is_no_microphone() {
        assert_eq!(
            classify_start_error("Failed to open microphone stream: No input device found"),
            StartError::NoMicrophone
        );
        assert_eq!(
            classify_start_error(
                "Failed to start recorder: Failed to fetch preferred config: CoreAudio device gone"
            ),
            StartError::NoMicrophone
        );
    }

    #[test]
    fn unknown_start_errors_are_other() {
        assert_eq!(
            classify_start_error("Recorder not available"),
            StartError::Other
        );
        assert_eq!(classify_start_error("something else"), StartError::Other);
    }

    #[test]
    fn every_compose_error_maps_to_a_plan_error() {
        let cases = [
            (ComposeError::NotConfigured, PlanError::NotConfigured),
            (ComposeError::NeedsAcknowledgement, PlanError::NotConfigured),
            (ComposeError::BadConfig, PlanError::NotConfigured),
            (ComposeError::Timeout, PlanError::Timeout),
            (ComposeError::Backend, PlanError::Backend),
            (ComposeError::OutputTooLarge, PlanError::Backend),
            (ComposeError::Empty, PlanError::Backend),
            (ComposeError::Unparseable, PlanError::Backend),
        ];
        for (input, expected) in cases {
            assert_eq!(map_compose_error(input), expected, "{input:?}");
        }
    }

    #[test]
    fn speech_deadline_has_a_ten_second_floor() {
        assert_eq!(speech_deadline(""), Duration::from_secs(10));
        assert_eq!(speech_deadline("hi there"), Duration::from_secs(10));
    }

    #[test]
    fn speech_deadline_grows_with_word_count() {
        let twenty = vec!["word"; 20].join(" ");
        assert_eq!(speech_deadline(&twenty), Duration::from_secs(21));
        let hundred = vec!["word"; 100].join(" ");
        assert_eq!(speech_deadline(&hundred), Duration::from_secs(85));
    }

    #[test]
    fn state_event_json_shape() {
        let listening = ConverseStateEvent {
            state: "listening".to_string(),
            end: None,
        };
        assert_eq!(
            serde_json::to_string(&listening).unwrap(),
            r#"{"state":"listening","end":null}"#
        );
        let ended = ConverseStateEvent {
            state: "idle".to_string(),
            end: Some("user_ended".to_string()),
        };
        assert_eq!(
            serde_json::to_string(&ended).unwrap(),
            r#"{"state":"idle","end":"user_ended"}"#
        );
    }
}
