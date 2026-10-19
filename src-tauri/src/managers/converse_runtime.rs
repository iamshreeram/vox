//! Session controller: owns the lifecycle of one conversation session at a
//! time. Pure logic behind an injected `Platform` (the real one lives in
//! `converse_platform.rs`), so ordering/cleanup guarantees are unit-tested
//! with fakes. Contract: DESIGN.md 3.3 (registry), 3.6 (ConverseManager) and
//! the AMENDMENTS section (Revision 4).

use crate::managers::converse::{
    run_session, BoxFuture, Brain, CancelToken, Deps, Ears, Hands, Mouth, RegistryState, Reporter,
    SessionConfig, SessionEnd, SessionRegistry, SessionState, TranscriptEvent, TranscriptSink,
};
use crate::managers::converse_capture::CaptureHandle;
use crate::managers::converse_confirm::PolicySource;
use crate::managers::converse_hooks;
use crate::managers::converse_latency::{LatencyEvent, LatencyReport, LatencyStats};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

pub const ALREADY_RUNNING: &str = "A conversation is already running.";
pub const START_FAILED: &str = "Could not start the conversation.";

/// Everything one session needs, built fresh per session.
pub struct SessionParts {
    pub ears: Box<dyn Ears>,
    pub mouth: Box<dyn Mouth>,
    pub hands: Box<dyn Hands>,
    pub brain: Box<dyn Brain>,
    pub cfg: SessionConfig,
    /// Transcript collector for this session only (DESIGN Rev 3, 3.5(1)). Default None.
    pub transcript: Option<Arc<dyn TranscriptSink>>,
    /// Runs in the spawned session task before listening, bounded and cancellable. Default None.
    pub prepare: Option<BoxFuture<'static, ()>>,
    /// Called exactly once with the session end, after the finalizer steps. Default None.
    pub on_complete: Option<Box<dyn FnOnce(SessionEnd) + Send>>,
    /// Policy source for jobs and the confirmation gate (M3). Default None (legacy behavior).
    pub policy: Option<Arc<dyn PolicySource>>,
    /// Announcer for job and gate speech (M3). Default None (legacy behavior).
    pub announcer: Option<Arc<crate::managers::job_announcer::Announcer>>,
}

/// Holds the per-session admission (for example an ownership lease). The
/// controller keeps it alive for the whole session; dropping it releases it.
pub trait AdmissionGuard: Send {}

/// Admission guard that claims nothing.
pub struct NoopGuard;

impl AdmissionGuard for NoopGuard {}

pub trait Platform: Send + Sync + 'static {
    /// Human-readable reason when a session cannot start (TTS disabled, no
    /// reply backend, wake word/ambient enabled, ...). Checked BEFORE the
    /// registry is touched.
    fn preflight(&self) -> Result<(), String>;
    /// Spawn the capture worker for this session.
    fn capture(&self) -> CaptureHandle;
    /// Build the session's collaborators; `capture` is the handle `capture()`
    /// returned (the real Ears wraps it).
    fn parts(&self, capture: &CaptureHandle) -> SessionParts;
    /// Emit one `converse-state` event: `(state_label, end_label)`.
    fn on_state(&self, state: &'static str, end: Option<&'static str>);
    /// Claim whatever exclusive resource a session needs (the real platform
    /// takes the ownership lease). Called first by `start`; the returned guard
    /// is held for the whole session. Default: claims nothing.
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
        Ok(Box::new(NoopGuard))
    }
    /// Receives each completed turn's latency report and stats (T9b). Attached to the session
    /// reporter by `start`. Default: no sink.
    fn latency_sink(&self) -> Option<LatencySink> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConverseStatus {
    Idle,
    Running,
    Stopping,
}

pub fn end_label(end: SessionEnd) -> &'static str {
    match end {
        SessionEnd::UserEnded => "user_ended",
        SessionEnd::Idle => "idle",
        SessionEnd::Cancelled => "cancelled",
        SessionEnd::NotConfigured => "not_configured",
        SessionEnd::AudioUnavailable => "audio_unavailable",
        SessionEnd::SpeechStuck => "speech_stuck",
        SessionEnd::TurnLimit => "turn_limit",
    }
}

pub fn state_label(state: SessionState) -> &'static str {
    match state {
        SessionState::Listening => "listening",
        SessionState::Thinking => "thinking",
        SessionState::Speaking => "speaking",
        SessionState::Acting => "acting",
        SessionState::Idle => "idle",
    }
}

/// Forwards every session state except `Idle` (the final idle is emitted only
/// by the finalizer).
struct ForwardReporter {
    platform: Arc<dyn Platform>,
    transcript: Option<Arc<dyn TranscriptSink>>,
    /// T9b: receives each latency report (numbers only). Default None (no-op).
    latency_sink: Option<LatencySink>,
}

/// T9b: receives one latency report and the session's rolling stats per completed turn.
pub type LatencySink = Arc<dyn Fn(&LatencyReport, &LatencyStats) + Send + Sync>;

/// T9b: builds a sink that emits `converse-latency` (`LatencyEvent`) only while
/// `panel_on()` is true at the moment of the call.
pub fn make_latency_sink(
    emit: Arc<dyn Fn(LatencyEvent) + Send + Sync>,
    panel_on: Arc<dyn Fn() -> bool + Send + Sync>,
) -> LatencySink {
    Arc::new(move |report, stats| {
        if panel_on() {
            emit(LatencyEvent {
                report: report.clone(),
                stats: stats.clone(),
            });
        }
    })
}

impl Reporter for ForwardReporter {
    fn state(&self, state: SessionState) {
        if state != SessionState::Idle {
            self.platform.on_state(state_label(state), None);
        }
    }

    fn transcript(&self, ev: TranscriptEvent) {
        if let Some(sink) = self.transcript.as_ref() {
            sink.record(ev);
        }
    }

    fn latency(&self, report: &LatencyReport, stats: &LatencyStats) {
        if let Some(sink) = self.latency_sink.as_ref() {
            sink(report, stats);
        }
    }
}

/// Upper bound for the session's `prepare` step before listening starts (DESIGN 8.3 / 3.5(2)).
const PREPARE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs the session's `prepare` step: bounded by `PREPARE_TIMEOUT` and abortable by cancel.
/// Returns `false` when cancel won (the session must end Cancelled without listening).
async fn run_prepare(prepare: Option<BoxFuture<'static, ()>>, cancel: &mut CancelToken) -> bool {
    let Some(prepare) = prepare else {
        return true;
    };
    if *cancel.borrow() {
        return false;
    }
    let bounded = tokio::time::timeout(PREPARE_TIMEOUT, prepare);
    tokio::select! {
        _ = bounded => true,
        _ = cancel.wait_for(|cancelled| *cancelled) => false,
    }
}

pub struct ConverseController {
    platform: Arc<dyn Platform>,
    registry: Arc<SessionRegistry>,
}

impl ConverseController {
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        ConverseController {
            platform,
            registry: Arc::new(SessionRegistry::new()),
        }
    }

    /// Start a session. Order: `platform.admit()` (Err => return it, nothing
    /// else runs) -> `platform.preflight()` (Err => return it) ->
    /// `registry.try_begin()` (None => `Err(ALREADY_RUNNING)`) ->
    /// `converse_hooks::set_session_active(true)` -> `platform.capture()` ->
    /// `converse_hooks::install_capture(Some(..))` -> `platform.parts()` ->
    /// spawn the session task on the tauri async runtime. Returns the session id.
    ///
    /// The admission guard stays local until the spawn: every early return and
    /// caught construction panic drops it. On success it moves into the session
    /// task and is dropped as that task's last statement.
    ///
    /// The session task runs `run_session` under `catch_unwind` (a panic counts
    /// as `Cancelled`), then the FINALIZER runs, always, in this exact order:
    /// (1) `capture.shutdown()` on a blocking thread and awaited,
    /// (2) `install_capture(None)`, (3) `set_session_active(false)`,
    /// (4) `platform.on_state("idle", Some(end_label(end)))`,
    /// (5) `registry.finish(id)`, (6) drop the admission guard.
    /// While running, the session's `Reporter` forwards every state except
    /// `Idle` (the final idle is emitted only by the finalizer, exactly once).
    pub fn start(&self) -> Result<u64, String> {
        let admission = self.platform.admit()?;
        self.platform.preflight()?;
        let (id, cancel) = self
            .registry
            .try_begin()
            .ok_or_else(|| ALREADY_RUNNING.to_string())?;
        converse_hooks::set_session_active(true);
        // Build the capture worker and collaborators. A panic here must undo
        // the whole start, so nothing is left claimed.
        let mut created: Option<CaptureHandle> = None;
        let built = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let capture = self.platform.capture();
            created = Some(capture.clone());
            converse_hooks::install_capture(Some(capture.clone()));
            (capture.clone(), self.platform.parts(&capture))
        }));
        let (capture, parts) = match built {
            Ok(pair) => pair,
            Err(_) => {
                if let Some(capture) = created {
                    capture.shutdown();
                }
                converse_hooks::install_capture(None);
                converse_hooks::set_session_active(false);
                self.registry.finish(id);
                return Err(START_FAILED.to_string());
            }
        };

        let platform = self.platform.clone();
        let registry = self.registry.clone();
        let latency_sink = self.platform.latency_sink();
        tauri::async_runtime::spawn(async move {
            let SessionParts {
                ears,
                mouth,
                hands,
                brain,
                cfg,
                transcript,
                prepare,
                on_complete,
                policy,
                announcer,
            } = parts;
            let mut cancel = cancel;
            // prepare + run_session share ONE caught boundary; any panic is Cancelled.
            let end = {
                let reporter = ForwardReporter {
                    platform: platform.clone(),
                    transcript,
                    latency_sink,
                };
                let deps = Deps {
                    policy: policy.as_deref(),
                    announcer: announcer.as_deref(),
                    ears: ears.as_ref(),
                    mouth: mouth.as_ref(),
                    hands: hands.as_ref(),
                    brain: brain.as_ref(),
                    reporter: &reporter,
                };
                let body = async {
                    if run_prepare(prepare, &mut cancel).await {
                        run_session(&deps, &cfg, cancel).await
                    } else {
                        SessionEnd::Cancelled
                    }
                };
                FutureExt::catch_unwind(AssertUnwindSafe(body))
                    .await
                    .unwrap_or(SessionEnd::Cancelled)
            };
            // Finalizer: exact order, always runs.
            let _ = tauri::async_runtime::spawn_blocking(move || capture.shutdown()).await;
            converse_hooks::install_capture(None);
            converse_hooks::set_session_active(false);
            platform.on_state("idle", Some(end_label(end)));
            registry.finish(id);
            // on_complete runs after the final idle and registry finish, under its own
            // boundary: a panic is logged with a fixed kind and never skips the steps below.
            if let Some(on_complete) = on_complete {
                if std::panic::catch_unwind(AssertUnwindSafe(move || on_complete(end))).is_err() {
                    log::error!("converse on_complete panicked");
                }
            }
            // Last: release admission only after the final idle (and so the
            // final overlay hide) was emitted and the registry entry finished.
            drop(admission);
        });
        Ok(id)
    }

    /// Request the session to end. True if a Running session was asked to
    /// stop; false when idle or already stopping. Idempotent.
    pub fn stop(&self) -> bool {
        self.registry.request_stop()
    }

    /// True for Running and Stopping (draining).
    pub fn is_active(&self) -> bool {
        self.registry.is_active()
    }

    pub fn status(&self) -> ConverseStatus {
        match self.registry.state() {
            RegistryState::Idle => ConverseStatus::Idle,
            RegistryState::Running(_) => ConverseStatus::Running,
            RegistryState::Draining(_) => ConverseStatus::Stopping,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse::{
        BoxFuture, ListenOutcome, PhraseOutcome, QuietOutcome, SayOutcome,
    };
    use crate::managers::converse_capture::{
        CaptureTimings, CaptureWorker, RecorderPort, StartError, TranscribeFailure, Transcriber,
    };
    use crate::managers::converse_hooks;
    use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
    use crate::managers::emotion::Emotion;
    use crate::managers::ownership::{Busy, Owner, OwnershipGate};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    // ----------------------------- fakes -----------------------------

    struct IdlePort {
        active: AtomicBool,
        cancels: AtomicUsize,
    }
    impl RecorderPort for IdlePort {
        fn start(&self) -> Result<(), StartError> {
            self.active.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&self) -> Option<Vec<f32>> {
            self.active
                .swap(false, Ordering::SeqCst)
                .then(|| vec![0.0; 16])
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.active.store(false, Ordering::SeqCst);
        }
    }
    struct NoTranscribe;
    impl Transcriber for NoTranscribe {
        fn transcribe(&self, _s: Vec<f32>) -> Result<String, TranscribeFailure> {
            Ok(String::new())
        }
    }

    /// Ears that really use the capture worker and wait for cancel.
    struct CaptureEars(CaptureHandle);
    impl Ears for CaptureEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            let h = self.0.clone();
            Box::pin(async move {
                h.listen(crate::managers::converse_audio::EndpointConfig::with_silence(1200))
                    .await
            })
        }
    }
    struct PanicEars;
    impl Ears for PanicEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            Box::pin(async move { panic!("ears exploded") })
        }
    }
    struct QuietMouth(Arc<Mutex<Vec<String>>>);
    impl Mouth for QuietMouth {
        fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
            self.0.lock().unwrap().push(format!("say:{text}"));
            Box::pin(async { SayOutcome::Done })
        }
        fn wait_quiet(&self, _m: Duration) -> BoxFuture<'_, QuietOutcome> {
            Box::pin(async { QuietOutcome::Quiet })
        }
        fn stop(&self) {
            self.0.lock().unwrap().push("mouth.stop".into());
        }
    }
    struct NoHands;
    impl Hands for NoHands {
        fn paste_generated(&self, _t: &str) -> Result<(), String> {
            Ok(())
        }
        fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            Box::pin(async { PhraseOutcome::NoMatch })
        }
        fn run_model_command<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            Box::pin(async { PhraseOutcome::NoMatch })
        }
    }
    struct NoBrain;
    impl Brain for NoBrain {
        fn plan<'a>(&'a self, _i: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
            Box::pin(async { Err(PlanError::Backend) })
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// Real capture-worker Ears: the session blocks until stopped.
        Listening,
        Panicking,
    }

    struct FakePlatform {
        mode: Mode,
        preflight: Mutex<Result<(), String>>,
        events: Mutex<Vec<(&'static str, Option<&'static str>)>>,
        /// Order of platform-visible cleanup steps.
        trace: Mutex<Vec<String>>,
        port: Arc<IdlePort>,
        mouth_log: Arc<Mutex<Vec<String>>>,
        captures_made: AtomicUsize,
        panic_in_parts: AtomicBool,
    }
    impl FakePlatform {
        fn new(mode: Mode) -> Arc<Self> {
            Arc::new(FakePlatform {
                mode,
                preflight: Mutex::new(Ok(())),
                events: Mutex::new(vec![]),
                trace: Mutex::new(vec![]),
                port: Arc::new(IdlePort {
                    active: AtomicBool::new(false),
                    cancels: AtomicUsize::new(0),
                }),
                mouth_log: Arc::new(Mutex::new(vec![])),
                captures_made: AtomicUsize::new(0),
                panic_in_parts: AtomicBool::new(false),
            })
        }
    }
    impl Platform for FakePlatform {
        fn preflight(&self) -> Result<(), String> {
            self.preflight.lock().unwrap().clone()
        }
        fn capture(&self) -> CaptureHandle {
            self.captures_made.fetch_add(1, Ordering::SeqCst);
            CaptureWorker::spawn(
                self.port.clone(),
                Arc::new(NoTranscribe),
                CaptureTimings {
                    busy_retry: Duration::from_millis(10),
                    busy_deadline: Duration::from_millis(100),
                    readiness_deadline: Duration::from_secs(30),
                    abs_slack: Duration::from_secs(30),
                    tick: Duration::from_millis(5),
                },
            )
        }
        fn parts(&self, capture: &CaptureHandle) -> SessionParts {
            if self.panic_in_parts.load(Ordering::SeqCst) {
                panic!("parts exploded");
            }
            let ears: Box<dyn Ears> = match self.mode {
                Mode::Listening => Box::new(CaptureEars(capture.clone())),
                Mode::Panicking => Box::new(PanicEars),
            };
            SessionParts {
                ears,
                mouth: Box::new(QuietMouth(self.mouth_log.clone())),
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
        fn on_state(&self, state: &'static str, end: Option<&'static str>) {
            // Hooks must already be reset when the final event is emitted.
            if state == "idle" {
                self.trace.lock().unwrap().push(format!(
                    "final-emit session_active={} capture_active={} port_active={}",
                    converse_hooks::session_active(),
                    converse_hooks::capture_active(),
                    self.port.active.load(Ordering::SeqCst)
                ));
            }
            self.events.lock().unwrap().push((state, end));
        }
    }

    fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(
                t.elapsed() < Duration::from_secs(5),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        converse_hooks::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn reset_hooks() {
        converse_hooks::install_capture(None);
        converse_hooks::set_session_active(false);
    }

    // ----------------------------- labels -----------------------------

    #[test]
    fn end_and_state_labels_are_the_documented_wire_strings() {
        assert_eq!(end_label(SessionEnd::UserEnded), "user_ended");
        assert_eq!(end_label(SessionEnd::Idle), "idle");
        assert_eq!(end_label(SessionEnd::Cancelled), "cancelled");
        assert_eq!(end_label(SessionEnd::NotConfigured), "not_configured");
        assert_eq!(end_label(SessionEnd::AudioUnavailable), "audio_unavailable");
        assert_eq!(end_label(SessionEnd::SpeechStuck), "speech_stuck");
        assert_eq!(end_label(SessionEnd::TurnLimit), "turn_limit");
        assert_eq!(state_label(SessionState::Listening), "listening");
        assert_eq!(state_label(SessionState::Thinking), "thinking");
        assert_eq!(state_label(SessionState::Speaking), "speaking");
        assert_eq!(state_label(SessionState::Acting), "acting");
        assert_eq!(state_label(SessionState::Idle), "idle");
    }

    // ----------------------------- lifecycle -----------------------------

    #[test]
    fn preflight_failure_is_returned_and_leaves_everything_untouched() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        *p.preflight.lock().unwrap() = Err("Turn on spoken replies first.".into());
        let c = ConverseController::new(p.clone());
        assert_eq!(c.start(), Err("Turn on spoken replies first.".to_string()));
        assert_eq!(c.status(), ConverseStatus::Idle);
        assert!(!c.is_active());
        assert!(!converse_hooks::session_active());
        assert_eq!(
            p.captures_made.load(Ordering::SeqCst),
            0,
            "no worker may be spawned"
        );
        assert!(p.events.lock().unwrap().is_empty());
        // and a later start still works once preflight passes
        *p.preflight.lock().unwrap() = Ok(());
        let id = c.start().expect("start after fixing preflight");
        assert!(id >= 1);
        c.stop();
        wait_until("idle", || !c.is_active());
        reset_hooks();
    }

    #[test]
    fn start_runs_stop_cleans_up_in_order_and_allows_a_restart() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        let c = ConverseController::new(p.clone());
        let id = c.start().unwrap();
        assert_eq!(c.status(), ConverseStatus::Running);
        assert!(c.is_active());
        assert!(converse_hooks::session_active());
        wait_until("capture to own the recorder", || {
            converse_hooks::capture_active()
        });
        assert_eq!(
            c.start(),
            Err(ALREADY_RUNNING.to_string()),
            "double start refused"
        );
        assert!(c.stop());
        assert!(!c.stop(), "second stop is a no-op");
        wait_until("session to drain", || !c.is_active());
        // finalizer order: hooks reset and recorder released BEFORE the final emit
        let trace = p.trace.lock().unwrap().clone();
        assert_eq!(
            trace,
            vec!["final-emit session_active=false capture_active=false port_active=false"]
        );
        assert!(!converse_hooks::session_active());
        assert!(!converse_hooks::capture_active());
        assert!(!p.port.active.load(Ordering::SeqCst));
        assert_eq!(c.status(), ConverseStatus::Idle);
        // exactly one final idle event, carrying the end reason, and it is last
        let ev = p.events.lock().unwrap().clone();
        assert_eq!(ev.iter().filter(|(s, _)| *s == "idle").count(), 1);
        assert_eq!(*ev.last().unwrap(), ("idle", Some("cancelled")));
        assert!(ev.contains(&("listening", None)));
        // a new session can start and gets a fresh id
        let id2 = c.start().unwrap();
        assert!(id2 > id);
        c.stop();
        wait_until("second drain", || !c.is_active());
        assert_eq!(p.captures_made.load(Ordering::SeqCst), 2);
        reset_hooks();
    }

    #[test]
    fn stop_on_an_idle_controller_is_a_harmless_no_op() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        let c = ConverseController::new(p);
        assert!(!c.stop());
        assert_eq!(c.status(), ConverseStatus::Idle);
    }

    #[test]
    fn draining_blocks_a_new_start_until_the_finalizer_is_done() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        let c = ConverseController::new(p.clone());
        c.start().unwrap();
        wait_until("capture active", || converse_hooks::capture_active());
        c.stop();
        // Between stop and the end of the finalizer a start must never succeed
        // while still active; once inactive it must succeed.
        let t = Instant::now();
        loop {
            match c.start() {
                Ok(_) => {
                    assert!(
                        !converse_hooks::capture_active()
                            || p.captures_made.load(Ordering::SeqCst) == 2
                    );
                    break;
                }
                Err(e) => {
                    assert_eq!(e, ALREADY_RUNNING);
                    assert!(t.elapsed() < Duration::from_secs(5), "never freed");
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        c.stop();
        wait_until("drain", || !c.is_active());
        reset_hooks();
    }

    #[test]
    fn a_panicking_session_still_runs_the_finalizer_and_frees_the_controller() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Panicking);
        let c = ConverseController::new(p.clone());
        c.start().unwrap();
        wait_until("finalizer after panic", || !c.is_active());
        assert!(!converse_hooks::session_active());
        assert!(!converse_hooks::capture_active());
        let ev = p.events.lock().unwrap().clone();
        assert_eq!(*ev.last().unwrap(), ("idle", Some("cancelled")));
        assert_eq!(ev.iter().filter(|(s, _)| *s == "idle").count(), 1);
        // controller is reusable after a panic
        *p.preflight.lock().unwrap() = Ok(());
        assert!(c.start().is_ok());
        wait_until("second finalizer", || !c.is_active());
        reset_hooks();
    }

    #[test]
    fn concurrent_starts_yield_exactly_one_session() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        let c = Arc::new(ConverseController::new(p.clone()));
        let wins = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = c.clone();
                let wins = wins.clone();
                std::thread::spawn(move || {
                    if c.start().is_ok() {
                        wins.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wins.load(Ordering::SeqCst), 1);
        assert_eq!(
            p.captures_made.load(Ordering::SeqCst),
            1,
            "losers must not spawn workers"
        );
        c.stop();
        wait_until("drain", || !c.is_active());
        reset_hooks();
    }

    #[test]
    fn a_panic_while_building_the_session_undoes_the_start_completely() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        p.panic_in_parts.store(true, Ordering::SeqCst);
        let c = ConverseController::new(p.clone());
        assert_eq!(c.start(), Err(START_FAILED.to_string()));
        assert!(!c.is_active(), "registry must be released");
        assert_eq!(c.status(), ConverseStatus::Idle);
        assert!(
            !converse_hooks::session_active(),
            "session flag must be undone"
        );
        assert!(!converse_hooks::capture_active());
        assert_eq!(
            p.captures_made.load(Ordering::SeqCst),
            1,
            "the worker was created ..."
        );
        assert!(!p.port.active.load(Ordering::SeqCst));
        assert!(p.events.lock().unwrap().is_empty(), "no session ever ran");
        // ... and the controller is fully reusable afterwards
        p.panic_in_parts.store(false, Ordering::SeqCst);
        assert!(c.start().is_ok());
        c.stop();
        wait_until("drain", || !c.is_active());
        reset_hooks();
    }

    // ------------------- admission guard lifecycle (O7'') -------------------

    /// Ears that replay scripted outcomes, then wait until cancelled.
    struct ScriptedEars(Mutex<VecDeque<ListenOutcome>>);
    impl Ears for ScriptedEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            let next = self.0.lock().unwrap().pop_front();
            let fut: BoxFuture<'_, ListenOutcome> = match next {
                Some(outcome) => Box::pin(async move { outcome }),
                None => Box::pin(std::future::pending()),
            };
            fut
        }
    }

    /// Brain that always answers with one spoken sentence.
    struct ScriptedBrain;
    impl Brain for ScriptedBrain {
        fn plan<'a>(&'a self, _i: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
            Box::pin(async {
                Ok(TurnPlan {
                    job: None,
                    kind: PlanKind::Answer,
                    speech: "Here it is.".to_string(),
                    emotion: Emotion::Neutral,
                    paste_text: None,
                    command: None,
                })
            })
        }
    }

    /// Wraps `FakePlatform`. `admit` claims the injected gate for the
    /// conversation. Every state event is logged as `show:<state>` (`hide` for
    /// the final idle) and probes the gate the way dictation would.
    struct GatedPlatform {
        gate: Arc<OwnershipGate>,
        inner: Arc<FakePlatform>,
        /// true: scripted turns (acting, thinking, speaking, listening). false:
        /// the inner fake's blocking capture-backed ears.
        scripted: bool,
        ops: Mutex<Vec<String>>,
        probes: Mutex<Vec<(&'static str, String)>>,
    }
    impl GatedPlatform {
        fn new(gate: Arc<OwnershipGate>, scripted: bool) -> Arc<Self> {
            Arc::new(GatedPlatform {
                gate,
                inner: FakePlatform::new(Mode::Listening),
                scripted,
                ops: Mutex::new(vec![]),
                probes: Mutex::new(vec![]),
            })
        }
        fn op_log(&self) -> Vec<String> {
            self.ops.lock().unwrap().clone()
        }
        fn probe_log(&self) -> Vec<(&'static str, String)> {
            self.probes.lock().unwrap().clone()
        }
        /// What a dictation start would see right now: `free`, or `Busy(owner)`.
        fn dictation_probe(&self) -> String {
            match self.gate.try_acquire(Owner::Dictation) {
                Ok(lease) => {
                    drop(lease);
                    "free".to_string()
                }
                Err(busy) => format!("{busy:?}"),
            }
        }
    }
    impl Platform for GatedPlatform {
        fn preflight(&self) -> Result<(), String> {
            self.inner.preflight()
        }
        fn admit(&self) -> Result<Box<dyn AdmissionGuard>, String> {
            self.ops.lock().unwrap().push("admit".to_string());
            match self.gate.try_acquire(Owner::Conversation) {
                Ok(lease) => Ok(Box::new(lease)),
                Err(busy) => Err(format!("busy:{busy:?}")),
            }
        }
        fn capture(&self) -> CaptureHandle {
            self.inner.capture()
        }
        fn parts(&self, capture: &CaptureHandle) -> SessionParts {
            if !self.scripted {
                return self.inner.parts(capture);
            }
            let turns = VecDeque::from(vec![ListenOutcome::Utterance(
                "open the notes app".to_string(),
            )]);
            SessionParts {
                ears: Box::new(ScriptedEars(Mutex::new(turns))),
                mouth: Box::new(QuietMouth(self.inner.mouth_log.clone())),
                hands: Box::new(NoHands),
                brain: Box::new(ScriptedBrain),
                cfg: SessionConfig {
                    post_speech_guard: Duration::from_millis(0),
                    quiet_wait: Duration::from_millis(5),
                    ..SessionConfig::default()
                },
                transcript: None,
                prepare: None,
                on_complete: None,
                policy: None,
                announcer: None,
            }
        }
        fn on_state(&self, state: &'static str, end: Option<&'static str>) {
            if state == "idle" {
                self.ops.lock().unwrap().push("hide".to_string());
            } else {
                self.ops.lock().unwrap().push(format!("show:{state}"));
            }
            let probe = self.dictation_probe();
            self.probes.lock().unwrap().push((state, probe));
            self.inner.on_state(state, end);
        }
    }

    /// Stops and waits for a controller's session when the test ends, even on
    /// panic, so a failing test cannot leak a running session into the next.
    struct DrainOnDrop(Arc<ConverseController>);
    impl Drop for DrainOnDrop {
        fn drop(&mut self) {
            self.0.stop();
            let t = Instant::now();
            while self.0.is_active() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// (a) The registry refuses the start after admission succeeded: the
    /// admission must be undone, so dictation can take the gate again.
    #[test]
    fn o7_admission_is_released_when_the_registry_rejects_the_start() {
        let _g = lock();
        reset_hooks();
        let gate = OwnershipGate::new();
        let p = GatedPlatform::new(gate.clone(), false);
        let c = ConverseController::new(p.clone());
        // Make the single-session guard refuse the start.
        let (held_id, _cancel) = c.registry.try_begin().expect("occupy the registry");
        let r = c.start();
        c.registry.finish(held_id);
        assert_eq!(r, Err(ALREADY_RUNNING.to_string()));
        assert_eq!(
            p.op_log(),
            vec!["admit".to_string()],
            "admission must have happened before the registry refused"
        );
        assert_eq!(gate.current(), None, "rejected start must release the gate");
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
        reset_hooks();
    }

    /// (b) A panic while building the capture/parts after admission: the start
    /// fails, the gate is free and no overlay op was recorded.
    /// NOTE: the controller already catches panics there (`catch_unwind`), so
    /// the panic path is tested as is, not as a plain Err.
    #[test]
    fn o7_admission_is_released_when_collaborator_construction_panics() {
        let _g = lock();
        reset_hooks();
        let gate = OwnershipGate::new();
        let p = GatedPlatform::new(gate.clone(), false);
        p.inner.panic_in_parts.store(true, Ordering::SeqCst);
        let c = ConverseController::new(p.clone());
        assert_eq!(c.start(), Err(START_FAILED.to_string()));
        assert_eq!(
            p.op_log(),
            vec!["admit".to_string()],
            "no show or hide may be recorded for a failed start"
        );
        assert!(
            p.probe_log().is_empty(),
            "no session state was ever emitted"
        );
        assert_eq!(gate.current(), None, "panic must release the gate");
        assert!(gate.try_acquire(Owner::Dictation).is_ok());
        reset_hooks();
    }

    /// (c) A full scripted session (listening, acting, thinking, speaking, then
    /// listening until stopped). Dictation must be refused in every observed
    /// state, and the gate may be taken only after the final hide is logged
    /// AND the registry entry is finished.
    #[test]
    fn o7_conversation_lease_blocks_dictation_in_every_observed_state_until_final_hide() {
        let _g = lock();
        reset_hooks();
        let gate = OwnershipGate::new();
        let p = GatedPlatform::new(gate.clone(), true);
        let c = Arc::new(ConverseController::new(p.clone()));
        let _drain = DrainOnDrop(c.clone());
        c.start().expect("conversation start");
        // The second listen blocks until stop, so this is a settled point.
        wait_until("the session to listen again", || {
            p.op_log().iter().filter(|o| *o == "show:listening").count() >= 2
        });
        let seen: Vec<&str> = p.probe_log().iter().map(|(s, _)| *s).collect();
        for state in ["listening", "acting", "thinking", "speaking"] {
            assert!(
                seen.contains(&state),
                "state {state} not observed: {seen:?}"
            );
        }
        assert_eq!(
            *p.inner.mouth_log.lock().unwrap(),
            vec!["say:Here it is.".to_string()],
            "the speaking turn must have run"
        );
        let before_stop = p.probe_log();
        assert!(
            before_stop.iter().all(|(_, r)| r == "Busy(Conversation)"),
            "dictation was admitted during the conversation: {before_stop:?}"
        );
        assert!(c.stop(), "running session must accept stop");
        // Bounded poll: each try must be refused until the final hide is logged
        // and the registry entry is gone. The snapshot is taken BEFORE the try.
        let t = Instant::now();
        loop {
            let hidden = p.op_log().iter().any(|o| o == "hide");
            let drained = !c.is_active();
            match gate.try_acquire(Owner::Dictation) {
                Ok(lease) => {
                    drop(lease);
                    assert!(
                        hidden && drained,
                        "gate freed before final hide/registry cleanup (hidden={hidden}, drained={drained}): {:?}",
                        p.op_log()
                    );
                    break;
                }
                Err(busy) => assert_eq!(busy, Busy(Owner::Conversation), "wrong holder"),
            }
            assert!(
                t.elapsed() < Duration::from_secs(2),
                "gate never freed after the final hide: {:?}",
                p.op_log()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let all = p.probe_log();
        assert!(all.iter().any(|(s, _)| *s == "idle"), "final idle observed");
        assert!(
            all.iter().all(|(_, r)| r == "Busy(Conversation)"),
            "dictation was admitted at some state: {all:?}"
        );
        assert_eq!(c.status(), ConverseStatus::Idle);
        reset_hooks();
    }

    /// (d) Dictation already holds the gate: the start is refused before any
    /// collaborator is built, no overlay op is recorded, and the controller's
    /// session guard is untouched, so a later start works.
    #[test]
    fn o7_refused_admission_builds_nothing_and_leaves_the_session_guard_alone() {
        let _g = lock();
        reset_hooks();
        let gate = OwnershipGate::new();
        let dictation = gate
            .try_acquire(Owner::Dictation)
            .expect("dictation holds the gate");
        let p = GatedPlatform::new(gate.clone(), false);
        let c = Arc::new(ConverseController::new(p.clone()));
        let _drain = DrainOnDrop(c.clone());
        let r = c.start();
        assert!(
            r.is_err(),
            "conversation must be refused while dictation holds the gate, got {r:?}"
        );
        assert_eq!(
            p.inner.captures_made.load(Ordering::SeqCst),
            0,
            "no collaborator may be constructed"
        );
        assert!(
            p.op_log().iter().all(|o| o == "admit"),
            "no overlay op may be recorded: {:?}",
            p.op_log()
        );
        assert!(!c.is_active());
        assert_eq!(c.status(), ConverseStatus::Idle);
        assert!(!converse_hooks::session_active());
        drop(dictation);
        c.start()
            .expect("a later start works once dictation released");
        assert!(c.stop());
        wait_until("drain", || !c.is_active());
        reset_hooks();
    }

    /// (e) The default `admit` (fakes that do not override it) claims nothing
    /// and a session still starts and stops as before.
    #[test]
    fn o7_default_admit_claims_nothing_and_a_session_still_runs() {
        let _g = lock();
        reset_hooks();
        let p = FakePlatform::new(Mode::Listening);
        let guard = p.admit();
        assert!(guard.is_ok(), "default admit must succeed");
        drop(guard);
        let c = Arc::new(ConverseController::new(p.clone()));
        let _drain = DrainOnDrop(c.clone());
        assert!(c.start().is_ok());
        assert!(c.stop());
        wait_until("drain", || !c.is_active());
        assert_eq!(c.status(), ConverseStatus::Idle);
        reset_hooks();
    }
}

#[cfg(test)]
#[path = "converse_hooks_tests.rs"]
mod hooks_tests;

#[cfg(test)]
#[path = "converse_latency_wiring_tests.rs"]
mod latency_wiring_tests;

#[cfg(test)]
#[path = "converse_runtime_sink_tests.rs"]
mod runtime_sink_tests;
