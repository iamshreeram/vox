//! Spec for the Tamil route of `natural_router` (S6: DESIGN.md R2-A, R2-B and the R4 amendment,
//! "tracking mode"). Each test maps to a scenario named in the step brief (A1..H1). Fakes keep
//! one ordered call log per rig ("tamil.styled", "tamil.kill", "system.spawn", ...) so the order
//! of kills and spawns is asserted, not guessed. Authoritative: do not weaken an assertion.
use crate::managers::emotion::Emotion;
use crate::managers::natural_chunk::is_mostly_tamil;
use crate::managers::natural_engine::{FailureReason, HealthState, NaturalHealth};
use crate::managers::natural_router::*;
use crate::managers::tts::{SpeechBackend, SpeechHandle, TtsManager};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TAMIL_ONE: &str = "இன்று நல்ல நாள். நாம் வெளியே போகலாம்.";
const TAMIL_TWO: &str = "மழை பெய்யத் தொடங்கியது.";
const ENGLISH: &str = "Hello there, how are you today?";
const ENGLISH_TWO: &str = "Only English words are in this sentence.";
/// Majority Tamil with two English words: still a Tamil sentence.
const TAMIL_WITH_TWO_ENGLISH_WORDS: &str = "இன்று நல்ல நாள் மிகவும் அருமையாக இருக்கிறது ok thanks";
/// One Tamil word and ten English words: an English sentence.
const ONE_TAMIL_WORD_TEN_ENGLISH: &str = "வணக்கம் hello there my dear friend how are you doing today";

/// Ordered call log shared by every route of one rig.
type Log = Arc<Mutex<Vec<String>>>;

/// Counts the routes running right now and every moment two of them ran at once.
#[derive(Default)]
struct Tracker {
    active: AtomicUsize,
    violations: AtomicUsize,
}

#[derive(Clone, Debug, PartialEq)]
struct Rec {
    kind: &'static str, // "spawn" | "styled" | "enqueue"
    text: String,
    voice: Option<String>,
    rate: Option<u32>,
    emotion: Emotion,
}

struct Route {
    name: &'static str,
    log: Log,
    tracker: Arc<Tracker>,
    queue: AtomicBool,
    running: AtomicBool,
    kill_ok: AtomicBool,
    kills: AtomicUsize,
    calls: Mutex<Vec<Rec>>,
    fail_next: Mutex<Option<io::ErrorKind>>,
}

impl Route {
    fn new(name: &'static str, log: Log, tracker: Arc<Tracker>, queue: bool) -> Arc<Self> {
        Arc::new(Self {
            name,
            log,
            tracker,
            queue: AtomicBool::new(queue),
            running: AtomicBool::new(false),
            kill_ok: AtomicBool::new(true),
            kills: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
            fail_next: Mutex::new(None),
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
    /// Test-side and handle-side state change. Turning a route on while another one runs is
    /// recorded as an overlap in the shared tracker.
    fn set_running(&self, on: bool) {
        if on {
            if !self.running.swap(true, Ordering::SeqCst) {
                let now = self.tracker.active.fetch_add(1, Ordering::SeqCst) + 1;
                if now > 1 {
                    self.tracker.violations.fetch_add(1, Ordering::SeqCst);
                }
            }
        } else if self.running.swap(false, Ordering::SeqCst) {
            self.tracker.active.fetch_sub(1, Ordering::SeqCst);
        }
    }
    fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }
    fn log_entry(&self, what: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}.{}", self.name, what));
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
        let route = &self.0;
        if let Some(kind_err) = route.fail_next.lock().unwrap().take() {
            return Err(io::Error::from(kind_err));
        }
        route.log_entry(kind);
        route.calls.lock().unwrap().push(Rec {
            kind,
            text: text.into(),
            voice: voice.map(str::to_string),
            rate,
            emotion,
        });
        route.set_running(true);
        Ok(Box::new(FakeHandle {
            route: route.clone(),
            own_queue: route.queue.load(Ordering::SeqCst),
        }))
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
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.record("styled", text, voice, rate, emotion)
    }
    fn supports_queue(&self) -> bool {
        self.0.queue.load(Ordering::SeqCst)
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if !self.supports_queue() {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        self.record("enqueue", text, voice, rate, emotion)
    }
}

/// The handle a fake returns. `own_queue` is what the fake's own handle reports, so a test can
/// tell the system's own handle (passthrough) from a router `RoutedHandle` (always queue-capable).
struct FakeHandle {
    route: Arc<Route>,
    own_queue: bool,
}

impl SpeechHandle for FakeHandle {
    fn kill(&mut self) -> bool {
        self.route.kills.fetch_add(1, Ordering::SeqCst);
        self.route.log_entry("kill");
        if self.route.kill_ok.load(Ordering::SeqCst) {
            self.route.set_running(false);
            true
        } else {
            false
        }
    }
    fn is_running(&mut self) -> bool {
        self.route.running()
    }
    fn queue_capable(&self) -> bool {
        self.own_queue
    }
}

/// Which routes a rig builds, and what the router is told about the user's choice.
struct Setup {
    system_queue: bool,
    natural: bool,
    phonon: bool,
    tamil: bool,
    choice: RouterChoice,
}

impl Setup {
    /// The system route (queue-capable or not), a Tamil route, and nothing else selected.
    fn new(system_queue: bool) -> Self {
        Self {
            system_queue,
            natural: false,
            phonon: false,
            tamil: true,
            choice: RouterChoice {
                natural: false,
                voice: "bf_emma".into(),
                phonon: false,
            },
        }
    }
    fn with_natural(mut self) -> Self {
        self.natural = true;
        self.choice.natural = true;
        self
    }
    fn with_phonon(mut self) -> Self {
        self.phonon = true;
        self.choice.phonon = true;
        self
    }
    fn without_tamil(mut self) -> Self {
        self.tamil = false;
        self
    }
}

/// Every route and knob of one rig. The router itself is returned separately so it can move
/// into a `TtsManager`.
struct Handles {
    log: Log,
    tracker: Arc<Tracker>,
    system: Arc<Route>,
    /// The queue-capable fallback route (`system_queue`).
    fallback: Arc<Route>,
    natural: Arc<Route>,
    phonon: Arc<Route>,
    tamil: Arc<Route>,
    tamil_health: Arc<NaturalHealth>,
    enabled: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
}

impl Handles {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn build(s: Setup) -> (EngineBackend, Handles) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let tracker = Arc::new(Tracker::default());
    let system = Route::new("system", log.clone(), tracker.clone(), s.system_queue);
    let fallback = Route::new("system_queue", log.clone(), tracker.clone(), true);
    let natural = Route::new("natural", log.clone(), tracker.clone(), true);
    let phonon = Route::new("phonon", log.clone(), tracker.clone(), true);
    let tamil = Route::new("tamil", log.clone(), tracker.clone(), true);
    let tamil_health = NaturalHealth::new(Duration::from_secs(300));
    let enabled = Arc::new(AtomicBool::new(true));
    let ready = Arc::new(AtomicBool::new(true));
    let choice = Arc::new(Mutex::new(s.choice));

    let c = choice.clone();
    let mut router = EngineBackend::with_phonon(
        Box::new(Fake(system.clone())),
        Box::new(Fake(fallback.clone())),
        s.natural
            .then(|| Box::new(Fake(natural.clone())) as Box<dyn SpeechBackend>),
        NaturalHealth::new(Duration::from_secs(300)),
        s.phonon
            .then(|| Box::new(Fake(phonon.clone())) as Box<dyn SpeechBackend>),
        NaturalHealth::new(Duration::from_secs(300)),
        RouterConfig {
            choice: Arc::new(move || c.lock().unwrap().clone()),
            model_ready: Arc::new(|| true),
            phonon_ready: Arc::new(|| true),
        },
    );
    if s.tamil {
        let (e, r) = (enabled.clone(), ready.clone());
        router = router.with_tamil_route(TamilRoute {
            backend: Box::new(Fake(tamil.clone())),
            health: tamil_health.clone(),
            enabled: Arc::new(move || e.load(Ordering::SeqCst)),
            ready: Arc::new(move || r.load(Ordering::SeqCst)),
        });
    }
    let handles = Handles {
        log,
        tracker,
        system,
        fallback,
        natural,
        phonon,
        tamil,
        tamil_health,
        enabled,
        ready,
    };
    (router, handles)
}

fn rec(kind: &'static str, text: &str, voice: Option<&str>, emotion: Emotion) -> Rec {
    Rec {
        kind,
        text: text.into(),
        voice: voice.map(str::to_string),
        rate: Some(200),
        emotion,
    }
}

// ---- A: English is untouched, Tamil text wins -------------------------------------------------

#[test]
fn a1_non_queueing_system_english_passes_through_while_tamil_is_active() {
    let (router, h) = build(Setup::new(false));
    assert!(
        !router.supports_queue(),
        "supports_queue follows the system (it never consults Tamil)"
    );
    router
        .spawn(ENGLISH, Some("Samantha"), Some(200))
        .expect("english spawn");
    let mut handle = router
        .spawn_styled(ENGLISH, Some("Samantha"), Some(200), Emotion::Calm)
        .expect("english styled spawn");
    assert_eq!(
        h.system.calls(),
        vec![
            rec("spawn", ENGLISH, Some("Samantha"), Emotion::Neutral),
            rec("styled", ENGLISH, Some("Samantha"), Emotion::Calm),
        ],
        "identical args to the system"
    );
    assert!(
        handle.queue_capable(),
        "returned handle is the router's RoutedHandle"
    );
    assert!(handle.kill(), "kill reaches the system handle");
    assert_eq!(h.log(), ["system.spawn", "system.styled", "system.kill"]);
    let err = router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .err()
        .expect("english enqueue is refused as the system refuses it");
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    assert!(h.tamil.calls().is_empty() && h.fallback.calls().is_empty());
}

#[test]
fn a2_non_queueing_system_tamil_goes_to_tamil_once_and_english_kills_it_first() {
    let (router, h) = build(Setup::new(false));
    let mut tamil_handle = router
        .spawn_styled(TAMIL_ONE, Some("Samantha"), Some(200), Emotion::Calm)
        .expect("tamil spawn");
    assert_eq!(h.tamil.calls().len(), 1, "tamil route reached once");
    assert_eq!(
        h.tamil.calls()[0].voice,
        None,
        "the Tamil route takes no voice"
    );
    assert!(h.system.calls().is_empty(), "system not called");
    assert!(tamil_handle.is_running());

    router
        .spawn(ENGLISH, Some("Samantha"), Some(200))
        .expect("english spawn");
    assert_eq!(
        h.log(),
        ["tamil.styled", "tamil.kill", "system.spawn"],
        "tamil is killed before the system spawn"
    );
    assert!(!h.tamil.running());
    assert!(
        tamil_handle.is_running(),
        "router-wide: the system's english burst is running"
    );
    assert!(tamil_handle.kill());
    assert!(!h.system.running());
}

#[test]
fn a3_tamil_wins_over_selected_phonon_and_natural_only_for_tamil_text() {
    let (router, h) = build(Setup::new(true).with_natural().with_phonon());
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil");
    assert_eq!(h.log(), ["tamil.styled"]);
    router
        .spawn(ENGLISH, None, None)
        .expect("english still goes to phonon");
    assert_eq!(h.log(), ["tamil.styled", "tamil.kill", "phonon.styled"]);
    assert_eq!(h.phonon.texts(), [ENGLISH]);
    assert!(
        h.natural.calls().is_empty(),
        "natural not used for english here"
    );
    assert_eq!(h.tamil.calls().len(), 1, "tamil never called for english");

    let (router, h) = build(Setup::new(true).with_natural());
    router
        .spawn_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english goes to natural as before");
    assert_eq!(h.log(), ["natural.styled"]);
    assert_eq!(h.natural.calls()[0].voice.as_deref(), Some("bf_emma"));
    assert!(h.tamil.calls().is_empty());

    let (router, h) = build(Setup::new(true).with_natural());
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil beats natural");
    assert_eq!(h.log(), ["tamil.styled"]);
    assert!(h.natural.calls().is_empty());
}

#[test]
fn a4_the_language_gate_is_a_majority_test() {
    assert!(!is_mostly_tamil(ONE_TAMIL_WORD_TEN_ENGLISH), "precondition");
    assert!(
        is_mostly_tamil(TAMIL_WITH_TWO_ENGLISH_WORDS),
        "precondition"
    );

    let (router, h) = build(Setup::new(false));
    router
        .spawn_styled(ONE_TAMIL_WORD_TEN_ENGLISH, None, None, Emotion::Neutral)
        .expect("english path");
    assert_eq!(h.log(), ["system.styled"]);
    assert!(h.tamil.calls().is_empty());

    let (router, h) = build(Setup::new(false));
    router
        .spawn_styled(TAMIL_WITH_TWO_ENGLISH_WORDS, None, None, Emotion::Neutral)
        .expect("tamil path");
    assert_eq!(h.log(), ["tamil.styled"]);
    assert!(h.system.calls().is_empty());
}

// ---- B: Tamil falls back to the system exactly as today ---------------------------------------

#[test]
fn b1_without_a_tamil_route_tamil_text_goes_to_the_system_as_today() {
    let (router, h) = build(Setup::new(false).without_tamil());
    router
        .spawn_styled(TAMIL_ONE, Some("Samantha"), Some(200), Emotion::Calm)
        .expect("system speaks");
    assert_eq!(h.log(), ["system.styled"]);
    assert_eq!(h.system.calls()[0].voice.as_deref(), Some("Samantha"));
}

#[test]
fn b2_a_disabled_tamil_route_is_not_chosen() {
    let (router, h) = build(Setup::new(false));
    h.enabled.store(false, Ordering::SeqCst);
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("system speaks");
    assert_eq!(h.log(), ["system.styled"]);
}

#[test]
fn b3_a_not_ready_tamil_route_is_not_chosen() {
    let (router, h) = build(Setup::new(false));
    h.ready.store(false, Ordering::SeqCst);
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("system speaks");
    assert_eq!(h.log(), ["system.styled"]);
}

#[test]
fn b4_a_failed_tamil_health_is_not_chosen() {
    let (router, h) = build(Setup::new(false));
    let revision = h.tamil_health.revision();
    assert!(h
        .tamil_health
        .report_failure(revision, FailureReason::Synth));
    assert!(matches!(h.tamil_health.state(), HealthState::Failed(_)));
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("system speaks");
    assert_eq!(h.log(), ["system.styled"]);
}

#[test]
fn b5_a_tamil_refusal_falls_back_once_to_the_system() {
    let (router, h) = build(Setup::new(true));
    *h.tamil.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    router
        .spawn_styled(TAMIL_ONE, Some("Samantha"), Some(200), Emotion::Calm)
        .expect("fallback speaks");
    assert_eq!(h.log(), ["system.styled"], "one fallback, no tamil call");
    assert_eq!(h.system.calls()[0].voice.as_deref(), Some("Samantha"));
}

#[test]
fn b6_a_tamil_would_block_is_propagated_without_a_fallback() {
    let (router, h) = build(Setup::new(true));
    *h.tamil.fail_next.lock().unwrap() = Some(io::ErrorKind::WouldBlock);
    let err = router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .err()
        .expect("propagated");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert!(h.log().is_empty(), "no fallback when a stop is unconfirmed");
}

// ---- C: queue-capable system, pinning, toggles, intentional skips ------------------------------

#[test]
fn c1_queue_capable_system_tamil_burst_is_pinned_then_english_returns_to_system() {
    let (router, h) = build(Setup::new(true));
    router
        .enqueue_styled(TAMIL_ONE, None, Some(180), Emotion::Neutral)
        .expect("first sentence");
    router
        .enqueue_styled(TAMIL_TWO, None, Some(180), Emotion::Neutral)
        .expect("second sentence");
    assert_eq!(h.tamil.texts(), [TAMIL_ONE, TAMIL_TWO]);
    assert_eq!(h.log(), ["tamil.enqueue", "tamil.enqueue"]);

    h.tamil.set_running(false); // the burst drained
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english goes to the system queue");
    assert_eq!(
        h.log(),
        ["tamil.enqueue", "tamil.enqueue", "system.enqueue"]
    );
    assert_eq!(h.tamil.texts().len(), 2);
}

#[test]
fn c2_disabling_tamil_mid_burst_keeps_the_burst_on_tamil() {
    let (router, h) = build(Setup::new(true));
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("first");
    h.enabled.store(false, Ordering::SeqCst);
    router
        .enqueue_styled(TAMIL_TWO, None, None, Emotion::Neutral)
        .expect("next sentence still tamil");
    assert_eq!(h.tamil.texts(), [TAMIL_ONE, TAMIL_TWO]);

    h.tamil.set_running(false); // the burst ended
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("disabled: system speaks");
    assert_eq!(h.tamil.texts().len(), 2);
    assert_eq!(h.system.texts(), [TAMIL_ONE]);
}

#[test]
fn c3_an_english_only_sentence_in_a_tamil_burst_is_an_intentional_skip() {
    let (router, h) = build(Setup::new(true));
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("first");
    router
        .enqueue_styled(ENGLISH_TWO, None, None, Emotion::Neutral)
        .expect("skip is Ok");
    assert_eq!(router.tamil_skipped_unspeakable(), 1);
    assert_eq!(
        h.tamil.texts(),
        [TAMIL_ONE],
        "tamil engine not called for the skip"
    );
    assert!(
        h.system.calls().is_empty(),
        "system not called for the skip"
    );
    assert!(h.tamil_health.is_ready(), "a skip never touches health");

    router
        .enqueue_styled(TAMIL_TWO, None, None, Emotion::Neutral)
        .expect("tamil sentence spoken once");
    assert_eq!(h.tamil.texts(), [TAMIL_ONE, TAMIL_TWO]);
    assert_eq!(router.tamil_skipped_unspeakable(), 1);
}

#[test]
fn c4_english_running_on_a_queue_system_keeps_a_tamil_enqueue_on_the_system_slot() {
    let (router, h) = build(Setup::new(true));
    let mut handle = router
        .spawn_styled(ENGLISH, Some("Samantha"), None, Emotion::Calm)
        .expect("english");
    assert!(h.system.running());
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("pinned to the system slot");
    assert!(h.tamil.calls().is_empty(), "no concurrent tamil start");
    assert_eq!(h.system.texts(), [ENGLISH, TAMIL_ONE]);
    assert_eq!(h.log(), ["system.styled", "system.enqueue"]);

    assert!(router.stop_all());
    assert!(!h.system.running());
    assert_eq!(h.system.kills(), 1, "the started handle saw the kill");
    assert!(!handle.is_running());
}

// ---- D: kill-before-spawn ----------------------------------------------------------------------

#[test]
fn d1_a_tamil_spawn_is_killed_before_an_english_spawn_starts() {
    let (router, h) = build(Setup::new(true));
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil");
    router
        .spawn(ENGLISH, None, None)
        .expect("english after the kill");
    assert_eq!(h.log(), ["tamil.styled", "tamil.kill", "system.spawn"]);
}

#[test]
fn d2_an_unconfirmed_tamil_kill_blocks_the_english_spawn() {
    let (router, h) = build(Setup::new(false));
    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil");
    h.tamil.kill_ok.store(false, Ordering::SeqCst);
    let err = router
        .spawn(ENGLISH, None, None)
        .err()
        .expect("unconfirmed stop refuses the new route");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(h.log(), ["tamil.styled", "tamil.kill"]);
    assert!(h.system.calls().is_empty(), "system spawn never called");

    h.tamil.kill_ok.store(true, Ordering::SeqCst);
    router
        .spawn(ENGLISH, None, None)
        .expect("once confirmed, not wedged");
    assert_eq!(
        h.log(),
        ["tamil.styled", "tamil.kill", "tamil.kill", "system.spawn"]
    );
}

#[test]
fn d3_an_unconfirmed_english_kill_blocks_a_tamil_spawn() {
    let (router, h) = build(Setup::new(false));
    router.spawn(ENGLISH, None, None).expect("english");
    h.system.kill_ok.store(false, Ordering::SeqCst);
    let err = router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .err()
        .expect("unconfirmed stop refuses the tamil route");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert_eq!(h.log(), ["system.spawn", "system.kill"]);
    assert!(h.tamil.calls().is_empty(), "tamil never called");
}

// ---- E: tracking off means passthrough -----------------------------------------------------------

#[test]
fn e1_with_tracking_off_the_system_returns_its_own_handle_as_today() {
    let (router, h) = build(Setup::new(false).without_tamil());
    let handle = router.spawn(ENGLISH, None, None).expect("system");
    assert!(
        !handle.queue_capable(),
        "the system's own handle, not a RoutedHandle"
    );
    assert_eq!(h.log(), ["system.spawn"]);

    let (router, h) = build(Setup::new(false));
    h.enabled.store(false, Ordering::SeqCst);
    let handle = router
        .spawn_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("system");
    assert!(
        !handle.queue_capable(),
        "disabled and idle: the system's own handle"
    );
    assert_eq!(h.log(), ["system.styled"]);
}

// ---- F: handles reach the Tamil slot -----------------------------------------------------------

#[test]
fn f1_kill_stop_all_and_is_running_reach_the_tamil_slot() {
    let (router, h) = build(Setup::new(false));
    let mut handle = router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil");
    assert!(handle.is_running());
    h.tamil.set_running(false); // the tamil audio finished
    assert!(
        !handle.is_running(),
        "reports running until the fake finishes"
    );

    router
        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("a new burst");
    assert!(handle.is_running());
    let before = h.tamil.kills();
    assert!(handle.kill());
    assert!(h.tamil.kills() > before, "kill reaches the tamil handle");
    assert!(!h.tamil.running());

    router
        .spawn_styled(TAMIL_TWO, None, None, Emotion::Neutral)
        .expect("tamil again");
    let before = h.tamil.kills();
    assert!(router.stop_all());
    assert!(
        h.tamil.kills() > before,
        "stop_all reaches the tamil handle"
    );
    assert!(!h.tamil.running());
}

// ---- G: through the real TtsManager ------------------------------------------------------------

#[test]
fn g1_queue_capable_manager_sends_tamil_sentences_to_tamil_in_order() {
    let (router, h) = build(Setup::new(true));
    let mgr = TtsManager::new(Box::new(router));
    let epoch = mgr.begin_utterance().unwrap();
    mgr.speak_queued_if_current(epoch, TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("first sentence");
    mgr.speak_queued_if_current(epoch, TAMIL_TWO, None, None, Emotion::Neutral)
        .expect("second sentence");
    assert_eq!(h.log(), ["tamil.enqueue", "tamil.enqueue"]);
    assert_eq!(h.tamil.texts(), [TAMIL_ONE, TAMIL_TWO]);
    assert!(mgr.is_speaking());
    assert!(mgr.stop());
    assert!(!h.tamil.running());
}

#[test]
fn g2_non_queueing_manager_speaks_a_whole_tamil_reply_with_one_tamil_spawn() {
    let (router, h) = build(Setup::new(false));
    let mgr = TtsManager::new(Box::new(router));
    assert!(!mgr.queue_capable(), "the system cannot queue");
    let reply = format!("{TAMIL_ONE} {TAMIL_TWO}");
    mgr.speak(&reply, None, None).expect("whole reply");
    assert_eq!(h.log(), ["tamil.styled"], "one spawn for the whole reply");
    assert_eq!(h.tamil.calls().len(), 1);
    assert!(h.system.calls().is_empty());
}

// ---- H: concurrency ----------------------------------------------------------------------------

#[test]
fn h1_concurrent_spawn_enqueue_and_kill_never_overlap_or_deadlock() {
    let (router, h) = build(Setup::new(true));
    let router = Arc::new(router);
    let (done_tx, done_rx) = mpsc::channel();
    for worker in 0..4usize {
        let router = router.clone();
        let done_tx = done_tx.clone();
        std::thread::spawn(move || {
            let mut errors = 0usize;
            for i in 0..200usize {
                let result = match (worker + i) % 4 {
                    0 => router
                        .spawn_styled(TAMIL_ONE, None, None, Emotion::Neutral)
                        .map(|_| ()),
                    1 => router.spawn(ENGLISH, None, None).map(|_| ()),
                    2 => {
                        let text = if i % 2 == 0 { TAMIL_TWO } else { ENGLISH };
                        router
                            .enqueue_styled(text, None, None, Emotion::Neutral)
                            .map(|_| ())
                    }
                    _ => {
                        router.stop_all();
                        Ok(())
                    }
                };
                if result.is_err() {
                    errors += 1;
                }
            }
            done_tx.send(errors).unwrap();
        });
    }
    drop(done_tx);
    for _ in 0..4 {
        let errors = done_rx
            .recv_timeout(Duration::from_secs(60))
            .expect("worker finished (no deadlock)");
        assert_eq!(errors, 0, "no unexpected refusals");
    }
    assert_eq!(
        h.tracker.violations.load(Ordering::SeqCst),
        0,
        "two routes never ran at once"
    );
    assert!(router.stop_all());
}
