//! Spec for `tts_serial` (step S5a). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it.
use crate::managers::emotion::Emotion;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use crate::managers::tts_serial::*;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[derive(Clone, Debug, PartialEq)]
struct Call {
    text: String,
    voice: Option<String>,
    rate: Option<u32>,
    emotion: Emotion,
}

/// Shared state of the fake inner `say` backend.
struct Inner {
    calls: Mutex<Vec<Call>>,
    running: AtomicUsize,
    max_running: AtomicUsize,
    kills: AtomicUsize,
    /// A child runs until `finish_current` / its own kill (or `child_ms` elapse when > 0).
    child_ms: Mutex<u64>,
    finish: AtomicBool,
    kill_ok: AtomicBool,
    fail_texts: Mutex<Vec<String>>,
    /// Blocks `spawn_styled` until released (simulates a slow process start).
    start_gate: AtomicBool,
    start_gated: AtomicBool,
}

impl Inner {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
            kills: AtomicUsize::new(0),
            child_ms: Mutex::new(0),
            finish: AtomicBool::new(false),
            kill_ok: AtomicBool::new(true),
            fail_texts: Mutex::new(Vec::new()),
            start_gate: AtomicBool::new(true),
            start_gated: AtomicBool::new(false),
        })
    }
    fn texts(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|c| c.text.clone()).collect()
    }
    fn finish_current(&self) {
        self.finish.store(true, Ordering::SeqCst);
    }
}

struct FakeChild {
    inner: Arc<Inner>,
    started: Instant,
    alive: bool,
}

impl SpeechHandle for FakeChild {
    fn kill(&mut self) -> bool {
        self.inner.kills.fetch_add(1, Ordering::SeqCst);
        if !self.inner.kill_ok.load(Ordering::SeqCst) {
            return false;
        }
        if self.alive {
            self.alive = false;
            self.inner.running.fetch_sub(1, Ordering::SeqCst);
        }
        true
    }
    fn is_running(&mut self) -> bool {
        if self.alive {
            let ms_cfg = *self.inner.child_ms.lock().unwrap();
            let done = (ms_cfg > 0 && self.started.elapsed() >= ms(ms_cfg))
                || self.inner.finish.swap(false, Ordering::SeqCst);
            if done {
                self.alive = false;
                self.inner.running.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.alive
    }
}

struct FakeSay(Arc<Inner>);

impl SpeechBackend for FakeSay {
    fn spawn(&self, text: &str, voice: Option<&str>, rate: Option<u32>) -> io::Result<Box<dyn SpeechHandle>> {
        self.spawn_styled(text, voice, rate, Emotion::Neutral)
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.0.calls.lock().unwrap().push(Call {
            text: text.into(),
            voice: voice.map(str::to_string),
            rate,
            emotion,
        });
        if self.0.start_gated.load(Ordering::SeqCst) {
            while !self.0.start_gate.load(Ordering::SeqCst) {
                std::thread::sleep(ms(2));
            }
        }
        if self.0.fail_texts.lock().unwrap().iter().any(|t| t == text) {
            return Err(io::Error::new(io::ErrorKind::Other, "cannot start"));
        }
        let now = self.0.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.max_running.fetch_max(now, Ordering::SeqCst);
        Ok(Box::new(FakeChild {
            inner: self.0.clone(),
            started: Instant::now(),
            alive: true,
        }))
    }
}

fn queue() -> (SerializedQueue, Arc<Inner>) {
    let inner = Inner::new();
    (SerializedQueue::new(Box::new(FakeSay(inner.clone()))), inner)
}

fn add(q: &SerializedQueue, text: &str) -> Box<dyn SpeechHandle> {
    q.enqueue_styled(text, None, None, Emotion::Neutral).expect("accepted")
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(ms(3));
    }
    cond()
}

#[test]
fn it_is_queue_capable_and_every_entry_point_appends() {
    let (q, inner) = queue();
    assert!(q.supports_queue());
    let h = q.spawn("one", None, None).unwrap();
    assert!(h.queue_capable());
    let h2 = q.spawn_styled("two", None, None, Emotion::Calm).unwrap();
    assert!(h2.queue_capable());
    let h3 = q.enqueue_styled("three", None, None, Emotion::Neutral).unwrap();
    assert!(h3.queue_capable());
    assert!(wait_until(ms(500), || inner.texts() == vec!["one"]));
    *inner.child_ms.lock().unwrap() = 20;
    assert!(wait_until(Duration::from_secs(3), || inner.texts() == vec!["one", "two", "three"]));
}

#[test]
fn items_run_strictly_one_at_a_time_in_order() {
    let (q, inner) = queue();
    *inner.child_ms.lock().unwrap() = 40;
    let mut h = add(&q, "a");
    add(&q, "b");
    add(&q, "c");
    assert!(wait_until(Duration::from_secs(3), || !h.is_running()));
    assert_eq!(inner.texts(), vec!["a", "b", "c"]);
    assert_eq!(inner.max_running.load(Ordering::SeqCst), 1, "never two children at once");
}

#[test]
fn is_running_is_true_while_anything_is_queued_or_speaking_then_false() {
    let (q, inner) = queue();
    let mut h = add(&q, "a");
    assert!(h.is_running(), "true right after acceptance");
    add(&q, "b");
    assert!(wait_until(ms(500), || inner.texts() == vec!["a"]));
    assert!(h.is_running());
    inner.finish_current();
    assert!(wait_until(ms(500), || inner.texts() == vec!["a", "b"]));
    assert!(h.is_running(), "b is speaking");
    inner.finish_current();
    assert!(wait_until(Duration::from_secs(1), || !h.is_running()));
}

#[test]
fn kill_clears_the_fifo_stops_the_child_and_nothing_stale_starts() {
    let (q, inner) = queue();
    let mut h = add(&q, "a");
    add(&q, "b");
    add(&q, "c");
    assert!(wait_until(ms(500), || inner.texts() == vec!["a"]));
    assert!(h.kill());
    assert!(!h.is_running());
    assert_eq!(inner.running.load(Ordering::SeqCst), 0);
    std::thread::sleep(ms(150));
    assert_eq!(inner.texts(), vec!["a"], "b and c must never start");
    // The queue works normally afterwards.
    let mut h2 = add(&q, "d");
    assert!(wait_until(ms(500), || inner.texts() == vec!["a", "d"]));
    inner.finish_current();
    assert!(wait_until(Duration::from_secs(1), || !h2.is_running()));
}

#[test]
fn kill_while_idle_is_true() {
    let (q, _inner) = queue();
    let mut h = add(&q, "a");
    assert!(h.kill());
    let mut again = add(&q, "b");
    assert!(again.kill());
    assert!(again.kill(), "idempotent");
}

#[test]
fn a_failed_child_kill_is_reported_and_the_queue_is_still_cleared() {
    let (q, inner) = queue();
    let mut h = add(&q, "a");
    add(&q, "b");
    assert!(wait_until(ms(500), || inner.texts() == vec!["a"]));
    inner.kill_ok.store(false, Ordering::SeqCst);
    assert!(!h.kill(), "unconfirmed stop must not be reported as stopped");
    assert!(h.is_running(), "the child is still alive");
    inner.kill_ok.store(true, Ordering::SeqCst);
    inner.finish_current();
    std::thread::sleep(ms(100));
    assert_eq!(inner.texts(), vec!["a"], "FIFO was cleared even though the kill failed");
}

#[test]
fn an_item_still_starting_when_kill_runs_is_killed_as_soon_as_it_has_started() {
    let (q, inner) = queue();
    inner.start_gated.store(true, Ordering::SeqCst);
    inner.start_gate.store(false, Ordering::SeqCst);
    let mut h = add(&q, "a");
    assert!(wait_until(ms(500), || inner.texts() == vec!["a"]), "start is blocked inside the inner spawn");
    h.kill();
    inner.start_gate.store(true, Ordering::SeqCst);
    assert!(wait_until(Duration::from_secs(1), || inner.running.load(Ordering::SeqCst) == 0));
    std::thread::sleep(ms(100));
    assert_eq!(inner.running.load(Ordering::SeqCst), 0, "the late child must not keep speaking");
    assert!(!h.is_running());
}

#[test]
fn a_start_failure_skips_only_that_item() {
    let (q, inner) = queue();
    inner.fail_texts.lock().unwrap().push("bad".into());
    *inner.child_ms.lock().unwrap() = 20;
    let mut h = add(&q, "bad");
    add(&q, "good");
    assert!(wait_until(Duration::from_secs(3), || !h.is_running()));
    assert_eq!(inner.texts(), vec!["bad", "good"]);
}

#[test]
fn the_fifo_is_bounded_and_refuses_with_would_block() {
    let (q, inner) = queue();
    add(&q, "first");
    assert!(wait_until(ms(500), || inner.texts() == vec!["first"]), "first is being spoken");
    for i in 0..SERIAL_MAX_PENDING {
        q.enqueue_styled(&format!("w{i}"), None, None, Emotion::Neutral)
            .unwrap_or_else(|e| panic!("waiting item {i} must be accepted: {e}"));
    }
    let err = q.enqueue_styled("overflow", None, None, Emotion::Neutral).err().expect("refused");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
    let err = q.spawn("overflow", None, None).err().expect("refused");
    assert_eq!(err.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn voice_rate_and_emotion_are_forwarded_unchanged() {
    let (q, inner) = queue();
    q.enqueue_styled("hello", Some("Samantha"), Some(210), Emotion::Excited).unwrap();
    assert!(wait_until(ms(500), || inner.calls.lock().unwrap().len() == 1));
    assert_eq!(
        inner.calls.lock().unwrap()[0],
        Call {
            text: "hello".into(),
            voice: Some("Samantha".into()),
            rate: Some(210),
            emotion: Emotion::Excited,
        }
    );
}

#[test]
fn dropping_the_adapter_kills_a_running_child() {
    let (q, inner) = queue();
    add(&q, "a");
    assert!(wait_until(ms(500), || inner.running.load(Ordering::SeqCst) == 1));
    drop(q);
    assert!(wait_until(Duration::from_secs(1), || inner.running.load(Ordering::SeqCst) == 0));
}
