//! Stage B3b (DESIGN Rev 8, 12.2.6, 12.3, 12.4 B3, 12.7.A): tests for the production barge-in
//! adapters and their session wiring. Fakes only: no audio device, no ONNX model, no Tauri app.
//! The AppHandle-bound adapters are covered through their pure helpers and the fake-platform
//! finalizer ordering test below.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};
use crate::managers::composer_config::TtsComposerMode;
use crate::managers::converse::{
    endpoint_config, BoxFuture, Brain, Ears, Hands, ListenFailure, ListenOutcome, Mouth,
    PhraseOutcome, QuietOutcome, SayOutcome, SessionConfig,
};
use crate::managers::converse_barge::BargeIn;
use crate::managers::converse_barge_monitor::{
    BargeConfig, BargeMonitor, ClipGate, ClipTranscriber, MonitorEffects, MonitorInput,
    MonitorStream, SampleCallback, VadFactory,
};
use crate::managers::converse_barge_platform::{
    accepts_verdict, barge_config, barge_enabled, listen_with_barge_recovery,
    start_monitor_fail_soft, BargeRecovery, BargeSlot, SpeakerClipGate, TranscriberClip,
};
use crate::managers::converse_capture::{
    CaptureHandle, CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure,
    Transcriber,
};
use crate::managers::converse_hooks;
use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::converse_platform::{classify_start_error, preflight_check};
use crate::managers::converse_runtime::{
    AdmissionGuard, ConverseController, Platform, SessionParts,
};
use crate::managers::emotion::Emotion;
use crate::managers::speaker_id::{GateResult, SpeakerGate, Verdict};
use crate::settings::{get_default_settings, AppSettings, ConverseBargeIn};

// ----------------------------- settings -----------------------------

fn settings(barge: ConverseBargeIn, spoken: bool, mode: TtsComposerMode) -> AppSettings {
    let mut s = get_default_settings();
    // Legacy fixture: each feature keeps its own provider settings.
    s.brain_source_composer = crate::settings::BrainSource::Own;
    s.brain_source_realtime = crate::settings::BrainSource::Own;
    s.brain_source_postprocess = crate::settings::BrainSource::Own;
    s.tts_composer_model = "legacy-model".into();
    s.converse_barge_in = barge;
    s.tts_enabled = spoken;
    s.tts_composer_mode = mode;
    // Keep ambient listening out of the picture (preflight refuses it while effective).
    s.wake_word_enabled = false;
    s.ambient_mode_enabled = false;
    s
}

#[test]
fn barge_enabled_truth_table() {
    use ConverseBargeIn::{Headphones, Off};
    use TtsComposerMode::{Model, Off as ComposerOff};
    // Off: never, whatever else is set.
    assert!(!barge_enabled(&settings(Off, true, Model)));
    // Headphones with spoken replies and a reply backend: on.
    assert!(barge_enabled(&settings(Headphones, true, Model)));
    // Headphones but spoken replies disabled: off.
    assert!(!barge_enabled(&settings(Headphones, false, Model)));
    // Headphones but no reply backend (composer off, no direct endpoint): off.
    assert!(!barge_enabled(&settings(Headphones, true, ComposerOff)));
}

#[test]
fn a_direct_endpoint_is_a_reply_backend_for_barge_in() {
    let mut s = settings(ConverseBargeIn::Headphones, true, TtsComposerMode::Off);
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    assert!(barge_enabled(&s));
}

#[test]
fn barge_enabled_agrees_with_the_preflight_backend_rule() {
    // For headphones settings, barge-in is enabled exactly when the session preflight passes:
    // the monitor must never run in a configuration the session itself would refuse.
    for spoken in [true, false] {
        for mode in [TtsComposerMode::Off, TtsComposerMode::Model] {
            let s = settings(ConverseBargeIn::Headphones, spoken, mode);
            assert_eq!(
                barge_enabled(&s),
                preflight_check(&s).is_ok(),
                "spoken={spoken} mode={mode:?}"
            );
        }
    }
}

#[test]
fn barge_config_takes_the_endpointing_of_a_normal_turn() {
    let mut s = get_default_settings();
    s.converse_silence_ms = 900;
    let endpoint = endpoint_config(&s);
    let cfg = barge_config(&endpoint);
    assert_eq!(cfg.endpoint_silence_ms, endpoint.silence_ms);
    assert_eq!(cfg.endpoint_silence_ms, 900);
    assert_eq!(cfg.max_utterance_ms, endpoint.max_utterance_ms);
    assert_eq!(cfg.heartbeat_ms, 1000);
    assert_eq!(cfg.channel_frames, 64);
}

#[test]
fn barge_silence_is_clamped_to_one_and_a_half_seconds() {
    let mut s = get_default_settings();
    s.converse_silence_ms = 4000;
    let endpoint = endpoint_config(&s);
    assert!(
        endpoint.silence_ms > 1500,
        "precondition: a long configured pause"
    );
    let cfg = barge_config(&endpoint);
    assert_eq!(cfg.endpoint_silence_ms, 1500);
    assert_eq!(cfg.max_utterance_ms, endpoint.max_utterance_ms);
}

// ----------------------------- clip gate and transcriber -----------------------------

struct FixedGate(Verdict);

impl SpeakerGate for FixedGate {
    fn check(&self, _buffer: &[f32], _cancel: &AtomicBool) -> GateResult {
        GateResult {
            verdict: self.0,
            verify_ms: None,
        }
    }
}

#[test]
fn clip_gate_maps_accept_and_too_short_to_true_and_reject_to_false() {
    let cases = [
        (Verdict::Accept, true, "Accept"),
        (Verdict::SkippedTooShort, true, "SkippedTooShort"),
        (Verdict::Reject, false, "Reject"),
    ];
    for (verdict, expected, label) in cases {
        let gate = SpeakerClipGate::new(Arc::new(FixedGate(verdict)));
        assert_eq!(
            gate.accepts(&[0.0; 16]),
            expected,
            "{label} through the clip gate"
        );
        assert_eq!(accepts_verdict(verdict), expected, "{label} mapping");
    }
}

struct EchoLength;

impl Transcriber for EchoLength {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeFailure> {
        Ok(samples.len().to_string())
    }
}

#[test]
fn transcriber_clip_returns_the_text_of_the_blocking_call() {
    let clip = TranscriberClip::new(Arc::new(EchoLength));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let out = runtime.block_on(clip.transcribe(vec![0.0; 3]));
    assert_eq!(out, Ok("3".to_string()));
}

// ----------------------------- fail-soft construction -----------------------------

struct FailingInput;

impl MonitorInput for FailingInput {
    fn open(&self, _on_samples: SampleCallback) -> Result<Box<dyn MonitorStream>, String> {
        Err("no microphone".to_string())
    }
}

/// Holds the sample callback like a real open stream does: dropping it would disconnect the
/// worker's channel, which the monitor correctly treats as a dead input.
struct QuietStream {
    _keep_alive: SampleCallback,
}

impl MonitorStream for QuietStream {
    fn close(self: Box<Self>) {}
}

struct OpenInput;

impl MonitorInput for OpenInput {
    fn open(&self, on_samples: SampleCallback) -> Result<Box<dyn MonitorStream>, String> {
        Ok(Box::new(QuietStream {
            _keep_alive: on_samples,
        }))
    }
}

struct QuietVad;

impl VoiceActivityDetector for QuietVad {
    fn push_frame<'a>(&'a mut self, _frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
        Ok(VadFrame::Noise)
    }

    fn frame_samples(&self) -> usize {
        160
    }
}

fn ok_vad() -> VadFactory {
    Box::new(|| Ok(Box::new(QuietVad) as Box<dyn VoiceActivityDetector>))
}

fn failing_vad() -> VadFactory {
    Box::new(|| Err("no model".to_string()))
}

struct NoEffects;

impl MonitorEffects for NoEffects {
    fn speaking(&self) -> bool {
        false
    }
    fn duck(&self) {}
    fn restore(&self) {}
    fn stop_speech(&self) {}
}

struct NoClips;

impl ClipTranscriber for NoClips {
    fn transcribe(&self, _samples: Vec<f32>) -> BoxFuture<'static, Result<String, String>> {
        Box::pin(std::future::ready(Ok(String::new())))
    }
}

fn monitor_cfg() -> BargeConfig {
    BargeConfig {
        endpoint_silence_ms: 800,
        max_utterance_ms: 5000,
        heartbeat_ms: 1000,
        channel_frames: 64,
        stall_ms: 1000,
    }
}

fn start_with(input: Arc<dyn MonitorInput>, vad: VadFactory) -> Result<Arc<BargeMonitor>, String> {
    BargeMonitor::start(
        input,
        vad,
        Arc::new(NoEffects),
        Arc::new(NoClips),
        None,
        monitor_cfg(),
    )
}

#[test]
fn disabled_barge_in_never_tries_to_build_a_monitor() {
    let mut built = false;
    let out = start_monitor_fail_soft(false, || {
        built = true;
        start_with(Arc::new(OpenInput), ok_vad())
    });
    assert!(out.is_none());
    assert!(!built, "no stream may be opened while barge-in is off");
}

#[test]
fn a_failed_build_is_fail_soft() {
    let mut calls = 0;
    let out = start_monitor_fail_soft(true, || {
        calls += 1;
        Err("no model".to_string())
    });
    assert!(out.is_none());
    assert_eq!(calls, 1);
}

#[test]
fn a_panicking_build_is_fail_soft() {
    let out = start_monitor_fail_soft(true, || -> Result<Arc<BargeMonitor>, String> {
        panic!("injected build panic")
    });
    assert!(out.is_none());
}

#[test]
fn stream_open_failure_and_vad_failure_are_fail_soft() {
    let no_stream = start_monitor_fail_soft(true, || start_with(Arc::new(FailingInput), ok_vad()));
    assert!(
        no_stream.is_none(),
        "stream open failure must not start a monitor"
    );
    let no_vad = start_monitor_fail_soft(true, || start_with(Arc::new(OpenInput), failing_vad()));
    assert!(
        no_vad.is_none(),
        "VAD init failure must not start a monitor"
    );
}

#[test]
fn an_empty_slot_offers_no_barge_in_and_shutdown_is_a_no_op() {
    let slot = BargeSlot::new(None);
    assert!(slot.view().is_none());
    slot.shutdown();
    slot.shutdown();
    assert!(BargeSlot::default().view().is_none());
}

#[test]
fn a_running_monitor_is_offered_and_shut_down() {
    let monitor = start_with(Arc::new(OpenInput), ok_vad()).expect("monitor starts");
    let slot = BargeSlot::new(Some(monitor.clone()));
    let offered: Option<&dyn BargeIn> = slot.view();
    assert!(offered.is_some());
    assert!(offered.and_then(|barge| barge.arm()).is_some());
    slot.shutdown();
    assert!(!monitor.is_healthy(), "shutdown must stop the monitor");
    assert!(
        slot.view().and_then(|barge| barge.arm()).is_none(),
        "a shut-down monitor arms nothing"
    );
}

// ----------------------------- listen recovery -----------------------------

/// Fake monitor side of the recovery: alive until it is retired.
struct FakeBarge {
    alive: AtomicBool,
    retires: AtomicUsize,
}

impl FakeBarge {
    fn with_monitor(alive: bool) -> Arc<FakeBarge> {
        Arc::new(FakeBarge {
            alive: AtomicBool::new(alive),
            retires: AtomicUsize::new(0),
        })
    }
}

impl BargeRecovery for FakeBarge {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn retire(&self) {
        self.retires.fetch_add(1, Ordering::SeqCst);
        self.alive.store(false, Ordering::SeqCst);
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Runs the recovery over scripted listen outcomes. Returns the outcome and the attempt count.
fn recover<R: BargeRecovery + 'static>(
    barge: Arc<R>,
    script: Vec<ListenOutcome>,
) -> (ListenOutcome, usize) {
    let attempts = AtomicUsize::new(0);
    let script = Mutex::new(VecDeque::from(script));
    let outcome = block_on(listen_with_barge_recovery(barge, || {
        attempts.fetch_add(1, Ordering::SeqCst);
        let next = script
            .lock()
            .unwrap()
            .pop_front()
            .expect("an unscripted listen attempt");
        std::future::ready(next)
    }));
    (outcome, attempts.load(Ordering::SeqCst))
}

#[test]
fn a_refused_listen_with_a_live_monitor_retires_it_and_retries_once() {
    let barge = FakeBarge::with_monitor(true);
    let (outcome, attempts) = recover(
        barge.clone(),
        vec![
            ListenOutcome::Failed(ListenFailure::StreamOpen),
            ListenOutcome::Utterance("hi".into()),
        ],
    );
    assert_eq!(outcome, ListenOutcome::Utterance("hi".into()));
    assert_eq!(attempts, 2);
    assert_eq!(barge.retires.load(Ordering::SeqCst), 1);
    assert!(!barge.is_alive());
}

#[test]
fn a_busy_listen_is_the_apps_own_state_and_never_retires_the_monitor() {
    let monitor = start_with(Arc::new(OpenInput), ok_vad()).expect("monitor starts");
    let slot = Arc::new(BargeSlot::new(Some(monitor.clone())));
    let (outcome, attempts) = recover(
        slot.clone(),
        vec![ListenOutcome::Failed(ListenFailure::Busy)],
    );
    assert_eq!(outcome, ListenOutcome::Failed(ListenFailure::Busy));
    assert_eq!(attempts, 1, "Busy is the app's own state: no retry");
    assert!(slot.view().is_some(), "the monitor is untouched");
    assert!(monitor.is_healthy());
}

#[test]
fn a_monitor_that_already_failed_soft_does_not_use_up_the_retry() {
    let monitor = start_with(Arc::new(OpenInput), ok_vad()).expect("monitor starts");
    monitor.report_input_failure();
    let deadline = Instant::now() + Duration::from_secs(5);
    while monitor.is_healthy() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!monitor.is_healthy(), "the input failure made it fail soft");
    let slot = Arc::new(BargeSlot::new(Some(monitor.clone())));
    let (outcome, attempts) = recover(slot, vec![ListenOutcome::Failed(ListenFailure::StreamOpen)]);
    assert_eq!(outcome, ListenOutcome::Failed(ListenFailure::StreamOpen));
    assert_eq!(attempts, 1, "no live monitor: no retry");
}

#[test]
fn raw_stream_open_failures_classify_as_the_retriable_stream_open() {
    for raw in [
        "Failed to open microphone stream: The requested stream configuration is not supported by the device",
        "Failed to open microphone stream: Failed to open stream: AudioUnit initialize failed (-10851)",
        "Failed to start recorder: failed to play stream: the input device stopped responding",
    ] {
        assert_eq!(classify_start_error(raw), StartError::StreamOpen, "{raw}");
    }
}

#[test]
fn permission_missing_device_busy_and_unknown_keep_their_classes() {
    assert_eq!(
        classify_start_error("Failed to start recorder: permission denied"),
        StartError::Permission
    );
    assert_eq!(
        classify_start_error("Failed to open microphone stream: No input device found"),
        StartError::NoMicrophone
    );
    assert_eq!(classify_start_error("Already recording"), StartError::Busy);
    assert_eq!(
        classify_start_error("Recorder not available"),
        StartError::Other
    );
}

#[test]
fn the_real_slot_offers_no_barge_in_after_a_refused_listen() {
    let monitor = start_with(Arc::new(OpenInput), ok_vad()).expect("monitor starts");
    let slot = Arc::new(BargeSlot::new(Some(monitor.clone())));
    assert!(slot.view().is_some());
    let (outcome, attempts) = recover(
        slot.clone(),
        vec![
            ListenOutcome::Failed(ListenFailure::NoMicrophone),
            ListenOutcome::NoSpeech,
        ],
    );
    assert_eq!(outcome, ListenOutcome::NoSpeech);
    assert_eq!(attempts, 2);
    assert!(
        slot.view().is_none(),
        "barge-in is None for the rest of the session"
    );
    assert!(!monitor.is_healthy(), "the refused monitor is shut down");
}

#[test]
fn a_listen_refused_twice_with_a_live_monitor_fails_after_two_attempts() {
    let barge = FakeBarge::with_monitor(true);
    let (outcome, attempts) = recover(
        barge.clone(),
        vec![
            ListenOutcome::Failed(ListenFailure::StreamOpen),
            ListenOutcome::Failed(ListenFailure::StreamOpen),
        ],
    );
    assert_eq!(outcome, ListenOutcome::Failed(ListenFailure::StreamOpen));
    assert_eq!(attempts, 2);
    assert_eq!(barge.retires.load(Ordering::SeqCst), 1);
}

#[test]
fn a_refused_listen_without_a_monitor_is_not_retried() {
    let barge = FakeBarge::with_monitor(false);
    let (outcome, attempts) = recover(
        barge.clone(),
        vec![ListenOutcome::Failed(ListenFailure::NoMicrophone)],
    );
    assert_eq!(outcome, ListenOutcome::Failed(ListenFailure::NoMicrophone));
    assert_eq!(attempts, 1);
    assert_eq!(barge.retires.load(Ordering::SeqCst), 0);
}

#[test]
fn other_failures_and_results_are_never_retried() {
    let cases = [
        ListenOutcome::Failed(ListenFailure::Permission),
        ListenOutcome::Failed(ListenFailure::Other),
        ListenOutcome::Failed(ListenFailure::ModelUnavailable),
        ListenOutcome::NoSpeech,
        ListenOutcome::TooLong,
        ListenOutcome::Aborted,
        ListenOutcome::Utterance("x".into()),
    ];
    for outcome in cases {
        let barge = FakeBarge::with_monitor(true);
        let (got, attempts) = recover(barge.clone(), vec![outcome.clone()]);
        assert_eq!(got, outcome);
        assert_eq!(attempts, 1, "{outcome:?} must not retry");
        assert_eq!(barge.retires.load(Ordering::SeqCst), 0);
        assert!(barge.is_alive());
    }
}

// ----------------------------- finalizer ordering -----------------------------

/// Records the finalizer steps this test can observe, in order.
type Log = Arc<Mutex<Vec<&'static str>>>;

#[derive(Clone, Copy, PartialEq)]
enum Ending {
    /// Two silent turns: the session ends Idle on its own.
    Natural,
    /// The user stops the session while it listens.
    SessionStop,
    /// The session task panics while listening.
    Panic,
}

struct OrderEars {
    ending: Ending,
    log: Log,
    barge_panics: bool,
}

impl Ears for OrderEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        match self.ending {
            Ending::Natural => Box::pin(async { ListenOutcome::NoSpeech }),
            Ending::SessionStop => Box::pin(std::future::pending()),
            Ending::Panic => Box::pin(async { panic!("ears exploded") }),
        }
    }

    fn barge_shutdown(&self) {
        self.log.lock().unwrap().push("barge_shutdown");
        if self.barge_panics {
            panic!("barge shutdown exploded");
        }
    }
}

struct OrderGuard(Log);

impl AdmissionGuard for OrderGuard {}

impl Drop for OrderGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().push("admission_drop");
    }
}

struct QuietMouth;

impl Mouth for QuietMouth {
    fn say<'a>(&'a self, _text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {}
}

struct NoHands;

impl Hands for NoHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
}

struct NoBrain;

impl Brain for NoBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        Box::pin(async { Err(PlanError::Backend) })
    }
}

struct NullPort;

impl RecorderPort for NullPort {
    fn start(&self) -> Result<(), StartError> {
        Ok(())
    }
    fn stop(&self) -> Option<Vec<f32>> {
        None
    }
    fn cancel(&self) {}
}

struct OrderPlatform {
    ending: Ending,
    log: Log,
    barge_panics: bool,
}

impl Platform for OrderPlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }

    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        Ok(Box::new(OrderGuard(self.log.clone())))
    }

    fn capture(&self) -> CaptureHandle {
        CaptureWorker::spawn(
            Arc::new(NullPort),
            Arc::new(EchoLength),
            CaptureTimings {
                busy_retry: Duration::from_millis(10),
                busy_deadline: Duration::from_millis(100),
                readiness_deadline: Duration::from_secs(30),
                abs_slack: Duration::from_secs(30),
                tick: Duration::from_millis(5),
            },
        )
    }

    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        SessionParts {
            ears: Box::new(OrderEars {
                ending: self.ending,
                log: self.log.clone(),
                barge_panics: self.barge_panics,
            }),
            mouth: Box::new(QuietMouth),
            hands: Box::new(NoHands),
            brain: Box::new(NoBrain),
            cfg: SessionConfig {
                post_speech_guard: Duration::from_millis(0),
                quiet_wait: Duration::from_millis(5),
                ..SessionConfig::default()
            },
            transcript: None,
            prepare: None,
            policy: None,
            announcer: None,
            on_complete: None,
        }
    }

    fn on_state(&self, state: &'static str, _end: Option<&'static str>) {
        if state == "idle" {
            self.log.lock().unwrap().push("idle");
        }
    }
}

fn run_session_to_the_end(ending: Ending) -> Vec<&'static str> {
    run_session_to_the_end_with(ending, false)
}

fn run_session_to_the_end_with(ending: Ending, barge_panics: bool) -> Vec<&'static str> {
    let _lock = converse_hooks::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    converse_hooks::install_capture(None);
    converse_hooks::set_session_active(false);
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let controller = ConverseController::new(Arc::new(OrderPlatform {
        ending,
        log: log.clone(),
        barge_panics,
    }));
    controller.start().expect("session starts");
    if ending == Ending::SessionStop {
        assert!(controller.stop(), "stop must be accepted while running");
    }
    let started = Instant::now();
    loop {
        if log.lock().unwrap().contains(&"admission_drop") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "session never released its admission"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let recorded = log.lock().unwrap().clone();
    recorded
}

#[test]
fn barge_shutdown_runs_before_the_admission_is_released_on_a_natural_end() {
    assert_eq!(
        run_session_to_the_end(Ending::Natural),
        vec!["barge_shutdown", "idle", "admission_drop"]
    );
}

#[test]
fn barge_shutdown_runs_before_the_admission_is_released_on_a_session_stop() {
    assert_eq!(
        run_session_to_the_end(Ending::SessionStop),
        vec!["barge_shutdown", "idle", "admission_drop"]
    );
}

#[test]
fn a_panicking_barge_shutdown_does_not_skip_the_later_finalizer_steps() {
    assert_eq!(
        run_session_to_the_end_with(Ending::Natural, true),
        vec!["barge_shutdown", "idle", "admission_drop"]
    );
    assert_eq!(
        run_session_to_the_end_with(Ending::SessionStop, true),
        vec!["barge_shutdown", "idle", "admission_drop"]
    );
}

#[test]
fn barge_shutdown_runs_on_a_session_panic() {
    assert_eq!(
        run_session_to_the_end(Ending::Panic),
        vec!["barge_shutdown", "idle", "admission_drop"]
    );
}
