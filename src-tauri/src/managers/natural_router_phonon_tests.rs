//! Spec for the Phonon peer slot of `natural_router` (P4a, DESIGN.md Rev 4 section 3 "P4 Routing +
//! wiring", section 5, A.5/A.6). Mirrors `natural_router_tests.rs` (same fake, health and manager
//! patterns). Authoritative: do not weaken an assertion; if a test is wrong, report it instead.
//!
//! Contract assumed by these tests (the production writer must match it):
//! * `RouterChoice { natural, voice, phonon }`.
//! * `RouterConfig { choice, model_ready, phonon_ready }`.
//! * `EngineBackend::with_phonon(system, system_queue, natural, health, phonon, phonon_health,
//!   config)`; `EngineBackend::new` keeps its arity and means "no Phonon route".
//! * `EngineBackend::stop_all`, `dropped_non_latin` and the `SpeechBackend` impl are unchanged.
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
    phonon: Arc<Route>,
    phonon_health: Arc<NaturalHealth>,
    choice: Arc<Mutex<RouterChoice>>,
    phonon_ready: Arc<AtomicBool>,
}

/// Phonon selected (the natural flag is off), voice as `tts_natural_voice` would carry it.
fn phonon_choice(voice: &str) -> RouterChoice {
    RouterChoice {
        natural: false,
        voice: voice.into(),
        phonon: true,
    }
}

fn natural_choice(voice: &str) -> RouterChoice {
    RouterChoice {
        natural: true,
        voice: voice.into(),
        phonon: false,
    }
}

/// `system_queue` decides whether the system backend can queue (and so is the fallback for a
/// natural or Phonon burst); `with_phonon` decides whether a Phonon backend is built at all.
/// `phonon_cooldown` is the Phonon health auto-retry cooldown.
fn rig_with(system_queue: bool, with_phonon: bool, phonon_cooldown: Duration) -> Rig {
    let system = Route::new(system_queue);
    let fallback = Route::new(true);
    let natural = Route::new(true);
    let phonon = Route::new(true);
    let natural_health = NaturalHealth::new(Duration::from_secs(300));
    let phonon_health = NaturalHealth::new(phonon_cooldown);
    let choice = Arc::new(Mutex::new(phonon_choice("bf_emma")));
    let model_ready = Arc::new(AtomicBool::new(true));
    let phonon_ready = Arc::new(AtomicBool::new(true));
    let (c, m, p) = (choice.clone(), model_ready, phonon_ready.clone());
    let router = EngineBackend::with_phonon(
        Box::new(Fake(system.clone())),
        Box::new(Fake(fallback.clone())),
        Some(Box::new(Fake(natural.clone())) as Box<dyn SpeechBackend>),
        natural_health,
        with_phonon.then(|| Box::new(Fake(phonon.clone())) as Box<dyn SpeechBackend>),
        phonon_health.clone(),
        RouterConfig {
            choice: Arc::new(move || c.lock().unwrap().clone()),
            model_ready: Arc::new(move || m.load(Ordering::SeqCst)),
            phonon_ready: Arc::new(move || p.load(Ordering::SeqCst)),
        },
    );
    Rig {
        router,
        system,
        fallback,
        natural,
        phonon,
        phonon_health,
        choice,
        phonon_ready,
    }
}

/// Phonon selected, every condition holds, system queue-capable (fallback = `system`).
fn rig() -> Rig {
    rig_with(true, true, Duration::from_secs(300))
}

impl Rig {
    fn fail_phonon_health(&self) {
        self.phonon_health
            .report_failure(self.phonon_health.revision(), FailureReason::Synth);
        assert!(matches!(self.phonon_health.state(), HealthState::Failed(_)));
    }
    fn speak(&self, text: &str) -> io::Result<Box<dyn SpeechHandle>> {
        self.router
            .spawn_styled(text, Some("Samantha"), Some(200), Emotion::Calm)
    }
    fn append(&self, text: &str) -> io::Result<Box<dyn SpeechHandle>> {
        self.router
            .enqueue_styled(text, Some("Samantha"), Some(200), Emotion::Calm)
    }
    /// How many times `text` reached any route (system, fallback, natural, Phonon).
    fn spoken_count(&self, text: &str) -> usize {
        [&self.system, &self.fallback, &self.natural, &self.phonon]
            .iter()
            .map(|r| r.texts().iter().filter(|t| t.as_str() == text).count())
            .sum()
    }
    /// Target of a fallback from a Phonon or natural burst: the system backend when it can
    /// queue, otherwise the queue-capable system route.
    fn fallback_target(&self, system_queue: bool) -> &Arc<Route> {
        if system_queue {
            &self.system
        } else {
            &self.fallback
        }
    }
}

const LATIN: &str = "Hello there, how are you today?";
const SPANISH: &str = "Hola amigo, gracias por todo.";
const FRENCH: &str = "Bonjour mon ami, merci beaucoup";
const CJK: &str = "你好世界";

// ---- happy path --------------------------------------------------------------------------------

#[test]
fn an_english_burst_goes_to_phonon_and_nothing_else() {
    let r = rig();
    let h = r.speak(LATIN).unwrap();
    let c = r.phonon.calls();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].kind, "styled");
    assert_eq!(c[0].rate, Some(200));
    assert_eq!(c[0].emotion, Emotion::Calm);
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
    assert!(r.natural.calls().is_empty(), "natural is not chosen");
    assert!(h.queue_capable());
    assert!(
        r.router.supports_queue(),
        "queue-capable while Phonon is selected"
    );
}

// ---- each gate alone sends the burst to the system route ----------------------------------------

#[test]
fn phonon_is_not_used_when_the_choice_does_not_select_it() {
    let r = rig();
    *r.choice.lock().unwrap() = RouterChoice {
        natural: false,
        voice: "bf_emma".into(),
        phonon: false,
    };
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.texts(), vec![LATIN]);
    assert!(r.phonon.calls().is_empty());
    assert!(r.natural.calls().is_empty() && r.fallback.calls().is_empty());
}

#[test]
fn phonon_is_not_used_without_a_phonon_backend() {
    let r = rig_with(true, false, Duration::from_secs(300));
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.texts(), vec![LATIN]);
    assert!(r.phonon.calls().is_empty());
    assert!(r.natural.calls().is_empty());
}

#[test]
fn phonon_is_not_used_while_its_health_is_failed() {
    let r = rig();
    r.fail_phonon_health();
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.texts(), vec![LATIN]);
    assert!(r.phonon.calls().is_empty());
    assert!(r.natural.calls().is_empty());
}

#[test]
fn phonon_is_not_used_while_phonon_ready_is_false() {
    let r = rig();
    r.phonon_ready.store(false, Ordering::SeqCst);
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.texts(), vec![LATIN]);
    assert!(r.phonon.calls().is_empty());
    assert!(r.natural.calls().is_empty());
}

#[test]
fn a_non_english_first_sentence_stays_off_phonon() {
    for text in [SPANISH, FRENCH, CJK] {
        let r = rig();
        r.speak(text).unwrap();
        assert_eq!(r.system.texts(), vec![text.to_string()], "{text}");
        assert!(r.phonon.calls().is_empty(), "{text}");
        assert!(r.natural.calls().is_empty(), "{text}");
    }
}

#[test]
fn a_non_speakable_first_sentence_stays_off_phonon() {
    // Same rule as the natural route: no alphabetic characters means not Phonon.
    for text in ["", "   ", "... --- ???"] {
        let r = rig();
        r.speak(text).unwrap();
        assert_eq!(r.system.texts(), vec![text.to_string()], "{text:?}");
        assert!(r.phonon.calls().is_empty(), "{text:?}");
    }
}

#[test]
fn a_phonon_ineligible_burst_is_a_passthrough_to_the_system_route_for_either_queue_capability() {
    // natural=false: the existing passthrough applies unchanged (no fallback route involved).
    for system_queue in [false, true] {
        let r = rig_with(system_queue, true, Duration::from_secs(300));
        r.fail_phonon_health();
        r.speak(LATIN).unwrap();
        assert_eq!(
            r.system.texts(),
            vec![LATIN.to_string()],
            "system_queue={system_queue}"
        );
        assert!(r.fallback.calls().is_empty(), "system_queue={system_queue}");
        assert!(r.phonon.calls().is_empty(), "system_queue={system_queue}");
        assert!(r.natural.calls().is_empty(), "system_queue={system_queue}");
    }
}

// ---- interaction with the natural choice (A.5 / P4) ----------------------------------------------

#[test]
fn the_natural_choice_alone_is_unchanged_by_the_phonon_slot() {
    let r = rig();
    *r.choice.lock().unwrap() = natural_choice("am_adam");
    r.speak(LATIN).unwrap();
    let c = r.natural.calls();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].voice.as_deref(), Some("am_adam"));
    assert!(r.phonon.calls().is_empty(), "Phonon not chosen");
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
}

#[test]
fn both_flags_with_phonon_not_ready_use_the_natural_route() {
    let r = rig();
    *r.choice.lock().unwrap() = RouterChoice {
        natural: true,
        voice: "bf_emma".into(),
        phonon: true,
    };
    r.phonon_ready.store(false, Ordering::SeqCst);
    r.speak(LATIN).unwrap();
    assert_eq!(r.natural.texts(), vec![LATIN]);
    assert!(r.phonon.calls().is_empty());
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
}

#[test]
fn both_flags_with_every_phonon_condition_met_pick_phonon() {
    let r = rig();
    *r.choice.lock().unwrap() = RouterChoice {
        natural: true,
        voice: "bf_emma".into(),
        phonon: true,
    };
    r.speak(LATIN).unwrap();
    assert_eq!(r.phonon.texts(), vec![LATIN]);
    assert!(r.natural.calls().is_empty());
}

#[test]
fn a_failed_phonon_health_does_not_touch_the_natural_route() {
    let r = rig();
    *r.choice.lock().unwrap() = RouterChoice {
        natural: true,
        voice: "bf_emma".into(),
        phonon: true,
    };
    r.fail_phonon_health();
    r.speak(LATIN).unwrap();
    assert_eq!(r.natural.texts(), vec![LATIN], "natural is healthy");
    assert!(r.phonon.calls().is_empty());
}

// ---- burst stability (P4: never two routes, never re-route mid-burst) ---------------------------

#[test]
fn later_sentences_of_a_phonon_burst_stay_on_phonon_even_when_not_english() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.append(SPANISH).unwrap();
    assert_eq!(
        r.phonon.texts(),
        vec![LATIN.to_string(), SPANISH.to_string()]
    );
    assert_eq!(r.phonon.calls()[1].kind, "enqueue");
    assert!(
        r.system.calls().is_empty() && r.fallback.calls().is_empty(),
        "the system voice never receives the Spanish sentence"
    );
    assert!(r.natural.calls().is_empty());
}

#[test]
fn a_new_burst_after_the_previous_one_drained_is_decided_afresh() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.phonon.set_running(false); // drained
    r.speak(SPANISH).unwrap();
    assert_eq!(r.phonon.texts().len(), 1, "no second Phonon burst");
    assert_eq!(
        r.system.texts(),
        vec![SPANISH.to_string()],
        "a new non-English burst goes to the system voice"
    );
}

#[test]
fn a_mid_burst_health_failure_and_choice_change_do_not_move_the_burst() {
    let r = rig();
    r.speak(LATIN).unwrap();
    r.fail_phonon_health();
    r.phonon_ready.store(false, Ordering::SeqCst);
    *r.choice.lock().unwrap() = natural_choice("am_adam");
    r.append("Next sentence arrives now.").unwrap();
    assert_eq!(
        r.phonon.texts(),
        vec![LATIN.to_string(), "Next sentence arrives now.".to_string()],
        "the running Phonon burst keeps its sentences"
    );
    assert!(r.natural.calls().is_empty(), "never two routes at once");
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
}

#[test]
fn changing_the_choice_mid_burst_does_not_move_the_burst() {
    let r = rig();
    r.speak(LATIN).unwrap();
    *r.choice.lock().unwrap() = natural_choice("bf_emma");
    r.append("Second sentence here.").unwrap();
    *r.choice.lock().unwrap() = RouterChoice {
        natural: false,
        voice: "bf_emma".into(),
        phonon: false,
    };
    r.append("Third sentence here.").unwrap();
    assert_eq!(r.phonon.texts().len(), 3);
    assert!(r.natural.calls().is_empty());
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
}

// ---- synchronous errors (mirror of the natural route) -------------------------------------------

#[test]
fn a_sync_error_from_phonon_retries_once_on_the_fallback() {
    for system_queue in [false, true] {
        let r = rig_with(system_queue, true, Duration::from_secs(300));
        *r.phonon.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
        r.speak(LATIN).expect("the fallback speaks");
        assert!(r.phonon.calls().is_empty(), "system_queue={system_queue}");
        assert_eq!(
            r.fallback_target(system_queue).texts(),
            vec![LATIN.to_string()],
            "system_queue={system_queue}"
        );
        assert_eq!(
            r.spoken_count(LATIN),
            1,
            "spoken exactly once overall (system_queue={system_queue})"
        );
        assert!(r.natural.calls().is_empty(), "system_queue={system_queue}");
    }
}

#[test]
fn a_would_block_from_phonon_is_propagated_without_a_fallback() {
    let r = rig();
    *r.phonon.fail_next.lock().unwrap() = Some(io::ErrorKind::WouldBlock);
    let err = r.speak(LATIN).err().expect("propagated");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert!(r.system.calls().is_empty() && r.fallback.calls().is_empty());
    assert_eq!(r.spoken_count(LATIN), 0);
}

#[test]
fn a_phonon_error_after_the_fallback_consumed_the_burst_does_not_loop() {
    let r = rig();
    *r.phonon.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    r.speak(LATIN).expect("the fallback speaks");
    r.append("Second sentence here.")
        .expect("the running fallback takes the append");
    assert!(
        r.phonon.calls().is_empty(),
        "a failed Phonon start is never retried on Phonon"
    );
    assert_eq!(
        r.system.texts(),
        vec![LATIN.to_string(), "Second sentence here.".to_string()]
    );
}

#[test]
fn phonon_and_the_fallback_both_failing_returns_an_error_without_looping() {
    let r = rig();
    *r.phonon.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    *r.system.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    assert!(r.speak(LATIN).is_err(), "no third attempt");
    assert!(
        r.phonon.calls().is_empty(),
        "Phonon was not re-entered after its own failure"
    );
    assert!(r.system.calls().is_empty());
    assert_eq!(r.spoken_count(LATIN), 0);
}

// ---- health (mirror of the natural route) ---------------------------------------------------------

#[test]
fn a_failed_phonon_health_recovers_through_the_automatic_retry() {
    let r = rig_with(true, true, Duration::from_millis(60));
    r.fail_phonon_health();
    r.speak(LATIN).unwrap();
    assert_eq!(r.system.texts().len(), 1, "fallback while Phonon is failed");
    r.router.stop_all();
    // The cooldown is measured on the health's own clock, so poll for the retry instead of a
    // fixed sleep: each speak inside the cooldown goes to the fallback; the first speak after it
    // reaches Phonon. Bounded at 30 s, returns as soon as Phonon is tried.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while r.phonon.texts().is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        r.speak(LATIN).unwrap();
    }
    assert_eq!(
        r.phonon.texts().len(),
        1,
        "after the cooldown Phonon is tried again"
    );
    assert!(r.phonon_health.is_ready());
}

// ---- through the real TtsManager (mirror of the natural manager tests) ----------------------------

/// The probes a manager test needs once the router has moved into the manager.
struct Probes {
    system: Arc<Route>,
    natural: Arc<Route>,
    phonon: Arc<Route>,
    phonon_health: Arc<NaturalHealth>,
}

fn manager(r: Rig) -> (TtsManager, Probes) {
    let Rig {
        router,
        system,
        natural,
        phonon,
        phonon_health,
        ..
    } = r;
    let mgr = TtsManager::new(Box::new(router));
    let probes = Probes {
        system,
        natural,
        phonon,
        phonon_health,
    };
    (mgr, probes)
}

#[test]
fn the_manager_appends_to_the_phonon_route_and_stop_silences_it() {
    let (mgr, p) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(p.phonon.texts().len(), 2);
    assert_eq!(
        p.phonon.kills(),
        0,
        "queued appends never kill earlier speech"
    );
    assert!(mgr.is_speaking());
    assert!(mgr.stop());
    assert!(!p.phonon.running());
    assert!(p.phonon.kills() >= 1);
    assert!(p.system.calls().is_empty() && p.natural.calls().is_empty());
}

#[test]
fn the_manager_keeps_tracking_when_phonon_drains_between_its_check_and_the_append() {
    let (mgr, p) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    // The manager's live-queue check polls `is_running()`: the hook makes Phonon drain and fail
    // right after that poll answered "running", before the router appends.
    let (phonon, health) = (p.phonon.clone(), p.phonon_health.clone());
    *p.phonon.on_next_is_running.lock().unwrap() = Some(Box::new(move || {
        phonon.set_running(false);
        health.report_failure(health.revision(), FailureReason::Synth);
    }));
    mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(p.phonon.texts().len(), 1);
    assert_eq!(p.system.texts().len(), 1, "the append went to the fallback");
    assert!(
        mgr.is_speaking(),
        "the manager's (router-wide) handle sees the fallback"
    );
    mgr.speak_queued_if_current(epoch, "Third sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    assert_eq!(
        p.system.texts().len(),
        2,
        "third append sticks to the running fallback"
    );
    assert_eq!(p.phonon.texts().len(), 1);
    assert!(mgr.stop());
    assert!(!p.system.running());
}

#[test]
fn recording_start_stops_phonon_speech_and_blocks_new_speech() {
    let (mgr, p) = manager(rig());
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, "First sentence here.", None, None, Emotion::Neutral)
        .unwrap();
    mgr.note_recording_started();
    assert!(
        !p.phonon.running(),
        "recording start must silence the Phonon voice"
    );
    assert!(!mgr.is_speaking());
    let blocked =
        mgr.speak_queued_if_current(epoch, "Second sentence here.", None, None, Emotion::Neutral);
    assert!(blocked.is_err());
    assert_eq!(p.phonon.texts().len(), 1);
}
