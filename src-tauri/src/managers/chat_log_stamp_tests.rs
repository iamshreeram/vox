//! G4a spec: admission gate for the chat log (DESIGN.md A8-1 and A9-1; A9-1 is the newest rule).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::managers::chat_log`, additions):
//!   struct Stamp { session_token: u64, rec_epoch: u64 }       (Debug, Clone, Copy, PartialEq, Eq)
//!   struct GateState { session_token: u64, rec_epoch: u64, recording: bool }   (Debug, Clone, Copy)
//!   struct AdmissionGate (one Mutex inside): new(recording) ; stamp(&self) -> Option<Stamp>
//!     (None while off) ; state(&self) -> GateState ; set_recording(&self, on) (bumps rec_epoch on
//!     EVERY call) ; bump_session(&self) -> u64 (new session_token) ; is_current(&self, Stamp) -> bool
//!   ChatEvent gains `pub(crate) stamp: Option<Stamp>` with #[serde(skip)] (never serialized).
//!   ChatLog::gate(&self) -> &AdmissionGate. ChatLog::say stamps each admitted line on the caller
//!   thread, at call time. Tests in this file are registered from managers/mod.rs.

use super::chat_log::*;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ---- a minimal recording emitter (chat_log_tests.rs keeps its own private one) ------------

struct Rec {
    events: Mutex<Vec<ChatEvent>>,
    cv: Condvar,
    open: Mutex<bool>,
    open_cv: Condvar,
}

impl Rec {
    fn new(open: bool) -> Arc<Rec> {
        Arc::new(Rec {
            events: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            open: Mutex::new(open),
            open_cv: Condvar::new(),
        })
    }

    fn set_open(&self, v: bool) {
        *self.open.lock().unwrap() = v;
        self.open_cv.notify_all();
    }

    /// Bounded wait for the first Entry event with `text`.
    fn wait_for_entry(&self, text: &str, ms: u64) -> Option<ChatEvent> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut guard = self.events.lock().unwrap();
        loop {
            if let Some(found) = guard.iter().find(|e| entry_text(e) == Some(text)) {
                return Some(found.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (g, _) = self.cv.wait_timeout(guard, deadline - now).unwrap();
            guard = g;
        }
    }
}

impl ChatEmitter for Rec {
    fn emit(&self, event: &ChatEvent) {
        {
            let mut open = self.open.lock().unwrap();
            while !*open {
                open = self.open_cv.wait(open).unwrap();
            }
        }
        self.events.lock().unwrap().push(event.clone());
        self.cv.notify_all();
    }
}

fn entry_text(e: &ChatEvent) -> Option<&str> {
    match &e.kind {
        ChatEventKind::Entry { entry } => Some(entry.text.as_str()),
        _ => None,
    }
}

// ---- AdmissionGate ------------------------------------------------------------------------

#[test]
fn stamp_is_stable_while_nothing_changes() {
    let gate = AdmissionGate::new(true);
    let first = gate.stamp().expect("recording is on");
    assert_eq!(gate.stamp(), Some(first));
    assert!(gate.is_current(first));
}

#[test]
fn toggling_recording_invalidates_an_old_stamp() {
    // A9-1: stamp, off, on -> the old stamp is never current again; the new one is.
    let gate = AdmissionGate::new(true);
    let old = gate.stamp().expect("on");
    gate.set_recording(false);
    gate.set_recording(true);
    assert!(!gate.is_current(old), "old stamp must be stale");
    let fresh = gate.stamp().expect("on again");
    assert!(gate.is_current(fresh));
    assert_ne!(fresh, old);
}

#[test]
fn stamp_is_none_while_recording_is_off() {
    let gate = AdmissionGate::new(false);
    assert_eq!(gate.stamp(), None);
    assert!(!gate.state().recording);
    gate.set_recording(true);
    assert!(gate.stamp().is_some());
    gate.set_recording(false);
    assert_eq!(gate.stamp(), None);
}

#[test]
fn set_recording_bumps_the_epoch_on_every_call() {
    // A8-1: rec_epoch moves on every call, both directions, even when the value repeats.
    let gate = AdmissionGate::new(true);
    let mut last = gate.state().rec_epoch;
    for on in [true, true, false, false, true] {
        gate.set_recording(on);
        let now = gate.state();
        assert_eq!(now.recording, on);
        assert!(
            now.rec_epoch > last,
            "epoch must grow: {} then {}",
            last,
            now.rec_epoch
        );
        last = now.rec_epoch;
    }
}

#[test]
fn bump_session_returns_the_new_token_and_invalidates_old_stamps() {
    // A8-1: session start bumps the token; every older stamp is stale.
    let gate = AdmissionGate::new(true);
    let old = gate.stamp().expect("on");
    let before = gate.state().session_token;
    let token = gate.bump_session();
    assert!(token > before, "token grows");
    assert_eq!(gate.state().session_token, token);
    assert!(!gate.is_current(old), "old session stamp is stale");
    let fresh = gate.stamp().expect("still recording");
    assert!(gate.is_current(fresh));
}

#[test]
fn bump_session_does_not_switch_recording_on() {
    let gate = AdmissionGate::new(true);
    gate.set_recording(false);
    gate.bump_session();
    assert!(
        !gate.state().recording,
        "a session start is not a recording change"
    );
    assert_eq!(gate.stamp(), None);
}

#[test]
fn concurrent_changes_never_leave_a_current_stamp_behind_a_later_change() {
    // A9-1: several threads stamp, then change the tuple; a stamp taken before a thread's own
    // change is never current afterwards. Epochs and tokens only grow, so this holds under any
    // interleaving.
    let gate = Arc::new(AdmissionGate::new(true));
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for t in 0..8_u64 {
        let gate = Arc::clone(&gate);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..200_u64 {
                let stamp = gate.stamp();
                match (t + i) % 3 {
                    0 => gate.set_recording(i % 2 == 0),
                    1 => {
                        gate.bump_session();
                    }
                    _ => gate.set_recording(true),
                }
                if let Some(s) = stamp {
                    assert!(
                        !gate.is_current(s),
                        "thread {t} iteration {i}: a stamp survived a later change"
                    );
                }
            }
        }));
    }
    for h in handles {
        h.join().expect("no stress thread panicked");
    }
}

// ---- ChatLog integration ------------------------------------------------------------------

#[test]
fn say_carries_the_stamp_that_was_current_at_call_time() {
    // A8-1: stamped synchronously on the caller thread, carried in the emitted event.
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let before = log.gate().stamp().expect("recording on");
    log.say(ChatRole::User, "stamped line");
    let ev = rec.wait_for_entry("stamped line", 3000).expect("delivered");
    assert_eq!(ev.stamp, Some(before));
    assert!(log.gate().is_current(before));
}

#[test]
fn a_line_said_before_a_toggle_is_delivered_with_its_old_stamp() {
    // A9-1: the worker is held back, the line is queued, recording goes off and on, then the
    // worker drains. The line arrives (delivery is unchanged) but its stamp is stale.
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "queued before toggle");
    log.set_recording(false);
    log.set_recording(true);
    rec.set_open(true);
    let ev = rec
        .wait_for_entry("queued before toggle", 3000)
        .expect("the admitted line is still delivered");
    let stamp = ev.stamp.expect("an admitted line carries a stamp");
    assert!(
        !log.gate().is_current(stamp),
        "a toggle after the say must make its stamp stale"
    );

    log.say(ChatRole::User, "after toggle");
    let fresh = rec
        .wait_for_entry("after toggle", 3000)
        .expect("delivered after the toggle");
    let fresh_stamp = fresh.stamp.expect("stamped");
    assert!(log.gate().is_current(fresh_stamp));
}

#[test]
fn a_bumped_session_stamps_later_lines_with_the_new_token() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let before = log.gate().stamp().expect("on");
    log.gate().bump_session();
    log.say(ChatRole::User, "new session line");
    let ev = rec
        .wait_for_entry("new session line", 3000)
        .expect("delivered");
    assert!(!log.gate().is_current(before), "old session stamp is stale");
    assert!(log.gate().is_current(ev.stamp.expect("stamped")));
}

#[test]
fn chat_log_recording_toggles_bump_the_gate_epoch_in_both_directions() {
    // A8-1: ChatLog::set_recording goes through the gate every time.
    let log = ChatLog::start(Rec::new(true), true);
    let e0 = log.gate().state().rec_epoch;
    log.set_recording(false);
    let e1 = log.gate().state().rec_epoch;
    assert!(e1 > e0, "off bumps the epoch");
    assert!(!log.gate().state().recording);
    log.set_recording(true);
    let e2 = log.gate().state().rec_epoch;
    assert!(e2 > e1, "on bumps the epoch");
    assert!(log.gate().state().recording);
    assert_eq!(log.recording(), log.gate().state().recording);
}

#[test]
fn serialized_entry_event_has_no_stamp_key() {
    // A8-1: the stamp is internal (#[serde(skip)]); the wire shape is unchanged.
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::Assistant, "hello wire");
    let ev = rec.wait_for_entry("hello wire", 3000).expect("delivered");
    assert!(ev.stamp.is_some(), "the in-process event carries the stamp");

    let json: serde_json::Value = serde_json::to_value(&ev).expect("serializes");
    let mut top: Vec<&str> = json
        .as_object()
        .expect("object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    top.sort();
    assert_eq!(top, vec!["kind", "seq"], "no stamp key at the top level");

    let mut inner: Vec<&str> = json["kind"]
        .as_object()
        .expect("kind object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    inner.sort();
    assert_eq!(inner, vec!["entry", "type"]);

    let mut entry_keys: Vec<&str> = json["kind"]["entry"]
        .as_object()
        .expect("entry object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    entry_keys.sort();
    assert_eq!(entry_keys, vec!["id", "role", "text", "tsMs"]);

    let text = serde_json::to_string(&ev).expect("serializes");
    assert!(!text.contains("stamp"), "no stamp in the wire text: {text}");
    assert!(!text.contains("sessionToken"));
    assert!(!text.contains("recEpoch"));
}
