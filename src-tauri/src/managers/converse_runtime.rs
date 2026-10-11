//! Session controller: owns the lifecycle of one conversation session at a
//! time. Pure logic behind an injected `Platform` (the real one lives in
//! `converse_platform.rs`), so ordering/cleanup guarantees are unit-tested
//! with fakes. Contract: DESIGN.md 3.3 (registry), 3.6 (ConverseManager) and
//! the AMENDMENTS section (Revision 4).

use crate::managers::converse::{
    run_session, Brain, Deps, Ears, Hands, Mouth, RegistryState, Reporter, SessionConfig,
    SessionEnd, SessionRegistry, SessionState,
};
use crate::managers::converse_capture::CaptureHandle;
use crate::managers::converse_hooks;
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
}

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
}

impl Reporter for ForwardReporter {
    fn state(&self, state: SessionState) {
        if state != SessionState::Idle {
            self.platform.on_state(state_label(state), None);
        }
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

    /// Start a session. Order: `platform.preflight()` (Err => return it, the
    /// registry is untouched and no hook is set) -> `registry.try_begin()`
    /// (None => `Err(ALREADY_RUNNING)`) -> `converse_hooks::set_session_active(true)`
    /// -> `platform.capture()` -> `converse_hooks::install_capture(Some(..))`
    /// -> `platform.parts()` -> spawn the session task on the tauri async
    /// runtime. Returns the session id.
    ///
    /// The session task runs `run_session` under `catch_unwind` (a panic counts
    /// as `Cancelled`), then the FINALIZER runs, always, in this exact order:
    /// (1) `capture.shutdown()` on a blocking thread and awaited,
    /// (2) `install_capture(None)`, (3) `set_session_active(false)`,
    /// (4) `platform.on_state("idle", Some(end_label(end)))`,
    /// (5) `registry.finish(id)`.
    /// While running, the session's `Reporter` forwards every state except
    /// `Idle` (the final idle is emitted only by the finalizer, exactly once).
    pub fn start(&self) -> Result<u64, String> {
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
        tauri::async_runtime::spawn(async move {
            let end = {
                let SessionParts {
                    ears,
                    mouth,
                    hands,
                    brain,
                    cfg,
                } = parts;
                let reporter = ForwardReporter {
                    platform: platform.clone(),
                };
                let deps = Deps {
                    ears: ears.as_ref(),
                    mouth: mouth.as_ref(),
                    hands: hands.as_ref(),
                    brain: brain.as_ref(),
                    reporter: &reporter,
                };
                FutureExt::catch_unwind(AssertUnwindSafe(run_session(&deps, &cfg, cancel)))
                    .await
                    .unwrap_or(SessionEnd::Cancelled)
            };
            // Finalizer: exact order, always runs.
            let _ = tauri::async_runtime::spawn_blocking(move || capture.shutdown()).await;
            converse_hooks::install_capture(None);
            converse_hooks::set_session_active(false);
            platform.on_state("idle", Some(end_label(end)));
            registry.finish(id);
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
    use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
    use crate::managers::emotion::Emotion;
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
}
