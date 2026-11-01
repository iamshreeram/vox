//! Spec for the Tamil handover in `natural_router` (DESIGN.md Revision 2,
//! `.ram-status/tamil-tts-handover-20261101c/DESIGN.md`, tests H1..H12). Written FIRST.
//!
//! Expected today (before the production fix): H1, H2, H5, H7 (handover half), H8, H10 (handover
//! half), H11 and H12 (handover/passthrough/drop/fallback records) FAIL. H3, H4, H6, H7 (negative
//! half), H9 (drained half), the H10 fallback guard and H12 drop/fallback guards are regression
//! guards that already PASS. SYSTEM pins are unchanged (see `c4` in natural_router_tamil_tests.rs).
//!
//! The fakes are copied from `natural_router_tamil_tests.rs` (private there, never edited).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead.
use crate::managers::emotion::Emotion;
use crate::managers::natural_chunk::is_mostly_tamil;
use crate::managers::natural_engine::{FailureReason, NaturalHealth, DEFAULT_AUTO_RETRY_COOLDOWN};
use crate::managers::natural_router::*;
use crate::managers::natural_sink::{PlaybackCtl, StreamFactory};
use crate::managers::natural_tamil::{build_tamil_route, TAMIL_MODEL_DIRNAME, TAMIL_SAMPLE_RATE};
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

const TAMIL_ONE: &str = "இன்று நல்ல நாள். நாம் வெளியே போகலாம்.";
const TAMIL_TWO: &str = "மழை பெய்யத் தொடங்கியது.";
const ENGLISH: &str = "Hello there, how are you today?";
const ENGLISH_TWO: &str = "Only English words are in this sentence.";
const SPANISH: &str = "Hola, ¿cómo estás hoy? Muy bien, gracias.";
/// Mostly Tamil with exactly one English word.
const TAMIL_ONE_ENGLISH_WORD: &str = "இன்று நல்ல நாள் மிகவும் அருமையாக இருக்கிறது ok";
/// Mostly English with exactly one Tamil word.
const MOSTLY_ENGLISH_ONE_TAMIL: &str = "வணக்கம் hello there my dear friend how are you doing today";
/// Distinctive sentinel: Tamil letters plus a Latin marker. Must never reach a log record.
const SENTINEL_TAMIL: &str = "இன்று நல்ல நாள் ZQXJsentinel மிகவும் அருமையாக இருக்கிறது.";
const SENTINEL_ENGLISH: &str = "Hello ZQXJsentinel there, how are you today?";

const ROUTER_TARGET: &str = "vox_app_lib::managers::natural_router";
const FIELD_KEYS: [&str; 8] = [
    "via",
    "pinned",
    "chosen",
    "chars",
    "mostly_tamil",
    "tamil_ready",
    "handover",
    "fell_back",
];

/// Ordered call log shared by every route of one rig, e.g. "phonon.enqueue", "phonon.kill".
type Log = Arc<Mutex<Vec<String>>>;

struct Route {
    name: &'static str,
    log: Log,
    queue: bool,
    running: AtomicBool,
    kill_ok: AtomicBool,
    kills: AtomicUsize,
    calls: Mutex<Vec<String>>,
    fail_next: Mutex<Option<io::ErrorKind>>,
}

impl Route {
    fn new(name: &'static str, log: &Log, queue: bool) -> Arc<Self> {
        Arc::new(Self {
            name,
            log: log.clone(),
            queue,
            running: AtomicBool::new(false),
            kill_ok: AtomicBool::new(true),
            kills: AtomicUsize::new(0),
            calls: Mutex::new(Vec::new()),
            fail_next: Mutex::new(None),
        })
    }
    fn texts(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    fn set_running(&self, on: bool) {
        self.running.store(on, Ordering::SeqCst);
    }
    fn kills(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }
    fn entry(&self, what: &str) {
        self.log
            .lock()
            .unwrap()
            .push(format!("{}.{}", self.name, what));
    }
    /// Starts one burst on this route: logs the call and leaves the route running.
    fn begin(
        self: &Arc<Self>,
        kind: &'static str,
        text: &str,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if let Some(kind) = self.fail_next.lock().unwrap().take() {
            return Err(io::Error::from(kind));
        }
        self.entry(kind);
        self.calls.lock().unwrap().push(text.into());
        self.set_running(true);
        Ok(Box::new(FakeHandle(self.clone())))
    }
}

struct Fake(Arc<Route>);

impl SpeechBackend for Fake {
    fn spawn(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.begin("spawn", text)
    }
    fn spawn_styled(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.begin("styled", text)
    }
    fn supports_queue(&self) -> bool {
        self.0.queue
    }
    fn enqueue_styled(
        &self,
        text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if !self.0.queue {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        self.0.begin("enqueue", text)
    }
}

struct FakeHandle(Arc<Route>);

impl SpeechHandle for FakeHandle {
    fn kill(&mut self) -> bool {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        self.0.entry("kill");
        if self.0.kill_ok.load(Ordering::SeqCst) {
            self.0.set_running(false);
            true
        } else {
            false
        }
    }
    fn is_running(&mut self) -> bool {
        self.0.running()
    }
}

/// The routes of one rig plus the knobs a test flips.
struct Rig {
    log: Log,
    system: Arc<Route>,
    natural: Arc<Route>,
    phonon: Arc<Route>,
    tamil: Arc<Route>,
    tamil_health: Arc<NaturalHealth>,
    enabled: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
}

impl Rig {
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

/// Builds the router with today's production API only. `system_queue` = the system route can
/// queue; `natural` / `phonon` = the peer is selected and built; `tamil` = the Tamil route is
/// attached (enabled and ready at first; a test flips the knobs).
fn rig(system_queue: bool, natural: bool, phonon: bool, tamil: bool) -> (EngineBackend, Rig) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let system = Route::new("system", &log, system_queue);
    let fallback = Route::new("system_queue", &log, true);
    let natural_route = Route::new("natural", &log, true);
    let phonon_route = Route::new("phonon", &log, true);
    let tamil_route = Route::new("tamil", &log, true);
    let tamil_health = NaturalHealth::new(Duration::from_secs(300));
    let enabled = Arc::new(AtomicBool::new(true));
    let ready = Arc::new(AtomicBool::new(true));
    let choice = RouterChoice {
        natural,
        voice: "bf_emma".into(),
        phonon,
    };
    let config = RouterConfig {
        choice: Arc::new(move || choice.clone()),
        model_ready: Arc::new(|| true),
        phonon_ready: Arc::new(|| true),
    };
    let mut router = EngineBackend::with_phonon(
        Box::new(Fake(system.clone())),
        Box::new(Fake(fallback)),
        natural.then(|| Box::new(Fake(natural_route.clone())) as Box<dyn SpeechBackend>),
        NaturalHealth::new(Duration::from_secs(300)),
        phonon.then(|| Box::new(Fake(phonon_route.clone())) as Box<dyn SpeechBackend>),
        NaturalHealth::new(Duration::from_secs(300)),
        config,
    );
    if tamil {
        let (on, is_ready) = (enabled.clone(), ready.clone());
        router = router.with_tamil_route(TamilRoute {
            backend: Box::new(Fake(tamil_route.clone())),
            health: tamil_health.clone(),
            enabled: Arc::new(move || on.load(Ordering::SeqCst)),
            ready: Arc::new(move || is_ready.load(Ordering::SeqCst)),
        });
    }
    let handles = Rig {
        log,
        system,
        natural: natural_route,
        phonon: phonon_route,
        tamil: tamil_route,
        tamil_health,
        enabled,
        ready,
    };
    (router, handles)
}

// ---- H1: a Tamil sentence hands over from a running Phonon ack -----------------------------------

#[test]
fn h1_running_phonon_ack_is_stopped_and_tamil_speaks() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack starts on phonon");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil sentence hands over");
    assert_eq!(
        r.log(),
        ["phonon.enqueue", "phonon.kill", "tamil.enqueue"],
        "handover: the running phonon ack is stopped, then tamil starts"
    );
    assert_eq!(r.phonon.texts(), [ENGLISH], "phonon never gets tamil text");
    assert_eq!(r.tamil.texts(), [TAMIL_ONE]);
}

// ---- H2: NATURAL pinned is not dropped; Tamil gets the sentence ------------------------------------

#[test]
fn h2_running_natural_ack_is_stopped_and_tamil_is_not_dropped() {
    let (router, r) = rig(true, true, false, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack starts on natural");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil sentence hands over");
    assert_eq!(
        router.dropped_non_latin(),
        0,
        "a mostly-Tamil sentence is handed over, not dropped"
    );
    assert_eq!(
        r.log(),
        ["natural.enqueue", "natural.kill", "tamil.enqueue"]
    );
    assert_eq!(r.tamil.texts(), [TAMIL_ONE]);
}

// ---- H3: SYSTEM pins are unchanged (no handover) -------------------------------------------------

#[test]
fn h3_queue_system_pin_keeps_tamil_on_the_system_slot() {
    let (router, r) = rig(true, false, false, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english starts on the system queue");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("pinned to the system slot");
    assert!(r.tamil.texts().is_empty(), "no tamil call for a system pin");
    assert_eq!(r.log(), ["system.enqueue", "system.enqueue"]);
}

#[test]
fn h3_plain_system_pin_refuses_enqueue_as_today_and_never_calls_tamil() {
    let (router, r) = rig(false, false, false, true);
    router
        .spawn(ENGLISH, None, None)
        .expect("english spawn on the plain system");
    let err = router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .err()
        .expect("the plain system refuses the enqueue as it does today");
    assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    assert!(r.tamil.texts().is_empty(), "no tamil call for a system pin");
    assert_eq!(r.log(), ["system.spawn"]);
}

// ---- H4: no Tamil route means no handover (regression guards) ---------------------------------

fn stays_on_phonon(tamil_attached: bool, break_route: impl FnOnce(&Rig)) {
    let (router, r) = rig(true, false, true, tamil_attached);
    break_route(&r);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack on phonon");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil text stays on phonon as before");
    assert!(r.tamil.texts().is_empty(), "zero tamil calls");
    assert_eq!(r.log(), ["phonon.enqueue", "phonon.enqueue"]);
    assert_eq!(r.phonon.texts(), [ENGLISH, TAMIL_ONE]);
}

#[test]
fn h4_absent_tamil_route_keeps_the_phonon_burst() {
    stays_on_phonon(false, |_| {});
}

#[test]
fn h4_disabled_tamil_route_keeps_the_phonon_burst() {
    stays_on_phonon(true, |r| r.enabled.store(false, Ordering::SeqCst));
}

#[test]
fn h4_not_ready_tamil_route_keeps_the_phonon_burst() {
    stays_on_phonon(true, |r| r.ready.store(false, Ordering::SeqCst));
}

#[test]
fn h4_failed_tamil_health_keeps_the_phonon_burst() {
    stays_on_phonon(true, |r| {
        let revision = r.tamil_health.revision();
        assert!(r
            .tamil_health
            .report_failure(revision, FailureReason::Synth));
    });
}

#[test]
fn h4_natural_burst_without_tamil_still_drops_non_latin_and_counts_it() {
    let (router, r) = rig(true, true, false, false);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack on natural");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("dropped sentences return Ok");
    assert_eq!(
        router.dropped_non_latin(),
        1,
        "dropped and counted as today"
    );
    assert_eq!(r.log(), ["natural.enqueue"]);
}

// ---- H5: an unconfirmed Phonon stop refuses the handover -------------------------------------------

#[test]
fn h5_unconfirmed_phonon_kill_refuses_the_handover_without_calling_tamil() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack on phonon");
    r.phonon.kill_ok.store(false, Ordering::SeqCst);
    let err = router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .err()
        .expect("an unconfirmed stop refuses the handover");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    assert!(r.tamil.texts().is_empty(), "zero tamil calls");
    assert!(
        r.phonon.running(),
        "the unconfirmed phonon slot is untouched"
    );
}

// ---- H6: non-Tamil sentences never hand over (regression guards) -------------------------------

#[test]
fn h6_spanish_on_a_running_phonon_burst_stays_on_phonon() {
    assert!(!is_mostly_tamil(SPANISH), "precondition: not mostly tamil");
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(SPANISH, None, None, Emotion::Neutral)
        .expect("spanish stays on phonon");
    assert_eq!(r.phonon.texts(), [ENGLISH, SPANISH]);
    assert!(r.tamil.texts().is_empty());
    assert_eq!(r.log(), ["phonon.enqueue", "phonon.enqueue"]);
}

#[test]
fn h6_english_on_a_running_natural_burst_stays_on_natural() {
    let (router, r) = rig(true, true, false, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english on natural");
    router
        .enqueue_styled(ENGLISH_TWO, None, None, Emotion::Neutral)
        .expect("english stays on natural");
    assert_eq!(r.natural.texts(), [ENGLISH, ENGLISH_TWO]);
    assert!(r.tamil.texts().is_empty());
    assert_eq!(r.log(), ["natural.enqueue", "natural.enqueue"]);
}

#[test]
fn h6_english_on_a_running_tamil_burst_keeps_the_skip() {
    let (router, r) = rig(true, false, false, true);
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil burst");
    router
        .enqueue_styled(ENGLISH_TWO, None, None, Emotion::Neutral)
        .expect("english inside a tamil burst is an intentional skip");
    assert_eq!(router.tamil_skipped_unspeakable(), 1);
    assert_eq!(r.tamil.texts(), [TAMIL_ONE]);
    assert!(r.system.texts().is_empty());
}

// ---- H7: the language threshold decides the handover ---------------------------------------------

#[test]
fn h7_mostly_tamil_with_one_english_word_hands_over() {
    assert!(
        is_mostly_tamil(TAMIL_ONE_ENGLISH_WORD),
        "threshold drift: this sentence must be mostly tamil"
    );
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(TAMIL_ONE_ENGLISH_WORD, None, None, Emotion::Neutral)
        .expect("hands over");
    assert_eq!(r.log(), ["phonon.enqueue", "phonon.kill", "tamil.enqueue"]);
    assert_eq!(r.tamil.texts(), [TAMIL_ONE_ENGLISH_WORD]);
}

#[test]
fn h7_mostly_english_with_one_tamil_word_does_not_hand_over() {
    assert!(
        !is_mostly_tamil(MOSTLY_ENGLISH_ONE_TAMIL),
        "threshold drift: this sentence must be mostly english"
    );
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(MOSTLY_ENGLISH_ONE_TAMIL, None, None, Emotion::Neutral)
        .expect("stays on phonon");
    assert!(r.tamil.texts().is_empty());
    assert_eq!(r.phonon.texts(), [ENGLISH, MOSTLY_ENGLISH_ONE_TAMIL]);
}

// ---- H8: a second Tamil sentence follows the Tamil pin; one kill only ------------------------------

#[test]
fn h8_second_tamil_sentence_follows_the_tamil_pin_with_one_kill() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("first tamil sentence hands over");
    router
        .enqueue_styled(TAMIL_TWO, None, None, Emotion::Neutral)
        .expect("second tamil sentence follows the tamil pin");
    assert_eq!(
        r.log(),
        [
            "phonon.enqueue",
            "phonon.kill",
            "tamil.enqueue",
            "tamil.enqueue"
        ]
    );
    assert_eq!(r.phonon.kills(), 1, "exactly one kill of the english slot");
    assert_eq!(r.tamil.texts(), [TAMIL_ONE, TAMIL_TWO]);
}

// ---- H9: a drained ack is routed afresh; a running ack is handed over once ----------------------

#[test]
fn h9_drained_ack_routes_tamil_to_tamil_with_no_kill() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    r.phonon.set_running(false); // the ack has already drained
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("tamil routed afresh");
    assert_eq!(
        r.log(),
        ["phonon.enqueue", "tamil.enqueue"],
        "no kill event"
    );
    assert_eq!(r.phonon.kills(), 0);
    assert_eq!(r.tamil.texts(), [TAMIL_ONE]);
}

#[test]
fn h9_running_ack_is_stopped_exactly_once_for_the_handover() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("handover");
    assert_eq!(r.phonon.kills(), 1, "handover stops the running ack once");
    assert_eq!(r.tamil.texts(), [TAMIL_ONE]);
}

// ---- H10: a Tamil failure after handover falls back like today --------------------------------

#[test]
fn h10_tamil_failure_after_handover_falls_back_to_the_system_queue() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    *r.tamil.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("the fallback speaks, as for any tamil error");
    assert_eq!(
        r.log(),
        ["phonon.enqueue", "phonon.kill", "system.enqueue"],
        "the same fallback slot as today's tamil-error fallback"
    );
    assert!(r.tamil.texts().is_empty());
    assert_eq!(r.system.texts(), [TAMIL_ONE]);
}

#[test]
fn h10_tamil_failure_without_a_burst_falls_back_to_the_system_as_today() {
    let (router, r) = rig(true, false, false, true);
    *r.tamil.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("the fallback speaks");
    assert_eq!(r.log(), ["system.enqueue"]);
}

// ---- H11: cancellation after handover stops Tamil and nothing starts afterwards ------------------

#[test]
fn h11_kill_after_handover_stops_tamil_and_nothing_starts_afterwards() {
    let (router, r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    let mut handle = router
        .enqueue_styled(TAMIL_ONE, None, None, Emotion::Neutral)
        .expect("handover");
    assert_eq!(r.log(), ["phonon.enqueue", "phonon.kill", "tamil.enqueue"]);
    let mut expected = r.log();
    expected.push("tamil.kill".into());
    assert!(
        handle.kill(),
        "kill through the returned handle is confirmed"
    );
    assert!(!r.tamil.running(), "tamil is stopped");
    assert!(!handle.is_running());
    assert_eq!(
        r.log(),
        expected,
        "the kill adds one tamil.kill and nothing else"
    );
}

// ---- H12: one structured decision record per `start`, no sentinel in any record -----------------

static RECORDS: Mutex<Vec<(log::Level, String, String)>> = Mutex::new(Vec::new());
static SERIAL: Mutex<()> = Mutex::new(());
static INSTALL: Once = Once::new();
static OURS: AtomicBool = AtomicBool::new(false);

struct CaptureLog;

impl log::Log for CaptureLog {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Ok(mut records) = RECORDS.lock() {
            records.push((
                record.level(),
                record.target().to_string(),
                record.args().to_string(),
            ));
        }
    }
    fn flush(&self) {}
}

static CAPTURE: CaptureLog = CaptureLog;

/// Serialises the logging tests and installs the capturing logger once per process.
fn capture_begin() -> std::sync::MutexGuard<'static, ()> {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    INSTALL.call_once(|| {
        if log::set_logger(&CAPTURE).is_ok() {
            OURS.store(true, Ordering::SeqCst);
        }
        log::set_max_level(log::LevelFilter::Trace);
    });
    assert!(
        OURS.load(Ordering::SeqCst),
        "another test installed the process-wide logger first; run natural_router_handover_tests alone"
    );
    RECORDS.lock().unwrap_or_else(|e| e.into_inner()).clear();
    guard
}

fn router_records(level: log::Level) -> Vec<String> {
    RECORDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(l, target, _)| *l == level && target == ROUTER_TARGET)
        .map(|(_, _, message)| message.clone())
        .collect()
}

fn speech_route_infos() -> Vec<String> {
    router_records(log::Level::Info)
        .into_iter()
        .filter(|m| m.starts_with("speech route:"))
        .collect()
}

/// Splits `speech route: k=v k=v ...` into ordered (key, value) pairs.
fn fields(line: &str) -> Vec<(String, String)> {
    let body = line
        .strip_prefix("speech route: ")
        .expect("record starts with `speech route: `");
    body.split(' ')
        .map(|pair| {
            let (k, v) = pair.split_once('=').expect("every field is key=value");
            (k.to_string(), v.to_string())
        })
        .collect()
}

fn keys(fields: &[(String, String)]) -> Vec<String> {
    fields.iter().map(|(k, _)| k.clone()).collect()
}

fn value<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("<missing>")
}

/// No record of ANY level or target may carry the Tamil sentinel: no Tamil letters, no marker.
fn assert_no_sentinel() {
    let records = RECORDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (level, target, message) in records {
        assert!(
            !message.contains("ZQXJsentinel"),
            "sentinel leaked into {level} {target}: {message}"
        );
        assert!(
            !message
                .chars()
                .any(|c| ('\u{0B80}'..='\u{0BFF}').contains(&c)),
            "tamil letters leaked into {level} {target}: {message}"
        );
    }
}

#[test]
#[ignore = "installs the process-global logger; run alone: cargo test --lib natural_router_handover_tests::h12 -- --ignored --test-threads=1"]
fn h12_handover_writes_one_decision_record_per_start() {
    let _serial = capture_begin();
    let (router, _r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(SENTINEL_TAMIL, None, None, Emotion::Neutral)
        .expect("handover");
    assert!(
        is_mostly_tamil(SENTINEL_TAMIL),
        "precondition: mostly tamil"
    );
    let infos = speech_route_infos();
    assert_eq!(infos.len(), 2, "one info record per start call: {infos:?}");
    let f = fields(&infos[1]);
    assert_eq!(
        keys(&f),
        FIELD_KEYS,
        "exactly the allow-listed keys, in order"
    );
    assert_eq!(value(&f, "via"), "Enqueue");
    assert_eq!(value(&f, "pinned"), "phonon");
    assert_eq!(value(&f, "chosen"), "tamil");
    assert_eq!(
        value(&f, "chars"),
        SENTINEL_TAMIL.chars().count().to_string()
    );
    assert_eq!(value(&f, "mostly_tamil"), "true");
    assert_eq!(value(&f, "tamil_ready"), "true");
    assert_eq!(value(&f, "handover"), "true");
    assert_eq!(value(&f, "fell_back"), "false");
    assert!(
        router_records(log::Level::Warn).is_empty(),
        "no warning on a handover"
    );
    assert_no_sentinel();
}

#[test]
#[ignore = "installs the process-global logger; run alone: cargo test --lib natural_router_handover_tests::h12 -- --ignored --test-threads=1"]
fn h12_passthrough_writes_one_decision_record_with_no_handover() {
    let _serial = capture_begin();
    let (router, _r) = rig(true, false, true, true);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack");
    router
        .enqueue_styled(SENTINEL_ENGLISH, None, None, Emotion::Neutral)
        .expect("english stays on phonon");
    let infos = speech_route_infos();
    assert_eq!(infos.len(), 2, "one info record per start call: {infos:?}");
    let f = fields(&infos[1]);
    assert_eq!(keys(&f), FIELD_KEYS);
    assert_eq!(value(&f, "pinned"), "phonon");
    assert_eq!(value(&f, "handover"), "false");
    assert_eq!(value(&f, "fell_back"), "false");
    assert_no_sentinel();
}

#[test]
#[ignore = "installs the process-global logger; run alone: cargo test --lib natural_router_handover_tests::h12 -- --ignored --test-threads=1"]
fn h12_drop_writes_a_decision_record_and_a_separate_warning() {
    let _serial = capture_begin();
    let (router, _r) = rig(true, true, false, false);
    router
        .enqueue_styled(ENGLISH, None, None, Emotion::Neutral)
        .expect("english ack on natural");
    router
        .enqueue_styled(SENTINEL_TAMIL, None, None, Emotion::Neutral)
        .expect("dropped and counted");
    let infos = speech_route_infos();
    assert_eq!(infos.len(), 2, "one info record per start call: {infos:?}");
    let f = fields(&infos[1]);
    assert_eq!(keys(&f), FIELD_KEYS);
    assert_eq!(value(&f, "pinned"), "natural");
    assert_eq!(value(&f, "handover"), "false");
    let warnings = router_records(log::Level::Warn);
    assert_eq!(
        warnings.len(),
        1,
        "the drop is a separate warning: {warnings:?}"
    );
    assert!(!warnings[0].starts_with("speech route:"));
    assert_no_sentinel();
}

#[test]
#[ignore = "installs the process-global logger; run alone: cargo test --lib natural_router_handover_tests::h12 -- --ignored --test-threads=1"]
fn h12_fallback_writes_a_decision_record_with_fell_back_and_a_warning() {
    let _serial = capture_begin();
    let (router, r) = rig(true, false, false, true);
    *r.tamil.fail_next.lock().unwrap() = Some(io::ErrorKind::Other);
    router
        .enqueue_styled(SENTINEL_TAMIL, None, None, Emotion::Neutral)
        .expect("the fallback speaks");
    let infos = speech_route_infos();
    assert_eq!(infos.len(), 1, "one info record per start call: {infos:?}");
    let f = fields(&infos[0]);
    assert_eq!(keys(&f), FIELD_KEYS);
    assert_eq!(value(&f, "pinned"), "none");
    assert_eq!(value(&f, "fell_back"), "true");
    let warnings = router_records(log::Level::Warn);
    assert_eq!(
        warnings.len(),
        1,
        "the fallback is a separate warning: {warnings:?}"
    );
    assert_no_sentinel();
}

// ---- speech_route_record: fixed key order and metadata only (always runs) ----------------------

#[test]
fn speech_route_record_has_fixed_keys_and_no_free_text() {
    // Slot indices: 0 = NATURAL, 1 = PHONON, 2 = SYSTEM, 3 = SYSTEM_QUEUE, 4 = TAMIL.
    // The signature takes no text argument, so only these metadata values can appear.
    let handover = speech_route_record(Via::Enqueue, Some(1), 4, 23, true, true, true, false);
    assert_eq!(
        handover,
        "speech route: via=Enqueue pinned=phonon chosen=tamil chars=23 mostly_tamil=true tamil_ready=true handover=true fell_back=false"
    );
    let passthrough = speech_route_record(Via::Enqueue, Some(1), 1, 30, false, true, false, false);
    assert_eq!(
        passthrough,
        "speech route: via=Enqueue pinned=phonon chosen=phonon chars=30 mostly_tamil=false tamil_ready=true handover=false fell_back=false"
    );
    let dropped = speech_route_record(Via::Enqueue, Some(0), 0, 12, false, false, false, false);
    assert_eq!(
        dropped,
        "speech route: via=Enqueue pinned=natural chosen=natural chars=12 mostly_tamil=false tamil_ready=false handover=false fell_back=false"
    );
    for line in [&handover, &passthrough, &dropped] {
        assert_eq!(keys(&fields(line)), FIELD_KEYS, "fixed key order: {line}");
    }
}

// ---- Live (ignored): a real Tamil engine behind the real router, after an English ack -----------

const LIVE_ENGLISH: &str = "The weather is lovely today.";
const LIVE_TAMIL: &str = "இன்று வானிலை மிகவும் அழகாக இருக்கிறது.";
const LIVE_MIN_AUDIO_SECS: f64 = 0.8;
const LIVE_DEADLINE: Duration = Duration::from_secs(30);

/// Sink that stands in for the audio device; keeps every rendered frame.
struct LiveCapture {
    captured: Arc<Mutex<Vec<f32>>>,
}

/// Stops the capture thread when the stream is dropped.
struct LiveStop(Arc<AtomicBool>);

impl Drop for LiveStop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl StreamFactory for LiveCapture {
    fn device_rate(&self) -> Result<u32, String> {
        Ok(TAMIL_SAMPLE_RATE)
    }
    fn open(&self, ctl: Arc<PlaybackCtl>) -> Result<Box<dyn Send>, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, sink) = (stop.clone(), self.captured.clone());
        std::thread::spawn(move || {
            let mut frame = vec![0.0_f32; 240];
            while !flag.load(Ordering::SeqCst) {
                ctl.render(&mut frame, std::time::Instant::now());
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .extend_from_slice(&frame);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(Box::new(LiveStop(stop)))
    }
}

/// A fake English PHONON burst: stays running until killed and makes no sound.
struct LiveEnglishBurst {
    running: Arc<AtomicBool>,
    kills: Arc<AtomicUsize>,
}

struct LiveEnglishHandle(LiveEnglishBurst);

impl SpeechHandle for LiveEnglishHandle {
    fn kill(&mut self) -> bool {
        self.0.kills.fetch_add(1, Ordering::SeqCst);
        self.0.running.store(false, Ordering::SeqCst);
        true
    }
    fn is_running(&mut self) -> bool {
        self.0.running.load(Ordering::SeqCst)
    }
}

impl SpeechBackend for LiveEnglishBurst {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
    fn supports_queue(&self) -> bool {
        true
    }
    fn enqueue_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.running.store(true, Ordering::SeqCst);
        Ok(Box::new(LiveEnglishHandle(LiveEnglishBurst {
            running: self.running.clone(),
            kills: self.kills.clone(),
        })))
    }
}

/// Silent system voice that counts calls; must stay at zero.
struct LiveSilentSystem(Arc<AtomicUsize>);

struct LiveSilentHandle;

impl SpeechHandle for LiveSilentHandle {
    fn kill(&mut self) -> bool {
        true
    }
    fn is_running(&mut self) -> bool {
        false
    }
}

impl SpeechBackend for LiveSilentSystem {
    fn spawn(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(LiveSilentHandle))
    }
    fn supports_queue(&self) -> bool {
        true
    }
    fn enqueue_styled(
        &self,
        _text: &str,
        _voice: Option<&str>,
        _rate: Option<u32>,
        _emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(LiveSilentHandle))
    }
}

/// Phonon selected and ready; no Natural; Tamil route is the real stack.
fn live_config() -> RouterConfig {
    RouterConfig {
        choice: Arc::new(|| RouterChoice {
            natural: false,
            voice: String::new(),
            phonon: true,
        }),
        model_ready: Arc::new(|| false),
        phonon_ready: Arc::new(|| true),
    }
}

#[test]
#[ignore = "needs the Tamil pack; set VOX_TAMIL_DIR to the pocket-tts-tamil-int4 folder"]
fn natural_router_live_tamil_after_english_ack_is_audible() -> Result<(), String> {
    let dir = std::env::var_os("VOX_TAMIL_DIR")
        .map(std::path::PathBuf::from)
        .ok_or("VOX_TAMIL_DIR is not set (point it at the pocket-tts-tamil-int4 folder)")?;
    if dir.file_name().and_then(|name| name.to_str()) != Some(TAMIL_MODEL_DIRNAME) {
        return Err(format!(
            "VOX_TAMIL_DIR must be the {TAMIL_MODEL_DIRNAME} folder itself, got {}",
            dir.display()
        ));
    }
    let root = dir
        .parent()
        .ok_or("VOX_TAMIL_DIR has no parent folder")?
        .to_path_buf();
    let audio_needed = (LIVE_MIN_AUDIO_SECS * f64::from(TAMIL_SAMPLE_RATE)) as usize;

    let captured = Arc::new(Mutex::new(Vec::new()));
    let factory = Arc::new(LiveCapture {
        captured: captured.clone(),
    });
    let route = build_tamil_route(&root, factory, Arc::new(|| true));

    let phonon_kills = Arc::new(AtomicUsize::new(0));
    let system_calls = Arc::new(AtomicUsize::new(0));
    let phonon = LiveEnglishBurst {
        running: Arc::new(AtomicBool::new(false)),
        kills: phonon_kills.clone(),
    };
    let router = EngineBackend::with_phonon(
        Box::new(LiveSilentSystem(system_calls.clone())),
        Box::new(LiveSilentSystem(system_calls.clone())),
        None,
        NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN),
        Some(Box::new(phonon)),
        NaturalHealth::new(DEFAULT_AUTO_RETRY_COOLDOWN),
        live_config(),
    )
    .with_tamil_route(route);

    // The English ack starts the Phonon burst and is left running.
    let _ack = router
        .enqueue_styled(LIVE_ENGLISH, None, None, Emotion::Neutral)
        .map_err(|e| format!("the English ack was refused: {e}"))?;
    // The Tamil sentence arrives while the ack runs: it must hand over to the Tamil engine.
    let mut tamil = router
        .enqueue_styled(LIVE_TAMIL, None, None, Emotion::Neutral)
        .map_err(|e| format!("the Tamil sentence was refused: {e}"))?;

    let deadline = std::time::Instant::now() + LIVE_DEADLINE;
    let nonzero = |c: &Mutex<Vec<f32>>| {
        c.lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| **s != 0.0)
            .count()
    };
    while std::time::Instant::now() < deadline {
        if nonzero(&captured) >= audio_needed && !tamil.is_running() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let kills = phonon_kills.load(Ordering::SeqCst);
    let system = system_calls.load(Ordering::SeqCst);
    let voiced: Vec<f32> = captured
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .copied()
        .filter(|s| *s != 0.0)
        .collect();
    let duration = voiced.len() as f64 / f64::from(TAMIL_SAMPLE_RATE);
    let rms = (voiced.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
        / voiced.len().max(1) as f64)
        .sqrt();
    println!(
        "handover phonon_kills={kills} system_calls={system} tamil_audio_s={duration:.2} tamil_rms={rms:.4}"
    );
    if kills != 1 {
        return Err(format!(
            "the handover must stop the English ack once, got {kills} kills"
        ));
    }
    if system != 0 {
        return Err(format!(
            "the Tamil sentence reached the system voice ({system} calls)"
        ));
    }
    if duration <= 1.0 {
        return Err(format!(
            "Tamil audio too short: {duration:.2} s (need > 1 s)"
        ));
    }
    if rms <= 0.01 {
        return Err(format!("Tamil audio too quiet: rms {rms:.4} (need > 0.01)"));
    }
    Ok(())
}
