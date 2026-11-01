//! Tests for `lid_slot` (written first; DESIGN.md Revision 3).
//!
//! Modelled on `indic_stt_slot_tests.rs`: a hand-moved fake clock, a counting fake loader and
//! fake sessions. No model pack, no files, no network. Every cross-thread wait is a bounded
//! `recv_timeout` (<= 2 s), so a wrong implementation fails instead of hanging.
//!
//! The slot passes samples through unchanged. Capping audio to 30 s is the caller's job
//! (`auto_lang::LID_MAX_SECS`), so the sample test checks the length the session receives.

use super::auto_lang::{LidOutcome, LidResult};
use super::indic_stt_slot::{Clock, UnloadPolicy, BACKOFF_MS};
use super::lid_slot::{LidLoader, LidSession, LidSlot};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(300);
const IDLE_MS: u64 = 300_000;
const SAMPLES: [f32; 1600] = [0.0; 1600];
const BOUND: Duration = Duration::from_secs(2);
const THREADS: usize = 4;

/// A clock that only moves when the test says so.
struct FakeClock(AtomicU64);

impl FakeClock {
    fn new() -> Arc<FakeClock> {
        Arc::new(FakeClock(AtomicU64::new(0)))
    }

    fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a fake session does on its next run.
#[derive(Clone, Debug, PartialEq)]
enum Reply {
    Done(String),
    Fails(String),
    Panics,
}

/// What the fake loader does on its next call.
#[derive(Clone, Debug, PartialEq)]
enum LoadMode {
    Works,
    Fails(String),
}

/// Session side of a latch: announce entry, then wait (bounded) for the test to release it.
struct SessionLatch {
    entered: Sender<()>,
    release: Receiver<()>,
}

/// Test side of a latch.
struct TestLatch {
    entered: Receiver<()>,
    release: Sender<()>,
}

fn new_latch() -> (SessionLatch, TestLatch) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let session = SessionLatch {
        entered: entered_tx,
        release: release_rx,
    };
    let test = TestLatch {
        entered: entered_rx,
        release: release_tx,
    };
    (session, test)
}

/// Shared knobs and counters for the fake loader and sessions.
#[derive(Clone)]
struct Knobs {
    reply: Arc<Mutex<Reply>>,
    load: Arc<Mutex<LoadMode>>,
    latch: Arc<Mutex<Option<SessionLatch>>>,
    loads: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    sample_lens: Arc<Mutex<Vec<usize>>>,
}

impl Knobs {
    fn new(reply: Reply) -> Knobs {
        Knobs {
            reply: Arc::new(Mutex::new(reply)),
            load: Arc::new(Mutex::new(LoadMode::Works)),
            latch: Arc::new(Mutex::new(None)),
            loads: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
            sample_lens: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn drops(&self) -> usize {
        self.drops.load(Ordering::SeqCst)
    }

    fn sample_lens(&self) -> Vec<usize> {
        self.sample_lens.lock().unwrap().clone()
    }

    fn set_reply(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }

    fn set_load(&self, mode: LoadMode) {
        *self.load.lock().unwrap() = mode;
    }

    /// Arms a latch for the next session run only.
    fn arm_latch(&self, latch: SessionLatch) {
        *self.latch.lock().unwrap() = Some(latch);
    }
}

/// A fake session. Its `Drop` is counted, so tests can see when the slot released it.
struct FakeSession {
    knobs: Knobs,
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.knobs.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl LidSession for FakeSession {
    fn identify(&mut self, samples: &[f32]) -> Result<LidResult, String> {
        self.knobs.calls.fetch_add(1, Ordering::SeqCst);
        self.knobs.sample_lens.lock().unwrap().push(samples.len());
        let latch = self.knobs.latch.lock().unwrap().take();
        if let Some(latch) = latch {
            let _ = latch.entered.send(());
            let _ = latch.release.recv_timeout(BOUND);
        }
        // Clone the reply first, so no lock is held when the session panics.
        let reply = self.knobs.reply.lock().unwrap().clone();
        match reply {
            Reply::Done(text) => Ok(lid(&text)),
            Reply::Fails(message) => Err(message),
            Reply::Panics => panic!("fake session panic"),
        }
    }
}

/// Builds a slot whose loader counts loads and hands out `FakeSession`s.
fn slot_with(knobs: &Knobs, clock: &Arc<FakeClock>) -> LidSlot {
    let k = knobs.clone();
    let loader: LidLoader = Box::new(move || -> Result<Box<dyn LidSession>, String> {
        k.loads.fetch_add(1, Ordering::SeqCst);
        let mode = k.load.lock().unwrap().clone();
        match mode {
            LoadMode::Works => Ok(Box::new(FakeSession { knobs: k.clone() })),
            LoadMode::Fails(message) => Err(message),
        }
    });
    let clock_dyn: Arc<dyn Clock> = clock.clone();
    LidSlot::new(loader, clock_dyn)
}

/// The detected result the fake session produces for `text`.
fn lid(text: &str) -> LidResult {
    LidResult {
        lang: Some("ta".to_string()),
        text: text.to_string(),
    }
}

fn done_of(outcome: LidOutcome) -> LidResult {
    match outcome {
        LidOutcome::Done(result) => result,
        other => panic!("expected Done, got {other:?}"),
    }
}

fn message_of(outcome: LidOutcome) -> String {
    match outcome {
        LidOutcome::Unavailable(message) => message,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

fn recv_bounded<T>(rx: &Receiver<T>, what: &str) -> T {
    rx.recv_timeout(BOUND)
        .unwrap_or_else(|error| panic!("{what}: nothing within 2 s ({error})"))
}

/// Runs one identify on a spawned thread and returns the channel its outcome arrives on.
fn identify_on_thread(slot: &Arc<LidSlot>, policy: UnloadPolicy) -> Receiver<LidOutcome> {
    let (tx, rx) = mpsc::channel();
    let slot = slot.clone();
    thread::spawn(move || {
        let _ = tx.send(slot.identify(&SAMPLES, policy));
    });
    rx
}

/// Runs one tick on a spawned thread and returns a channel that fires when it returns.
fn tick_on_thread(slot: &Arc<LidSlot>, policy: UnloadPolicy) -> Receiver<()> {
    let (tx, rx) = mpsc::channel();
    let slot = slot.clone();
    thread::spawn(move || {
        slot.tick(policy);
        let _ = tx.send(());
    });
    rx
}

#[test]
fn construction_loads_nothing() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("x".into()));
    let slot = slot_with(&knobs, &clock);
    assert_eq!(knobs.loads(), 0);
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(slot.last_error(), None);
}

#[test]
fn first_call_loads_once_and_second_call_reuses_the_session() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("vanakkam".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("vanakkam"));
    assert_eq!(knobs.loads(), 1);
    assert_eq!(knobs.calls(), 1);
    assert!(slot.is_loaded());
    assert_eq!(slot.state_name(), "ready");
    assert_eq!(slot.last_error(), None);

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("vanakkam"));
    assert_eq!(knobs.loads(), 1, "the second call must reuse the session");
    assert_eq!(knobs.calls(), 2);
}

#[test]
fn empty_samples_are_unavailable_without_loading_or_backoff() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("x".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    let message = message_of(slot.identify(&[], policy));
    assert!(!message.is_empty());
    assert_eq!(knobs.loads(), 0, "empty audio must not load the model");
    assert_eq!(knobs.calls(), 0);
    assert_eq!(
        slot.state_name(),
        "unloaded",
        "empty audio must not back off"
    );
    assert_eq!(slot.last_error(), None);

    // Not in backoff, so the next real call works at once.
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("x"));
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn immediately_drops_the_session_after_each_call_and_reloads_next_time() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::Immediately;

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(
        knobs.drops(),
        1,
        "the session must be dropped after the call"
    );
    assert_eq!(knobs.loads(), 1);

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));
    assert!(!slot.is_loaded());
    assert_eq!(
        knobs.loads(),
        2,
        "each eligible call builds a fresh session"
    );
    assert_eq!(knobs.drops(), 2);

    slot.tick(policy);
    assert_eq!(knobs.loads(), 2, "tick on an unloaded slot loads nothing");
}

#[test]
fn never_keeps_the_session_across_a_huge_clock_jump() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::Never;
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));

    clock.advance(u64::MAX / 4);
    slot.tick(policy);
    assert!(slot.is_loaded());
    assert_eq!(knobs.drops(), 0);

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn after_idle_unloads_only_when_the_idle_time_has_passed_then_reloads() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));

    clock.advance(IDLE_MS - 1);
    slot.tick(policy);
    assert!(slot.is_loaded(), "must not unload before the idle time");
    assert_eq!(knobs.drops(), 0);

    clock.advance(1);
    slot.tick(policy);
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(knobs.drops(), 1, "the idle session must be dropped");
    assert_eq!(knobs.calls(), 1, "tick must not run the session");

    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("a"));
    assert_eq!(knobs.loads(), 2, "the next identify reloads");
}

#[test]
fn busy_returns_at_once_and_the_running_call_still_delivers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("first".into()));
    let (session_latch, test_latch) = new_latch();
    knobs.arm_latch(session_latch);
    let slot = Arc::new(slot_with(&knobs, &clock));
    let policy = UnloadPolicy::After(IDLE);

    let running = identify_on_thread(&slot, policy);
    recv_bounded(&test_latch.entered, "the first call entered the session");

    // Second caller while the first holds the slot: Busy at once, nothing changes.
    let state_before = slot.state_name();
    let started = Instant::now();
    let second = identify_on_thread(&slot, policy);
    let busy = recv_bounded(&second, "the second call returned before release");
    let elapsed = started.elapsed();
    assert_eq!(busy, LidOutcome::Busy);
    assert!(
        elapsed < Duration::from_millis(500),
        "busy must not wait, took {elapsed:?}"
    );
    assert_eq!(slot.state_name(), state_before);
    assert_eq!(slot.last_error(), None, "busy must not start a backoff");
    assert_eq!(knobs.loads(), 1, "busy must not call the loader");

    // A tick during the running call must not block or unload, even when the idle time is due.
    clock.advance(IDLE_MS);
    let ticked = tick_on_thread(&slot, policy);
    recv_bounded(&ticked, "tick returned during a running call");
    assert!(slot.is_loaded(), "tick must not unload a session in use");
    assert_eq!(knobs.drops(), 0);

    test_latch.release.send(()).unwrap();
    assert_eq!(
        recv_bounded(&running, "the first call delivered"),
        LidOutcome::Done(lid("first"))
    );

    knobs.set_reply(Reply::Done("third".into()));
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("third"));
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn loader_failure_backs_off_and_retries_only_after_it_then_recovers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("ok".into()));
    knobs.set_load(LoadMode::Fails("boom".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    let message = message_of(slot.identify(&SAMPLES, policy));
    assert!(message.contains("boom"), "{message}");
    assert_eq!(slot.state_name(), "backoff");
    let last = slot.last_error().expect("the load error is kept");
    assert!(last.contains("boom"), "{last}");
    assert_eq!(knobs.loads(), 1);

    clock.advance(BACKOFF_MS - 1);
    assert!(matches!(
        slot.identify(&SAMPLES, policy),
        LidOutcome::Unavailable(_)
    ));
    assert_eq!(
        knobs.loads(),
        1,
        "no load attempt inside the backoff window"
    );

    clock.advance(1);
    knobs.set_load(LoadMode::Works);
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("ok"));
    assert_eq!(
        knobs.loads(),
        2,
        "the loader is tried again after the backoff"
    );
    assert_eq!(slot.state_name(), "ready");
    assert_eq!(
        slot.last_error(),
        None,
        "a successful load clears the error"
    );
}

#[test]
fn run_failure_drops_the_session_backs_off_and_reloads_after_it() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Fails("bad run".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    let message = message_of(slot.identify(&SAMPLES, policy));
    assert!(message.contains("bad run"), "{message}");
    assert_eq!(knobs.drops(), 1, "the failed session must be dropped");
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "backoff");
    assert_eq!(knobs.calls(), 1);

    clock.advance(BACKOFF_MS - 1);
    assert!(matches!(
        slot.identify(&SAMPLES, policy),
        LidOutcome::Unavailable(_)
    ));
    assert_eq!(knobs.loads(), 1, "no reload during backoff");
    assert_eq!(knobs.calls(), 1, "no session run during backoff");

    clock.advance(1);
    knobs.set_reply(Reply::Done("again".into()));
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("again"));
    assert_eq!(
        knobs.loads(),
        2,
        "a dropped session is rebuilt after the backoff"
    );
}

#[test]
fn panicking_session_is_unavailable_dropped_and_the_slot_recovers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Panics);
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    let message = message_of(slot.identify(&SAMPLES, policy));
    assert!(message.to_lowercase().contains("panic"), "{message}");
    assert_eq!(knobs.drops(), 1, "the panicking session must be dropped");
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "backoff");
    assert!(
        slot.last_error().is_some(),
        "the panic is kept as the last error"
    );

    assert!(matches!(
        slot.identify(&SAMPLES, policy),
        LidOutcome::Unavailable(_)
    ));
    assert_eq!(knobs.calls(), 1, "no session run inside the backoff window");

    clock.advance(BACKOFF_MS);
    knobs.set_reply(Reply::Done("back".into()));
    assert_eq!(done_of(slot.identify(&SAMPLES, policy)), lid("back"));
    assert_eq!(knobs.loads(), 2);
}

#[test]
fn samples_reach_the_session_unchanged_and_are_not_capped_here() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("long".into()));
    let slot = slot_with(&knobs, &clock);
    // 45 s at 16 kHz. The 30 s cap belongs to the caller, so the slot must pass all of it on.
    let long = vec![0.25_f32; 720_000];

    assert_eq!(
        done_of(slot.identify(&long, UnloadPolicy::Never)),
        lid("long")
    );
    assert_eq!(knobs.sample_lens(), vec![720_000]);
}

#[test]
fn four_concurrent_callers_give_one_done_and_three_busy_with_one_load() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Done("x".into()));
    let (session_latch, test_latch) = new_latch();
    knobs.arm_latch(session_latch);
    let slot = Arc::new(slot_with(&knobs, &clock));
    let start = Arc::new(Barrier::new(THREADS));
    let (tx, rx) = mpsc::channel::<LidOutcome>();

    for _ in 0..THREADS {
        let slot = slot.clone();
        let start = start.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            start.wait();
            let _ = tx.send(slot.identify(&SAMPLES, UnloadPolicy::Never));
        });
    }
    drop(tx);

    // The winner holds the session until the test releases it, so the three others must
    // report Busy while it is still inside.
    recv_bounded(&test_latch.entered, "the winner entered the session");
    let losers: Vec<LidOutcome> = (0..THREADS - 1)
        .map(|_| recv_bounded(&rx, "a losing caller reported"))
        .collect();
    assert!(
        losers.iter().all(|outcome| *outcome == LidOutcome::Busy),
        "{losers:?}"
    );

    test_latch.release.send(()).unwrap();
    assert_eq!(
        recv_bounded(&rx, "the winner reported"),
        LidOutcome::Done(lid("x"))
    );
    assert_eq!(knobs.loads(), 1, "the loader must run exactly once");
}
