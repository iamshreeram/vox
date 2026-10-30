//! Spec: speech signals travel in the chat log's ordered stream (design Revision 2, D2).
//!
//! The bubble must know which assistant lines belong to a spoken reply. The signals are queued
//! with the lines on the caller's thread and applied by the same worker, in call order, so a
//! reset, a begin, the lines and an end can never be reordered. Authoritative: do not weaken an
//! assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::managers::chat_log`, additions):
//!   trait ChatEmitter { ...; fn speech(&self, scope: u64, on: bool, stamp: Option<Stamp>) {} }
//!     // default no-op, called by the worker only, in order with `emit`
//!   ChatLog::speech_begin(&self, scope: u64)
//!     // stamped like a line (recording off => nothing queued); emitted as speech(scope, true, Some(stamp))
//!   ChatLog::speech_end(&self, scope: u64)
//!     // never refused by the recording flag; emitted as speech(scope, false, None)
//!   Marks are not log entries: not in `snapshot()`, no seq, never counted as dropped.
//!   Marks bypass PENDING_CAP (an end must never be lost). `reset()` discards queued marks like
//!   queued lines. After `shutdown()` the calls are harmless no-ops.
#![allow(dead_code)]

use super::chat_log::*;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Rec {
    log: Mutex<Vec<String>>,
    marks: Mutex<Vec<(u64, bool, Option<Stamp>)>>,
    /// Texts whose `emit` has been entered (before the gate), so a test knows the worker is in
    /// flight inside the emitter without sleeping.
    entered: Mutex<Vec<String>>,
    entered_cv: Condvar,
    cv: Condvar,
    open: Mutex<bool>,
    open_cv: Condvar,
}

impl Rec {
    fn new(open: bool) -> Arc<Rec> {
        Arc::new(Rec {
            log: Mutex::new(Vec::new()),
            marks: Mutex::new(Vec::new()),
            entered: Mutex::new(Vec::new()),
            entered_cv: Condvar::new(),
            cv: Condvar::new(),
            open: Mutex::new(open),
            open_cv: Condvar::new(),
        })
    }
    fn set_open(&self, v: bool) {
        *self.open.lock().unwrap() = v;
        self.open_cv.notify_all();
    }
    /// Blocks until the worker is inside `emit` for the entry with this text.
    fn wait_entered(&self, text: &str, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut guard = self.entered.lock().unwrap();
        loop {
            if guard.iter().any(|t| t == text) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (g, _) = self.entered_cv.wait_timeout(guard, deadline - now).unwrap();
            guard = g;
        }
    }
    fn wait_open(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.open_cv.wait(open).unwrap();
        }
    }
    fn push(&self, item: String) {
        self.log.lock().unwrap().push(item);
        self.cv.notify_all();
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
    fn wait_for<F: Fn(&[String]) -> bool>(&self, f: F, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut guard = self.log.lock().unwrap();
        loop {
            if f(&guard) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (g, _) = self.cv.wait_timeout(guard, deadline - now).unwrap();
            guard = g;
        }
    }
}

impl ChatEmitter for Rec {
    fn emit(&self, event: &ChatEvent) {
        if let ChatEventKind::Entry { entry } = &event.kind {
            self.entered.lock().unwrap().push(entry.text.clone());
            self.entered_cv.notify_all();
        }
        self.wait_open();
        match &event.kind {
            ChatEventKind::Entry { entry } => self.push(format!("E:{}", entry.text)),
            ChatEventKind::Reset => self.push("R".to_string()),
            ChatEventKind::Clear => self.push("C".to_string()),
            ChatEventKind::State { .. } | ChatEventKind::Config { .. } => {}
        }
    }
    fn speech(&self, scope: u64, on: bool, stamp: Option<Stamp>) {
        self.wait_open();
        self.marks.lock().unwrap().push((scope, on, stamp));
        self.push(format!("S{}{}", if on { "+" } else { "-" }, scope));
    }
}

/// An emitter that only implements `emit`: the default `speech` must exist and do nothing.
struct EmitOnly(Mutex<Vec<String>>);
impl ChatEmitter for EmitOnly {
    fn emit(&self, event: &ChatEvent) {
        if let ChatEventKind::Entry { entry } = &event.kind {
            self.0.lock().unwrap().push(entry.text.clone());
        }
    }
}

fn start(rec: &Arc<Rec>) -> Arc<ChatLog> {
    ChatLog::start(rec.clone(), true)
}

#[test]
fn signals_and_lines_come_out_in_call_order() {
    let rec = Rec::new(true);
    let log = start(&rec);
    log.speech_begin(1);
    log.say(ChatRole::Assistant, "a");
    log.say(ChatRole::Assistant, "b");
    log.speech_end(1);
    log.speech_begin(2);
    log.say(ChatRole::Assistant, "c");
    log.speech_end(2);
    assert!(rec.wait_for(|l| l.len() >= 7, 3_000), "got {:?}", rec.log());
    assert_eq!(
        rec.log(),
        vec!["S+1", "E:a", "E:b", "S-1", "S+2", "E:c", "S-2"]
    );
}

#[test]
fn a_begin_carries_a_stamp_and_an_end_carries_none() {
    let rec = Rec::new(true);
    let log = start(&rec);
    let stamp = log.gate().stamp().expect("recording is on");
    log.speech_begin(3);
    log.speech_end(3);
    assert!(rec.wait_for(|l| l.len() >= 2, 3_000));
    let marks = rec.marks.lock().unwrap().clone();
    assert_eq!(marks, vec![(3, true, Some(stamp)), (3, false, None)]);
}

#[test]
fn signals_are_not_transcript_lines() {
    let rec = Rec::new(true);
    let log = start(&rec);
    log.speech_begin(1);
    log.say(ChatRole::Assistant, "only line");
    log.speech_end(1);
    assert!(rec.wait_for(|l| l.len() >= 3, 3_000));
    let snap = log.snapshot();
    assert_eq!(snap.entries.len(), 1, "the signals are not entries");
    assert_eq!(snap.entries[0].text, "only line");
}

#[test]
fn a_begin_is_not_queued_while_recording_is_off_but_an_end_still_is() {
    let rec = Rec::new(true);
    let log = start(&rec);
    log.speech_begin(1);
    log.set_recording(false);
    log.speech_begin(2); // refused: no stamp while recording is off
    log.speech_end(2); // an end is never refused
    log.speech_end(1);
    assert!(
        rec.wait_for(|l| l.contains(&"S-1".to_string()), 3_000),
        "{:?}",
        rec.log()
    );
    let log_now = rec.log();
    assert!(log_now.contains(&"S+1".to_string()));
    assert!(
        !log_now.contains(&"S+2".to_string()),
        "no begin without a stamp"
    );
    assert!(log_now.contains(&"S-2".to_string()));
}

#[test]
fn a_reset_discards_queued_signals_and_lines_and_later_ones_work() {
    let rec = Rec::new(false);
    let log = start(&rec);
    log.say(ChatRole::Assistant, "blocker");
    // Wait until the worker is in flight inside the emitter (no sleeping).
    assert!(rec.wait_entered("blocker", 3_000));
    log.speech_begin(4);
    log.say(ChatRole::Assistant, "late");
    log.speech_end(4);
    log.reset();
    rec.set_open(true);
    log.speech_begin(5);
    assert!(
        rec.wait_for(|l| l.contains(&"S+5".to_string()), 3_000),
        "{:?}",
        rec.log()
    );
    assert_eq!(rec.log(), vec!["E:blocker", "R", "S+5"]);
}

fn lines_starting(all: &[String], prefix: &str) -> usize {
    all.iter().filter(|x| x.starts_with(prefix)).count()
}

#[test]
fn signals_bypass_the_pending_cap_and_do_not_change_the_line_accounting() {
    let rec = Rec::new(false);
    let log = start(&rec);
    log.say(ChatRole::Assistant, "blocker");
    assert!(rec.wait_entered("blocker", 3_000));
    // The blocker holds one pending slot while in flight, so PENDING_CAP - 1 more lines fit and
    // the other 21 are dropped. The two signals are queued after the cap is full.
    for i in 0..(PENDING_CAP + 20) {
        log.say(ChatRole::Assistant, &format!("l{i}"));
    }
    log.speech_begin(8);
    log.speech_end(8);
    rec.set_open(true);
    assert!(
        rec.wait_for(|l| l.contains(&"E:system:dropped:21".to_string()), 5_000),
        "{} entries so far",
        rec.log().len()
    );
    let all = rec.log();
    assert_eq!(
        lines_starting(&all, "E:l"),
        PENDING_CAP - 1,
        "exact admitted lines"
    );
    let begin_at = all
        .iter()
        .position(|x| x == "S+8")
        .expect("the begin was lost");
    let end_at = all
        .iter()
        .position(|x| x == "S-8")
        .expect("the end was lost");
    assert!(begin_at < end_at);
}

#[test]
fn discarding_queued_signals_does_not_free_the_slot_of_a_line_in_flight() {
    let rec = Rec::new(false);
    let log = start(&rec);
    log.say(ChatRole::Assistant, "blocker");
    assert!(rec.wait_entered("blocker", 3_000));
    log.speech_begin(4);
    log.say(ChatRole::Assistant, "late");
    log.speech_end(4);
    log.reset(); // discards the queued line and both queued signals
                 // Only the blocker is still pending, so PENDING_CAP - 1 new lines fit and 6 are dropped.
                 // If the discard also freed slots for the signals, more lines would be admitted. The drops
                 // and the wipe share one batch, and a wiped batch never announces drops, so there is no
                 // `dropped` marker here: an end signal queued last is the sentinel that tells us the batch
                 // is done.
    for i in 0..(PENDING_CAP + 5) {
        log.say(ChatRole::Assistant, &format!("x{i}"));
    }
    log.speech_end(9);
    rec.set_open(true);
    assert!(
        rec.wait_for(|l| l.contains(&"S-9".to_string()), 5_000),
        "{} entries so far",
        rec.log().len()
    );
    let all = rec.log();
    assert_eq!(lines_starting(&all, "E:x"), PENDING_CAP - 1);
    assert!(
        !all.iter().any(|x| x.starts_with("E:system:dropped")),
        "a wiped batch has no drop marker"
    );
    assert!(!all.contains(&"S+4".to_string()) && !all.contains(&"S-4".to_string()));
    assert!(!all.contains(&"E:late".to_string()));
}

#[test]
fn the_calls_are_harmless_after_shutdown() {
    let rec = Rec::new(true);
    let log = start(&rec);
    log.shutdown();
    log.speech_begin(1);
    log.speech_end(1);
}

#[test]
fn an_emitter_that_ignores_signals_still_works() {
    let sink = Arc::new(EmitOnly(Mutex::new(Vec::new())));
    let log = ChatLog::start(sink.clone(), true);
    log.speech_begin(1);
    log.say(ChatRole::Assistant, "hello");
    log.speech_end(1);
    let deadline = Instant::now() + Duration::from_millis(3_000);
    while Instant::now() < deadline && sink.0.lock().unwrap().is_empty() {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(sink.0.lock().unwrap().clone(), vec!["hello".to_string()]);
}
