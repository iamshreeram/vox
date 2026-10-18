//! Spec for `natural_router` (step S5b). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it. Covers critic findings C2 (router-wide handles, race
//! between the manager's live-queue check and the append) and C3 (failed stop, replacement).
use crate::managers::emotion::Emotion;
use crate::managers::natural_engine::{FailureReason, HealthState, NaturalHealth};
use crate::managers::natural_router::*;
use crate::managers::tts::{SpeechBackend, SpeechHandle, TtsManager};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
struct Rec {
    kind: &'static str, // "spawn" | "styled" | "enqueue"
    text: String,
    voice: Option<String>,
    rate: Option<u32>,
    emotion: Emotion,
}

type Hook = Box<dyn Fn() + Send + Sync>;

struct Route {
    queue: AtomicBool,
    calls: Mutex<Vec<Rec>>,
    running: AtomicBool,
    kill_ok: AtomicBool,
    kills: AtomicUsize,
    fail_next: Mutex<Option<io::ErrorKind>>,
    /// Runs on the next `is_running()` call; that call still returns the PRE-hook value.
    on_next_is_running: Mutex<Option<Hook>>,
}

impl Route {
    fn new(queue: bool) -> Arc<Self> {
        Arc::new(Self {
            queue: AtomicBool::new(queue),
            calls: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            kill_ok: AtomicBool::new(true),
            kills: AtomicUsize::new(0),
            fail_next: Mutex::new(None),
            on_next_is_running: Mutex::new(None),
        })
    }
    fn calls(&self) -> Vec<Rec> {
        self.calls.lock().unwrap().clone()
    }
    fn texts(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c.text).collect()
    }
    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    fn set_running(&self, v: bool) {
        self.running.store(v, Ordering::SeqCst);
    }
    fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }
}

struct Fake(Arc<Route>);

impl Fake {
    fn record(
        &self,
        kind: &'static str,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if let Some(kind_err) = self.0.fail_next.lock().unwrap().take() {
            return Err(io::Error::from(kind_err));
        }
        self.0.calls.lock().unwrap().push(Rec {
            kind,
            text: text.into(),
            voice: voice.map(str::to_string),
            rate,
            emotion,
        });
        self.0.set_running(true);
        Ok(Box::new(FakeHandle(self.0.clone())))
    }
}

impl SpeechBackend for Fake {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.record("spawn", text, voice, rate, Emotion::Neutral)
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        e: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.record("styled", text, voice, rate, e)
    }
    fn supports_queue(&self) -> bool {
        self.0.queue.load(Ordering::SeqCst)
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        e: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if !self.supports_queue() {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        self.record("enqueue", text, voice, rate, e)
    }
}

struct FakeHandle(Arc<Route>);

impl SpeechHandle for FakeHandle {
    fn kill(&mut self) -> bool {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        if self.0.kill_ok.load(Ordering::SeqCst) {
            self.0.set_running(false);
            true
        } else {
            false
        }
    }
    fn is_running(&mut self) -> bool {
        let before = self.0.running();
        if let Some(hook) = self.0.on_next_is_running.lock().unwrap().take() {
            hook();
        }
        before
    }
    fn queue_capable(&self) -> bool {
        self.0.queue.load(Ordering::SeqCst)
    }
}

struct Rig {
    router: EngineBackend,
    system: Arc<Route>,
    fallback: Arc<Route>,
    natural: Arc<Route>,
    health: Arc<NaturalHealth>,
    choice: Arc<Mutex<RouterChoice>>,
    model_ready: Arc<AtomicBool>,
}

fn natural_choice(voice: &str) -> RouterChoice {
    RouterChoice {
        natural: true,
        voice: voice.into(),
    }
}

fn rig_with(system_queue: bool, with_natural: bool, cooldown: Duration) -> Rig {
    let system = Route::new(system_queue);
    let fallback = Route::new(true);
    let natural = Route::new(true);
    let health = NaturalHealth::new(cooldown);
    let choice = Arc::new(Mutex::new(natural_choice("bf_emma")));
    let model_ready = Arc::new(AtomicBool::new(true));
    let (c, m) = (choice.clone(), model_ready.clone());
    let router = EngineBackend::new(
        Box::new(Fake(system.clone())),
        Box::new(Fake(fallback.clone())),
        with_natural.then(|| Box::new(Fake(natural.clone())) as Box<dyn SpeechBackend>),
        health.clone(),
        RouterConfig {
            choice: Arc::new(move || c.lock().unwrap().clone()),
            model_ready: Arc::new(move || m.load(Ordering::SeqCst)),
        },
    );
    Rig {
        router,
        system,
        fallback,
        natural,
        health,
        choice,
        model_ready,
    }
}

fn rig() -> Rig {
    rig_with(false, true, Duration::from_secs(300))
}

impl Rig {
    fn fail_health(&self) {
        self.health
            .report_failure(self.health.revision(), FailureReason::Synth);
        assert!(matches!(self.health.state(), HealthState::Failed(_)));
    }
    fn speak(&self, text: &str) -> io::Result<Box<dyn SpeechHandle>> {
        self.router
            .spawn_styled(text, Some("Samantha"), Some(200), Emotion::Calm)
    }
    fn append(&self, text: &str) -> io::Result<Box<dyn SpeechHandle>> {
        self.router
            .enqueue_styled(text, Some("Samantha"), Some(200), Emotion::Calm)
    }
}

const LATIN: &str = "Hello there, how are you today?";
const CHINESE: &str = "你好，今天天气怎么样？我们出去走走吧。";

// ---- passthrough -------------------------------------------------------------------------------------

#[test]
fn system_mode_is_a_pure_passthrough() {
    let r = rig();
    r.choice.lock().unwrap().natural = false;
    let mut h = r.router.spawn(LATIN, Some("Samantha"), Some(200)).unwrap();
    assert_eq!(r.system.calls()[0].kind, "spawn");
    assert_eq!(r.system.calls()[0].voice.as_deref(), Some("Samantha"));
    assert_eq!(r.system.calls()[0].rate, Some(200));
    r.router
        .spawn_styled("Second.", None, None, Emotion::Excited)
        .unwrap();
    assert_eq!(r.system.calls()[1].kind, "styled");
    assert_eq!(r.system.calls()[1].emotion, Emotion::Excited);
    assert!(!r.router.supports_queue(), "follows the system backend");
    r.system.queue.store(true, Ordering::SeqCst);
    assert!(r.router.supports_queue());
    r.router
        .enqueue_styled("Third.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(r.system.calls()[2].kind, "enqueue");
    assert!(r.natural.calls().is_empty() && r.fallback.calls().is_empty());
    assert!(h.kill());
    assert_eq!(r.system.kills(), 1);
}

#[test]
fn without_a_natural_route_everything_goes_to_the_system_backend() {
    let r = rig_with(true, false, Duration::from_secs(300));
    r.router
        .spawn_styled(LATIN, Some("Samantha"), None, Emotion::Neutral)
        .unwrap();
    assert_eq!(r.system.texts(), vec![LATIN]);
    assert!(r.fallback.calls().is_empty());
}

// ---- natural routing ----------------------------------------------------------------------------------

#[test]
fn a_latin_burst_goes_to_the_natural_route_with_the_natural_voice() {
    let r = rig();
    let h = r.speak(LATIN).unwrap();
    let c = r.natural.calls();
    assert_eq!(c.len(), 1);
    assert_eq!(
        c[0].voice.as_deref(),
        Some("bf_emma"),
        "natural voice, not the system voice"
    );
    assert_eq!(c[0].rate, Some(200));
    assert_eq!(c[0].emotion, Emotion::Calm);
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
    assert!(h.queue_capable());
    assert!(
        r.router.supports_queue(),
        "queue-capable for every text while Natural is selected"
    );
}

#[test]
fn fallbacks_use_the_original_voice_and_the_right_system_route() {
    // (health failed | model missing | non-Latin text) x (system cannot queue | can queue)
    for system_queue in [false, true] {
        for case in ["health", "model", "text"] {
            let r = rig_with(system_queue, true, Duration::from_secs(300));
            let text = match case {
                "health" => {
                    r.fail_health();
                    LATIN
                }
                "model" => {
                    r.model_ready.store(false, Ordering::SeqCst);
                    LATIN
                }
                _ => CHINESE,
            };
            r.speak(text).unwrap();
            assert!(r.natural.calls().is_empty(), "{case}/{system_queue}");
            let target = if system_queue { &r.system } else { &r.fallback };
            let other = if system_queue { &r.fallback } else { &r.system };
            let c = target.calls();
            assert_eq!(c.len(), 1, "{case}/{system_queue}");
            assert_eq!(
                c[0].voice.as_deref(),
                Some("Samantha"),
                "system voice untouched"
            );
            assert!(other.calls().is_empty(), "{case}/{system_queue}");
            assert!(
                r.router.supports_queue(),
                "still queue-capable while Natural is selected"
            );
        }
    }
}

#[test]
fn settings_are_read_on_every_call() {
    let r = rig();
    let mut h = r.speak(LATIN).unwrap();
    h.kill();
    r.choice.lock().unwrap().voice = "am_adam".into();
    r.speak(LATIN).unwrap();
    assert_eq!(
        r.natural.calls()[1].voice.as_deref(),
        Some("am_adam"),
        "voice change applies at once"
    );
    r.router.stop_all();
    r.choice.lock().unwrap().natural = false;
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.calls().len(), 1, "engine change applies at once");
}

#[test]
fn a_failed_health_recovers_through_the_automatic_retry() {
    let r = rig_with(false, true, Duration::from_millis(60));
    r.fail_health();
    r.speak(LATIN).unwrap();
    assert_eq!(r.fallback.calls().len(), 1);
    r.router.stop_all();
    std::thread::sleep(Duration::from_millis(120));
    r.speak(LATIN).unwrap();
    assert_eq!(
        r.natural.calls().len(),
        1,
        "after the cooldown natural is tried again"
    );
    assert!(r.health.is_ready());
}

#[test]
fn a_natural_start_refusal_falls_back_but_would_block_is_propagated() {
    let r = rig();
    *r.natural.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    r.speak(LATIN).expect("fallback speaks");
    assert_eq!(r.fallback.calls().len(), 1);
    r.router.stop_all();
    *r.natural.fail_next.lock().unwrap() = Some(io::ErrorKind::WouldBlock);
    let err = r.speak(LATIN).err().expect("propagated");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(
        r.fallback.calls().len(),
        1,
        "no fallback when the previous stop is unconfirmed"
    );
}

// ---- derived pin ------------------------------------------------------------------------------------------

#[test]
fn appends_stick_to_the_running_route_and_non_latin_appends_are_dropped_and_counted() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.append("Another sentence follows here.").unwrap();
    assert_eq!(r.natural.calls()[1].kind, "enqueue");
    assert_eq!(r.natural.calls()[1].voice.as_deref(), Some("bf_emma"));
    r.append(CHINESE).expect("dropped but accepted");
    assert_eq!(r.natural.calls().len(), 2);
    assert!(r.fallback.calls().is_empty() && r.system.calls().is_empty());
    assert_eq!(r.router.dropped_non_latin(), 1);
}

#[test]
fn the_pin_does_not_stick_after_the_route_drains() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.natural.set_running(false); // drained
    r.append(CHINESE).unwrap();
    assert_eq!(r.natural.calls().len(), 1);
    assert_eq!(
        r.fallback.calls().len(),
        1,
        "idle: chosen afresh, non-Latin goes to the system voice"
    );
    assert_eq!(r.router.dropped_non_latin(), 0);
}

// ---- router-wide handles (C2) ----------------------------------------------------------------------------

#[test]
fn an_old_handle_reports_running_while_any_route_is_running() {
    let r = rig();
    let mut old = r.speak(LATIN).unwrap();
    r.natural.set_running(false);
    r.fail_health();
    r.append("Next sentence arrives now.").unwrap(); // now on the fallback
    assert_eq!(r.fallback.calls().len(), 1);
    assert!(old.is_running(), "router-wide: the fallback is running");
    assert!(old.queue_capable());
    r.fallback.set_running(false);
    assert!(!old.is_running());
}

#[test]
fn kill_stops_every_route_and_is_true_only_if_all_confirmed() {
    let r = rig();
    let mut h = r.speak(LATIN).unwrap();
    r.natural.set_running(false);
    r.fail_health();
    r.append("Fallback sentence here.").unwrap();
    r.natural.set_running(true); // both routes audibly busy
    assert!(h.kill());
    assert_eq!((r.natural.kills(), r.fallback.kills()), (1, 1));
    assert!(!r.natural.running() && !r.fallback.running());

    // One route cannot confirm: every route is still tried and the result is false.
    let r = rig();
    let mut h = r.speak(LATIN).unwrap();
    r.natural.set_running(false);
    r.fail_health();
    r.append("Fallback sentence here.").unwrap();
    r.natural.set_running(true);
    r.natural.kill_ok.store(false, Ordering::SeqCst);
    assert!(!h.kill());
    assert_eq!(
        r.fallback.kills(),
        1,
        "no early return after the first failure"
    );
    assert!(h.is_running(), "still audible on the unconfirmed route");
}

#[test]
fn a_replacement_speaks_after_a_failed_stop_once_the_tail_has_drained() {
    let r = rig();
    let mut h = r.speak(LATIN).unwrap();
    r.natural.kill_ok.store(false, Ordering::SeqCst);
    assert!(!h.kill());
    r.natural.kill_ok.store(true, Ordering::SeqCst);
    r.natural.set_running(false); // the tail expired
    r.speak("A fresh sentence for you.").expect("not wedged");
    assert_eq!(r.natural.calls().len(), 2);
}

#[test]
fn stop_all_stops_every_route() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.natural.set_running(false);
    r.fail_health();
    r.append("Fallback sentence here.").unwrap();
    r.system.set_running(true);
    r.natural.set_running(true);
    assert!(r.router.stop_all());
    assert!(!r.natural.running() && !r.fallback.running());
    assert_eq!(r.fallback.kills(), 1);
}

// ---- through the real TtsManager (C2/C3) ---------------------------------------------------------------

fn manager(r: Rig) -> (TtsManager, Rig) {
    // The router moves into the manager; keep clones of everything the test observes.
    let Rig {
        router,
        system,
        fallback,
        natural,
        health,
        choice,
        model_ready,
    } = r;
    let mgr = TtsManager::new(Box::new(router));
    let observer = Rig {
        router: EngineBackend::new(
            Box::new(Fake(system.clone())),
            Box::new(Fake(fallback.clone())),
            None,
            health.clone(),
            RouterConfig {
                choice: Arc::new(|| natural_choice("af_heart")),
                model_ready: Arc::new(|| true),
            },
        ),
        system,
        fallback,
        natural,
        health,
        choice,
        model_ready,
    };
    (mgr, observer)
}

#[test]
fn the_manager_appends_to_the_same_route_and_stop_silences_it() {
    let (mgr, o) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(o.natural.texts().len(), 2);
    assert_eq!(
        o.natural.kills(),
        0,
        "queued appends never kill earlier speech"
    );
    assert!(mgr.is_speaking());
    assert!(mgr.stop());
    assert!(!o.natural.running());
    assert!(o.natural.kills() >= 1);
}

#[test]
fn the_manager_keeps_tracking_when_the_route_drains_between_its_check_and_the_append() {
    let (mgr, o) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    // The manager's live-queue check polls `is_running()`: the hook makes natural drain and
    // fail right after that poll answered "running", before the router appends.
    let (natural, health) = (o.natural.clone(), o.health.clone());
    *o.natural.on_next_is_running.lock().unwrap() = Some(Box::new(move || {
        natural.set_running(false);
        health.report_failure(health.revision(), FailureReason::Synth);
    }));
    mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(o.natural.texts().len(), 1);
    assert_eq!(
        o.fallback.texts().len(),
        1,
        "the append went to the fallback"
    );
    assert!(
        mgr.is_speaking(),
        "the manager's (router-wide) handle sees the fallback"
    );
    mgr.speak_queued_if_current(epoch, "Third sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(
        o.fallback.texts().len(),
        2,
        "third append sticks to the running fallback"
    );
    assert_eq!(o.natural.texts().len(), 1);
    assert!(mgr.stop());
    assert!(!o.fallback.running());
}

#[test]
fn recording_start_stops_natural_speech_and_blocks_new_speech() {
    let (mgr, o) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    mgr.note_recording_started();
    assert!(
        !o.natural.running(),
        "recording start must silence the natural voice"
    );
    assert!(!mgr.is_speaking());
    let blocked =
        mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral);
    assert!(blocked.is_err());
    assert_eq!(o.natural.texts().len(), 1);
}
