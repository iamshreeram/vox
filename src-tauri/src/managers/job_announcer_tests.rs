//! RED tests for `Announcer` (M3 S1b, DESIGN R2-6). Real short durations only.

use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

/// Scriptable fake mouth. Answers come from `script` first, then `default_answer`.
struct FakeMouth {
    script: Mutex<VecDeque<bool>>,
    default_answer: AtomicBool,
    calls: AtomicUsize,
    spoken: Mutex<Vec<String>>,
    reentrant: Mutex<Option<Weak<Announcer>>>,
    reentered: AtomicBool,
}

impl FakeMouth {
    fn new(default_answer: bool) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(VecDeque::new()),
            default_answer: AtomicBool::new(default_answer),
            calls: AtomicUsize::new(0),
            spoken: Mutex::new(Vec::new()),
            reentrant: Mutex::new(None),
            reentered: AtomicBool::new(false),
        })
    }
    fn script(&self, answers: &[bool]) {
        self.script.lock().unwrap().extend(answers.iter().copied());
    }
    fn set_default(&self, answer: bool) {
        self.default_answer.store(answer, Ordering::SeqCst);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn spoken(&self) -> Vec<String> {
        self.spoken.lock().unwrap().clone()
    }
}

impl LowPriorityMouth for FakeMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let weak = self.reentrant.lock().unwrap().clone();
        if let Some(weak) = weak {
            if !self.reentered.swap(true, Ordering::SeqCst) {
                if let Some(announcer) = weak.upgrade() {
                    // Re-entrant call from inside the mouth: must not deadlock.
                    announcer.announce(AnnounceKind::Progress, "reentrant".to_string());
                }
            }
        }
        let scripted = self.script.lock().unwrap().pop_front();
        let answer = scripted.unwrap_or_else(|| self.default_answer.load(Ordering::SeqCst));
        if answer {
            self.spoken.lock().unwrap().push(text.to_string());
        }
        answer
    }
}

fn announcer(mouth: &Arc<FakeMouth>, retry: Duration, give_up: Duration) -> Arc<Announcer> {
    Arc::new(Announcer::with_timing(mouth.clone(), retry, give_up))
}

fn fast(mouth: &Arc<FakeMouth>) -> Arc<Announcer> {
    announcer(mouth, Duration::from_millis(10), Duration::from_secs(5))
}

fn wait_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if cond() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn texts(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn idle_progress_is_spoken_immediately_once() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.announce(AnnounceKind::Progress, "step one".to_string());
    assert_eq!(mouth.spoken(), texts(&["step one"]));
    assert_eq!(mouth.calls(), 1);
    assert!(a.take_pending().is_empty(), "spoken text is not queued");
}

#[test]
fn idle_finish_is_spoken_immediately_once() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.announce(AnnounceKind::Finish, "job done".to_string());
    assert_eq!(mouth.spoken(), texts(&["job done"]));
    assert_eq!(mouth.calls(), 1);
}

#[test]
fn conversation_active_queues_without_speaking() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Progress, "queued".to_string());
    thread::sleep(Duration::from_millis(30));
    assert_eq!(mouth.calls(), 0, "mouth must not be called while active");
    assert!(mouth.spoken().is_empty());
}

#[test]
fn take_pending_returns_queued_texts_in_arrival_order_and_clears() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Progress, "first".to_string());
    a.announce(AnnounceKind::Progress, "second".to_string());
    a.announce(AnnounceKind::Finish, "third".to_string());
    assert_eq!(a.take_pending(), texts(&["first", "second", "third"]));
    assert!(a.take_pending().is_empty(), "take_pending clears the queue");
}

#[test]
fn take_pending_with_nothing_queued_returns_empty() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    assert!(a.take_pending().is_empty());
}

#[test]
fn confirmation_guard_holds_speech_until_the_last_drop() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    let guard = a.confirmation_guard();
    a.announce(AnnounceKind::Finish, "done".to_string());
    thread::sleep(Duration::from_millis(40));
    assert_eq!(mouth.calls(), 0, "nothing spoken while confirming");
    assert!(mouth.spoken().is_empty());
    drop(guard);
    assert!(
        wait_until(
            || mouth.spoken() == texts(&["done"]),
            Duration::from_secs(2)
        ),
        "flushed after the guard drops"
    );
    thread::sleep(Duration::from_millis(30));
    assert_eq!(mouth.calls(), 1, "spoken exactly once");
}

#[test]
fn nested_confirmation_guards_release_only_after_the_last_drop() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    let outer = a.confirmation_guard();
    let inner = a.confirmation_guard();
    a.announce(AnnounceKind::Progress, "held".to_string());
    drop(outer);
    thread::sleep(Duration::from_millis(40));
    assert!(mouth.spoken().is_empty(), "inner guard still alive");
    drop(inner);
    assert!(wait_until(
        || mouth.spoken() == texts(&["held"]),
        Duration::from_secs(2)
    ));
}

#[test]
fn confirmation_guard_drop_does_not_flush_while_conversation_is_active() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    let guard = a.confirmation_guard();
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Finish, "done".to_string());
    drop(guard);
    thread::sleep(Duration::from_millis(40));
    assert_eq!(mouth.calls(), 0, "active conversation keeps speech held");
    assert_eq!(a.take_pending(), texts(&["done"]));
}

#[test]
fn busy_finish_is_retried_then_spoken_exactly_once() {
    let mouth = FakeMouth::new(true);
    mouth.script(&[false, false]);
    let a = fast(&mouth);
    a.announce(AnnounceKind::Finish, "done".to_string());
    assert!(
        wait_until(
            || mouth.spoken() == texts(&["done"]),
            Duration::from_secs(2)
        ),
        "retried until the mouth is idle"
    );
    thread::sleep(Duration::from_millis(80));
    assert_eq!(mouth.spoken().len(), 1, "spoken exactly once");
    assert_eq!(mouth.calls(), 3, "two busy answers then one success");
    assert!(a.take_pending().is_empty());
}

#[test]
fn busy_progress_is_dropped_and_never_retried() {
    let mouth = FakeMouth::new(false);
    let a = fast(&mouth);
    a.announce(AnnounceKind::Progress, "busy progress".to_string());
    thread::sleep(Duration::from_millis(80));
    assert_eq!(mouth.calls(), 1, "a busy progress is never retried");
    mouth.set_default(true);
    a.notify_idle();
    thread::sleep(Duration::from_millis(40));
    assert!(mouth.spoken().is_empty(), "dropped progress never spoken");
    assert!(a.take_pending().is_empty(), "dropped progress not queued");
}

#[test]
fn queue_max_four_drops_oldest_progress_first() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    for label in ["p1", "p2", "p3", "p4"] {
        a.announce(AnnounceKind::Progress, label.to_string());
    }
    a.announce(AnnounceKind::Progress, "p5".to_string());
    a.announce(AnnounceKind::Finish, "f".to_string());
    assert_eq!(a.take_pending(), texts(&["p3", "p4", "p5", "f"]));
}

#[test]
fn queue_never_drops_a_finish_while_a_progress_can_be_dropped() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Finish, "f1".to_string());
    a.announce(AnnounceKind::Progress, "p1".to_string());
    a.announce(AnnounceKind::Progress, "p2".to_string());
    a.announce(AnnounceKind::Progress, "p3".to_string());
    a.announce(AnnounceKind::Finish, "f2".to_string());
    a.announce(AnnounceKind::Finish, "f3".to_string());
    assert_eq!(a.take_pending(), texts(&["f1", "p3", "f2", "f3"]));
}

#[test]
fn queue_full_of_finishes_keeps_every_finish_and_drops_incoming_progress() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    for label in ["f1", "f2", "f3", "f4"] {
        a.announce(AnnounceKind::Finish, label.to_string());
    }
    a.announce(AnnounceKind::Progress, "p1".to_string());
    assert_eq!(a.take_pending(), texts(&["f1", "f2", "f3", "f4"]));
}

#[test]
fn give_up_after_drops_a_never_idle_finish() {
    let mouth = FakeMouth::new(false);
    let a = announcer(
        &mouth,
        Duration::from_millis(10),
        Duration::from_millis(120),
    );
    a.announce(AnnounceKind::Finish, "never".to_string());
    thread::sleep(Duration::from_millis(300));
    assert!(mouth.calls() >= 2, "retried before giving up");
    let settled = mouth.calls();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(mouth.calls(), settled, "no retries after give_up_after");
    mouth.set_default(true);
    a.notify_idle();
    thread::sleep(Duration::from_millis(30));
    assert!(
        mouth.spoken().is_empty(),
        "the dropped finish is never spoken"
    );
    assert!(a.take_pending().is_empty());
}

#[test]
fn set_conversation_active_false_flushes_pending() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Finish, "done".to_string());
    assert!(mouth.spoken().is_empty());
    a.set_conversation_active(false);
    assert!(wait_until(
        || mouth.spoken() == texts(&["done"]),
        Duration::from_secs(2)
    ));
    assert!(a.take_pending().is_empty(), "flushed items leave the queue");
}

#[test]
fn flush_with_busy_mouth_keeps_finish_pending_and_retries_it() {
    let mouth = FakeMouth::new(false);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Finish, "done".to_string());
    a.set_conversation_active(false);
    thread::sleep(Duration::from_millis(30));
    assert!(mouth.spoken().is_empty());
    mouth.set_default(true);
    assert!(wait_until(
        || mouth.spoken() == texts(&["done"]),
        Duration::from_secs(2)
    ));
    assert!(a.take_pending().is_empty());
}

#[test]
fn notify_idle_retries_immediately_without_waiting_for_the_timer() {
    let mouth = FakeMouth::new(false);
    let a = announcer(&mouth, Duration::from_secs(10), Duration::from_secs(60));
    a.announce(AnnounceKind::Finish, "done".to_string());
    assert_eq!(mouth.calls(), 1);
    mouth.set_default(true);
    a.notify_idle();
    assert!(
        wait_until(
            || mouth.spoken() == texts(&["done"]),
            Duration::from_secs(1)
        ),
        "notify_idle must not wait for the 10 s timer"
    );
    assert_eq!(mouth.calls(), 2);
}

#[test]
fn no_mouth_call_happens_while_the_announcer_lock_is_held() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    *mouth.reentrant.lock().unwrap() = Some(Arc::downgrade(&a));
    let (tx, rx) = mpsc::channel();
    let worker = a.clone();
    thread::spawn(move || {
        worker.announce(AnnounceKind::Finish, "outer".to_string());
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "announce deadlocked: mouth re-entered the announcer"
    );
    assert!(mouth.spoken().contains(&"outer".to_string()));
    assert!(mouth.spoken().contains(&"reentrant".to_string()));
}

#[test]
fn empty_and_whitespace_text_is_ignored_everywhere() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.announce(AnnounceKind::Progress, String::new());
    a.announce(AnnounceKind::Finish, "   ".to_string());
    assert_eq!(mouth.calls(), 0, "empty text never reaches the mouth");
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Progress, "\n\t".to_string());
    assert!(a.take_pending().is_empty(), "empty text is never queued");
}

#[test]
fn finish_is_ordered_after_progress() {
    let mouth = FakeMouth::new(true);
    let a = fast(&mouth);
    a.set_conversation_active(true);
    a.announce(AnnounceKind::Progress, "p".to_string());
    a.announce(AnnounceKind::Finish, "f".to_string());
    a.set_conversation_active(false);
    assert!(wait_until(
        || mouth.spoken() == texts(&["p", "f"]),
        Duration::from_secs(2)
    ));
}
