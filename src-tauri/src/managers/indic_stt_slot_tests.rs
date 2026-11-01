//! Tests for `indic_stt_slot`: lazy session, Busy/Backoff rules, idle unload and the watcher.
//! Every test uses a hand-moved fake clock and fake sessions, so no model pack is needed.

use crate::managers::indic_stt_slot::{
    spawn_watcher, Clock, IndicSession, IndicSlot, SlotOutcome, SystemClock, UnloadPolicy,
    BACKOFF_MS,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const IDLE: Duration = Duration::from_secs(300);
const IDLE_MS: u64 = 300_000;
const SAMPLES: [f32; 1600] = [0.0; 1600];

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

/// What a fake session returns on its next `transcribe` call.
#[derive(Clone, Debug, PartialEq)]
enum Reply {
    Text(String),
    Err(String),
    Panic,
}

/// What the fake loader does on its next call.
#[derive(Clone, Debug, PartialEq)]
enum LoadMode {
    Ok,
    Err(String),
    Panic,
}

/// Two barriers: the session reaches `entered`, waits for `release`, then answers.
#[derive(Clone)]
struct Latch {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

/// Shared knobs and counters for the fake loader and sessions.
#[derive(Clone)]
struct Knobs {
    reply: Arc<Mutex<Reply>>,
    load: Arc<Mutex<LoadMode>>,
    latch: Arc<Mutex<Option<Latch>>>,
    loads: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    load_delay: Duration,
}

impl Knobs {
    fn new(reply: Reply) -> Knobs {
        Knobs {
            reply: Arc::new(Mutex::new(reply)),
            load: Arc::new(Mutex::new(LoadMode::Ok)),
            latch: Arc::new(Mutex::new(None)),
            loads: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            load_delay: Duration::ZERO,
        }
    }

    fn with_delay(mut self, delay: Duration) -> Knobs {
        self.load_delay = delay;
        self
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn set_reply(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }

    fn set_load(&self, mode: LoadMode) {
        *self.load.lock().unwrap() = mode;
    }

    /// Arms a latch for the next session run only.
    fn arm_latch(&self) -> Latch {
        let latch = Latch {
            entered: Arc::new(Barrier::new(2)),
            release: Arc::new(Barrier::new(2)),
        };
        *self.latch.lock().unwrap() = Some(latch.clone());
        latch
    }
}

struct FakeSession {
    knobs: Knobs,
}

impl IndicSession for FakeSession {
    fn transcribe(&mut self, _samples: &[f32]) -> Result<String, String> {
        self.knobs.calls.fetch_add(1, Ordering::SeqCst);
        let latch = self.knobs.latch.lock().unwrap().take();
        if let Some(latch) = latch {
            latch.entered.wait();
            latch.release.wait();
        }
        let reply = self.knobs.reply.lock().unwrap().clone();
        match reply {
            Reply::Text(text) => Ok(text),
            Reply::Err(message) => Err(message),
            Reply::Panic => panic!("fake session panic"),
        }
    }
}

/// Builds a slot whose loader counts loads and hands out `FakeSession`s.
fn slot_with(knobs: &Knobs, clock: &Arc<FakeClock>) -> IndicSlot {
    let k = knobs.clone();
    let loader = move || -> Result<Box<dyn IndicSession>, String> {
        k.loads.fetch_add(1, Ordering::SeqCst);
        if !k.load_delay.is_zero() {
            thread::sleep(k.load_delay);
        }
        let mode = k.load.lock().unwrap().clone();
        match mode {
            LoadMode::Ok => Ok(Box::new(FakeSession { knobs: k.clone() })),
            LoadMode::Err(message) => Err(message),
            LoadMode::Panic => panic!("fake loader panic"),
        }
    };
    let clock_dyn: Arc<dyn Clock> = clock.clone();
    IndicSlot::new(Box::new(loader), clock_dyn)
}

fn text_of(outcome: SlotOutcome) -> String {
    match outcome {
        SlotOutcome::Text(text) => text,
        other => panic!("expected Text, got {other:?}"),
    }
}

fn wait_until(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    condition()
}

#[test]
fn backoff_constant_is_thirty_seconds() {
    assert_eq!(BACKOFF_MS, 30_000);
}

#[test]
fn default_idle_policy_is_five_minutes() {
    assert_eq!(
        UnloadPolicy::default_idle(),
        UnloadPolicy::After(Duration::from_secs(300))
    );
}

#[test]
fn system_clock_never_goes_backwards() {
    let clock = SystemClock::new();
    let first = clock.now_ms();
    let second = clock.now_ms();
    assert!(second >= first);
}

#[test]
fn slot_is_unloaded_and_loads_nothing_on_construction() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("x".into()));
    let slot = slot_with(&knobs, &clock);
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(slot.last_error(), None);
    assert!(!slot.watcher_started());
    assert_eq!(knobs.loads(), 0);
}

#[test]
fn first_call_loads_once_and_returns_text() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("vanakkam".into()));
    let slot = slot_with(&knobs, &clock);
    let outcome = slot.transcribe(&SAMPLES, UnloadPolicy::After(IDLE));
    assert_eq!(outcome, SlotOutcome::Text("vanakkam".into()));
    assert_eq!(knobs.loads(), 1);
    assert_eq!(knobs.calls(), 1);
    assert!(slot.is_loaded());
    assert_eq!(slot.state_name(), "ready");
    assert_eq!(slot.last_error(), None);
}

#[test]
fn second_call_reuses_the_session() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "a");
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "a");
    assert_eq!(knobs.loads(), 1);
    assert_eq!(knobs.calls(), 2);
}

#[test]
fn idle_expiry_unloads_only_after_the_idle_time_and_without_a_request() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);
    slot.transcribe(&SAMPLES, policy);

    clock.advance(IDLE_MS - 1);
    slot.tick(policy);
    assert!(slot.is_loaded(), "must not unload before the idle time");

    clock.advance(1);
    slot.tick(policy);
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(knobs.calls(), 1, "tick must not run the session");
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn idle_time_counts_from_the_last_use() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);
    slot.transcribe(&SAMPLES, policy);

    clock.advance(200_000);
    slot.transcribe(&SAMPLES, policy);

    clock.advance(IDLE_MS - 1);
    slot.tick(policy);
    assert!(slot.is_loaded(), "idle time restarts at the second use");

    clock.advance(1);
    slot.tick(policy);
    assert!(!slot.is_loaded());
}

#[test]
fn immediately_drops_the_session_after_each_call() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("a".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::Immediately;

    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "a");
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "unloaded");
    assert_eq!(knobs.loads(), 1);

    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "a");
    assert!(!slot.is_loaded());
    assert_eq!(knobs.loads(), 2, "each call builds a fresh session");

    slot.tick(policy);
    assert_eq!(knobs.loads(), 2, "tick on an unloaded slot does nothing");
}

#[test]
fn never_keeps_the_session_across_a_huge_clock_jump() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("a".into()));
    let slot = slot_with(&knobs, &clock);
    slot.transcribe(&SAMPLES, UnloadPolicy::Never);

    clock.advance(u64::MAX / 4);
    slot.tick(UnloadPolicy::Never);
    assert!(slot.is_loaded());

    assert_eq!(text_of(slot.transcribe(&SAMPLES, UnloadPolicy::Never)), "a");
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn concurrent_callers_build_one_session() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("x".into())).with_delay(Duration::from_millis(200));
    let slot = Arc::new(slot_with(&knobs, &clock));

    let first = {
        let slot = slot.clone();
        thread::spawn(move || slot.transcribe(&SAMPLES, UnloadPolicy::Never))
    };
    thread::sleep(Duration::from_millis(50));
    let second = {
        let slot = slot.clone();
        thread::spawn(move || slot.transcribe(&SAMPLES, UnloadPolicy::Never))
    };

    let first_outcome = first.join().unwrap();
    let second_outcome = second.join().unwrap();
    assert_eq!(first_outcome, SlotOutcome::Text("x".into()));
    assert!(
        second_outcome == SlotOutcome::Busy || second_outcome == SlotOutcome::Text("x".into()),
        "second caller got {second_outcome:?}"
    );
    assert_eq!(knobs.loads(), 1, "the loader must run once");
}

#[test]
fn busy_returns_at_once_changes_nothing_and_the_running_call_still_delivers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("first".into()));
    let latch = knobs.arm_latch();
    let slot = Arc::new(slot_with(&knobs, &clock));

    let running = {
        let slot = slot.clone();
        thread::spawn(move || slot.transcribe(&SAMPLES, UnloadPolicy::Never))
    };
    latch.entered.wait(); // the first call is now inside the session, holding the slot

    let state_before = slot.state_name();
    let started = Instant::now();
    let busy = slot.transcribe(&SAMPLES, UnloadPolicy::Never);
    let elapsed = started.elapsed();
    assert_eq!(busy, SlotOutcome::Busy);
    assert!(
        elapsed < Duration::from_millis(100),
        "busy must not wait, took {elapsed:?}"
    );
    assert_eq!(slot.state_name(), state_before);
    assert_eq!(slot.state_name(), "ready");
    assert_eq!(slot.last_error(), None, "busy must not start a backoff");

    latch.release.wait();
    assert_eq!(running.join().unwrap(), SlotOutcome::Text("first".into()));

    knobs.set_reply(Reply::Text("third".into()));
    assert_eq!(
        slot.transcribe(&SAMPLES, UnloadPolicy::Never),
        SlotOutcome::Text("third".into())
    );
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn loader_error_enters_backoff_and_retries_only_after_it() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    knobs.set_load(LoadMode::Err("pack missing".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    assert_eq!(
        slot.transcribe(&SAMPLES, policy),
        SlotOutcome::Unavailable("pack missing".into())
    );
    assert_eq!(slot.state_name(), "backoff");
    assert_eq!(slot.last_error(), Some("pack missing".into()));
    assert_eq!(knobs.loads(), 1);

    clock.advance(BACKOFF_MS - 1);
    assert_eq!(
        slot.transcribe(&SAMPLES, policy),
        SlotOutcome::Unavailable("pack missing".into())
    );
    assert_eq!(
        knobs.loads(),
        1,
        "no load attempt inside the backoff window"
    );

    clock.advance(1);
    knobs.set_load(LoadMode::Ok);
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "ok");
    assert_eq!(knobs.loads(), 2);
    assert_eq!(slot.state_name(), "ready");
}

#[test]
fn session_error_drops_the_session_backs_off_and_retries_after_expiry() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Err("decode failed".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    assert_eq!(
        slot.transcribe(&SAMPLES, policy),
        SlotOutcome::Unavailable("decode failed".into())
    );
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "backoff");
    assert_eq!(knobs.calls(), 1);

    clock.advance(BACKOFF_MS - 1);
    assert!(matches!(
        slot.transcribe(&SAMPLES, policy),
        SlotOutcome::Unavailable(_)
    ));
    assert_eq!(knobs.calls(), 1, "no session run inside the backoff window");

    clock.advance(1);
    knobs.set_reply(Reply::Text("again".into()));
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "again");
    assert_eq!(knobs.loads(), 2, "a dropped session is rebuilt");
}

#[test]
fn panicking_session_is_unavailable_and_the_slot_recovers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Panic);
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    match slot.transcribe(&SAMPLES, policy) {
        SlotOutcome::Unavailable(message) => assert!(message.contains("fake session panic")),
        other => panic!("expected Unavailable, got {other:?}"),
    }
    assert!(!slot.is_loaded());
    assert_eq!(slot.state_name(), "backoff");

    assert!(matches!(
        slot.transcribe(&SAMPLES, policy),
        SlotOutcome::Unavailable(_)
    ));
    assert_eq!(knobs.calls(), 1);

    clock.advance(BACKOFF_MS);
    knobs.set_reply(Reply::Text("back".into()));
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "back");
}

#[test]
fn panicking_loader_is_unavailable_and_the_slot_recovers() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    knobs.set_load(LoadMode::Panic);
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::After(IDLE);

    match slot.transcribe(&SAMPLES, policy) {
        SlotOutcome::Unavailable(message) => assert!(message.contains("fake loader panic")),
        other => panic!("expected Unavailable, got {other:?}"),
    }
    assert_eq!(slot.state_name(), "backoff");

    clock.advance(BACKOFF_MS);
    knobs.set_load(LoadMode::Ok);
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "ok");
    assert_eq!(knobs.loads(), 2);
}

#[test]
fn poisoned_owner_lock_is_recovered_and_the_session_dropped() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    let slot = slot_with(&knobs, &clock);
    let policy = UnloadPolicy::Never;
    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "ok");

    slot.poison_owner_for_test();

    assert_eq!(text_of(slot.transcribe(&SAMPLES, policy)), "ok");
    assert_eq!(
        knobs.loads(),
        2,
        "the poisoned session was dropped and rebuilt"
    );
    slot.tick(UnloadPolicy::Never);
    assert!(slot.is_loaded());
}

#[test]
fn watcher_unloads_an_idle_session_without_another_request() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    let slot = Arc::new(slot_with(&knobs, &clock));
    assert_eq!(
        text_of(slot.transcribe(&SAMPLES, UnloadPolicy::After(IDLE))),
        "ok"
    );

    clock.advance(IDLE_MS);
    let policy_fn: Arc<dyn Fn() -> UnloadPolicy + Send + Sync> =
        Arc::new(|| UnloadPolicy::After(IDLE));
    spawn_watcher(&slot, policy_fn, Duration::from_millis(20));

    assert!(
        wait_until(Duration::from_secs(2), || !slot.is_loaded()),
        "the watcher did not unload the idle session"
    );
    assert_eq!(knobs.calls(), 1, "no request ran the session");
    assert_eq!(knobs.loads(), 1);
}

#[test]
fn spawn_watcher_twice_starts_only_one_thread() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    let slot = Arc::new(slot_with(&knobs, &clock));
    let policy_fn: Arc<dyn Fn() -> UnloadPolicy + Send + Sync> = Arc::new(|| UnloadPolicy::Never);

    spawn_watcher(&slot, policy_fn.clone(), Duration::from_millis(20));
    spawn_watcher(&slot, policy_fn, Duration::from_millis(20));

    assert!(slot.watcher_started());
    assert_eq!(slot.watcher_spawn_count(), 1);
}

#[test]
fn watcher_thread_exits_when_the_slot_is_dropped() {
    let clock = FakeClock::new();
    let knobs = Knobs::new(Reply::Text("ok".into()));
    let slot = Arc::new(slot_with(&knobs, &clock));
    let policy_fn: Arc<dyn Fn() -> UnloadPolicy + Send + Sync> = Arc::new(|| UnloadPolicy::Never);
    spawn_watcher(&slot, policy_fn, Duration::from_millis(20));
    assert!(slot.watcher_running());

    let alive: Arc<AtomicBool> = slot.watcher_alive_flag();
    drop(slot);
    assert!(
        wait_until(Duration::from_secs(2), || !alive.load(Ordering::SeqCst)),
        "the watcher thread did not exit after the slot was dropped"
    );
}
