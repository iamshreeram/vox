//! Spec tests for the chat platform tap (DESIGN-CHAT Rev 6, section 2.2, A15). Written before the
//! implementation; red by design while `chat_tap.rs` is a stub. Real `ChatLog` with a recording
//! emitter. Do not edit to make code pass. All waits are bounded.
#![allow(dead_code)]

use super::chat_log::*;
use super::chat_tap::ChatTap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// ---- recording emitter with a gate (copy of the chat_log_tests.rs helper) ------------------

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
    fn events(&self) -> Vec<ChatEvent> {
        self.events.lock().unwrap().clone()
    }
    fn wait_for<F: Fn(&[ChatEvent]) -> bool>(&self, f: F, ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut guard = self.events.lock().unwrap();
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

// ---- helpers ------------------------------------------------------------------------------

const END_LABELS: [&str; 7] = [
    "user_ended",
    "idle",
    "cancelled",
    "not_configured",
    "audio_unavailable",
    "speech_stuck",
    "turn_limit",
];

/// (role, text) of every Entry event, in order.
fn entries(events: &[ChatEvent]) -> Vec<(ChatRole, String)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            ChatEventKind::Entry { entry } => Some((entry.role, entry.text.clone())),
            _ => None,
        })
        .collect()
}

fn states(events: &[ChatEvent]) -> Vec<(String, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            ChatEventKind::State { state, end } => Some((state.clone(), end.clone())),
            _ => None,
        })
        .collect()
}

fn has_entry(events: &[ChatEvent], role: ChatRole, text: &str) -> bool {
    entries(events).iter().any(|(r, t)| *r == role && t == text)
}

fn has_state(events: &[ChatEvent], state: &str, end: Option<&str>) -> bool {
    states(events)
        .iter()
        .any(|(s, e)| s == state && e.as_deref() == end)
}

// ---- session start --------------------------------------------------------------------------

/// `on_session_start` empties the log, then records the fixed System line `system:started`.
#[test]
fn session_start_resets_then_records_the_started_line() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let tap = ChatTap::new(log.clone());
    tap.on_session_start();
    assert!(
        rec.wait_for(|e| e.len() >= 2, 3000),
        "two events expected: Reset then the System entry"
    );
    let events = rec.events();
    assert_eq!(events.len(), 2, "exactly Reset and one entry: {events:?}");
    assert_eq!(events[0].kind, ChatEventKind::Reset);
    assert_eq!(
        entries(&events[1..]),
        vec![(ChatRole::System, "system:started".to_string())]
    );
}

// ---- lines ----------------------------------------------------------------------------------

/// `on_chat` records the given role and text, in call order.
#[test]
fn on_chat_records_user_and_assistant_lines_in_order() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let tap = ChatTap::new(log.clone());
    tap.on_chat(ChatRole::User, "what is rust");
    tap.on_chat(ChatRole::Assistant, "Rust is a language.");
    assert!(
        rec.wait_for(|e| entries(e).len() >= 2, 3000),
        "both lines emitted"
    );
    assert_eq!(
        entries(&rec.events()),
        vec![
            (ChatRole::User, "what is rust".to_string()),
            (ChatRole::Assistant, "Rust is a language.".to_string()),
        ]
    );
}

// ---- state ----------------------------------------------------------------------------------

/// A plain state change produces a State event and no System line.
#[test]
fn on_state_listening_emits_a_state_event_only() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let tap = ChatTap::new(log.clone());
    tap.on_state("listening", None);
    assert!(
        rec.wait_for(|e| has_state(e, "listening", None), 3000),
        "State{{listening, None}} emitted"
    );
    assert!(
        entries(&rec.events()).is_empty(),
        "no System line for a plain state"
    );
}

/// Every end label produces the State event AND the System line `system:end:<label>`.
#[test]
fn on_state_idle_with_each_end_label_records_the_end_line() {
    for label in END_LABELS {
        let rec = Rec::new(true);
        let log = ChatLog::start(rec.clone(), true);
        let tap = ChatTap::new(log.clone());
        tap.on_state("idle", Some(label));
        let expected = format!("system:end:{label}");
        assert!(
            rec.wait_for(
                |e| has_state(e, "idle", Some(label)) && has_entry(e, ChatRole::System, &expected),
                3000
            ),
            "label {label}: State{{idle, Some({label})}} and `{expected}` expected, got {:?}",
            rec.events()
        );
    }
}

// ---- recording switched off -----------------------------------------------------------------

/// With recording off: `on_chat` and the end line record nothing, but state still updates.
#[test]
fn recording_off_records_no_lines_but_state_still_updates() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.set_recording(false);
    assert!(
        rec.wait_for(
            |e| e
                .iter()
                .any(|x| matches!(x.kind, ChatEventKind::Config { recording: false })),
            3000
        ),
        "recording switched off"
    );
    let tap = ChatTap::new(log.clone());
    tap.on_chat(ChatRole::User, "private line");
    tap.on_chat(ChatRole::Assistant, "private reply");
    tap.on_state("idle", Some("user_ended"));
    assert!(
        rec.wait_for(|e| has_state(e, "idle", Some("user_ended")), 3000),
        "the state change is still emitted"
    );
    let events = rec.events();
    assert!(
        entries(&events).is_empty(),
        "nothing recorded while off: {events:?}"
    );
    let snap = log.snapshot();
    assert_eq!(snap.state, "idle", "state updated while recording is off");
    assert_eq!(snap.end.as_deref(), Some("user_ended"));
    assert!(snap.entries.is_empty());
}

// ---- hooks never block ----------------------------------------------------------------------

/// With the emitter stuck, the hooks still return promptly: 5000 `on_chat` calls < 2 s.
#[test]
fn hooks_never_block_when_the_emitter_is_stuck() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    let tap = ChatTap::new(log.clone());
    let started = Instant::now();
    for i in 0..5000 {
        tap.on_chat(ChatRole::User, &format!("line {i}"));
    }
    tap.on_session_start();
    tap.on_state("idle", Some("user_ended"));
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "hooks blocked behind a stuck emitter: {took:?}"
    );
    rec.set_open(true);
    log.shutdown();
}

// ---- source contract ------------------------------------------------------------------------

/// chat_tap.rs does no file I/O and no logging in non-comment lines (DESIGN-CHAT A15: nothing
/// is written to disk or logged).
#[test]
fn chat_tap_source_has_no_file_io_or_logging() {
    let src = include_str!("chat_tap.rs");
    let forbidden = [
        "std::fs",
        "fs::write",
        "File::open",
        "File::create",
        "println!",
        "eprintln!",
        "print!(",
        "dbg!(",
        "tracing::",
        "log::info",
        "log::warn",
        "log::error",
        "log::debug",
        "log::trace",
        "info!(",
        "warn!(",
        "error!(",
        "debug!(",
        "trace!(",
    ];
    for (index, line) in src.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        for token in forbidden {
            assert!(
                !code.contains(token),
                "chat_tap.rs line {} contains forbidden `{token}`: {line}",
                index + 1
            );
        }
    }
}
