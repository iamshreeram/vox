//! Spec tests for `chat_log` (DESIGN-CHAT Rev 5: A4, A9, A10-A13). Written before the
//! implementation; red by design while `chat_log.rs` is a skeleton. Do not edit to make code
//! pass. All waits use bounded polling; no test sleeps unconditionally for long.
#![allow(dead_code)]

use super::chat_log::*;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ---- recording emitter with a gate and an optional one-shot panic -------------------------

struct Rec {
    events: Mutex<Vec<ChatEvent>>,
    cv: Condvar,
    open: Mutex<bool>,
    open_cv: Condvar,
    panic_on: Mutex<Option<String>>,
}

impl Rec {
    fn new(open: bool) -> Arc<Rec> {
        Arc::new(Rec {
            events: Mutex::new(Vec::new()),
            cv: Condvar::new(),
            open: Mutex::new(open),
            open_cv: Condvar::new(),
            panic_on: Mutex::new(None),
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
        if let ChatEventKind::Entry { entry } = &event.kind {
            let mut p = self.panic_on.lock().unwrap();
            if p.as_deref() == Some(entry.text.as_str()) {
                *p = None;
                drop(p);
                panic!("injected emitter panic");
            }
        }
        self.events.lock().unwrap().push(event.clone());
        self.cv.notify_all();
    }
}

fn texts(events: &[ChatEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            ChatEventKind::Entry { entry } => Some(entry.text.clone()),
            _ => None,
        })
        .collect()
}

fn entry_count(events: &[ChatEvent]) -> usize {
    texts(events).len()
}

fn has_text(events: &[ChatEvent], t: &str) -> bool {
    texts(events).iter().any(|x| x == t)
}

fn assert_seq_increasing(events: &[ChatEvent]) {
    let mut last = 0u64;
    for e in events {
        assert!(
            e.seq > last,
            "seq must strictly increase: {} after {}",
            e.seq,
            last
        );
        if let ChatEventKind::Entry { entry } = &e.kind {
            assert_eq!(entry.id, e.seq, "entry id equals its event seq");
        }
        last = e.seq;
    }
}

fn dropped_total(events: &[ChatEvent]) -> usize {
    texts(events)
        .iter()
        .filter_map(|t| t.strip_prefix("system:dropped:"))
        .map(|n| n.parse::<usize>().expect("dropped count parses"))
        .sum()
}

/// Wait until a sentinel System line has been applied, proving everything admitted before it
/// has been handled.
fn settle(log: &ChatLog, rec: &Rec) {
    log.say(ChatRole::System, "system:sentinel");
    assert!(
        rec.wait_for(|e| has_text(e, "system:sentinel"), 3000),
        "sentinel never emitted"
    );
}

// ---- cleaning and caps --------------------------------------------------------------------

#[test]
fn cleaning_strips_controls_keeps_newline_and_trims() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "  hi\u{7}\u{0} there\nline2\u{1b}[31m \t ");
    log.say(ChatRole::User, "a\tb\r\nc");
    settle(&log, &rec);
    let t = texts(&rec.events());
    assert_eq!(t[0], "hi there\nline2[31m");
    assert_eq!(t[1], "a b\nc");
}

#[test]
fn empty_and_control_only_text_is_ignored() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "");
    log.say(ChatRole::User, "   \u{7}\n \t");
    settle(&log, &rec);
    assert_eq!(texts(&rec.events()), vec!["system:sentinel".to_string()]);
}

#[test]
fn text_cap_is_exact_and_char_boundary_safe() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::Assistant, &"a".repeat(TEXT_CAP));
    log.say(ChatRole::Assistant, &"a".repeat(TEXT_CAP + 1));
    log.say(ChatRole::Assistant, &"é".repeat(3000));
    log.say(
        ChatRole::Assistant,
        &"\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}".repeat(800),
    );
    settle(&log, &rec);
    let t = texts(&rec.events());
    assert_eq!(
        t[0],
        "a".repeat(TEXT_CAP),
        "exactly at the cap is untouched"
    );
    assert_eq!(t[1].chars().count(), TEXT_CAP);
    assert!(t[1].ends_with('…'));
    assert_eq!(t[1].chars().filter(|c| *c == 'a').count(), TEXT_CAP - 1);
    assert_eq!(t[2].chars().count(), TEXT_CAP);
    assert!(t[2].ends_with('…'));
    assert_eq!(t[3].chars().count(), TEXT_CAP);
    assert!(t[3].ends_with('…'));
}

// ---- ordering, ids, cap -------------------------------------------------------------------

#[test]
fn initial_snapshot_is_idle_and_empty() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec, true);
    let s = log.snapshot();
    assert_eq!(s.seq, 0);
    assert!(s.entries.is_empty());
    assert_eq!(s.state, "idle");
    assert_eq!(s.end, None);
    assert!(s.recording);
    let rec2 = Rec::new(true);
    let off = ChatLog::start(rec2, false);
    assert!(!off.snapshot().recording);
    assert!(!off.recording());
}

#[test]
fn single_thread_order_and_strictly_increasing_ids() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    for i in 0..30 {
        log.say(ChatRole::User, &format!("m{i}"));
    }
    settle(&log, &rec);
    let ev = rec.events();
    assert_seq_increasing(&ev);
    let t = texts(&ev);
    for i in 0..30 {
        assert_eq!(t[i], format!("m{i}"));
    }
    assert_eq!(log.snapshot().seq, ev.last().unwrap().seq);
}

#[test]
fn log_keeps_only_the_newest_entries() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    for i in 0..250 {
        log.say(ChatRole::User, &format!("m{i}"));
    }
    settle(&log, &rec);
    let snap = log.snapshot();
    assert_eq!(snap.entries.len(), LOG_CAP);
    assert_eq!(snap.entries.first().unwrap().text, "m51");
    assert_eq!(snap.entries.last().unwrap().text, "system:sentinel");
    let ids: Vec<u64> = snap.entries.iter().map(|e| e.id).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn concurrent_hooks_keep_unique_ids_and_per_thread_order() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for t in 0..8 {
        let log = log.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..25 {
                log.say(ChatRole::User, &format!("t{t}-{i}"));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert!(rec.wait_for(|e| entry_count(e) == 200, 5000));
    let ev = rec.events();
    assert_seq_increasing(&ev);
    for t in 0..8 {
        let mine: Vec<usize> = texts(&ev)
            .iter()
            .filter_map(|x| {
                x.strip_prefix(&format!("t{t}-"))
                    .map(|n| n.parse().unwrap())
            })
            .collect();
        assert_eq!(mine, (0..25).collect::<Vec<_>>(), "thread {t} order");
    }
}

// ---- reset, clear -------------------------------------------------------------------------

#[test]
fn reset_empties_log_and_ids_keep_growing() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "a");
    log.say(ChatRole::Assistant, "b");
    assert!(rec.wait_for(|e| entry_count(e) == 2, 3000));
    log.reset();
    assert!(rec.wait_for(|e| e.iter().any(|x| x.kind == ChatEventKind::Reset), 3000));
    log.say(ChatRole::User, "c");
    assert!(rec.wait_for(|e| has_text(e, "c"), 3000));
    let ev = rec.events();
    assert_seq_increasing(&ev);
    let reset_pos = ev
        .iter()
        .position(|x| x.kind == ChatEventKind::Reset)
        .unwrap();
    let c_pos = ev
        .iter()
        .position(|x| matches!(&x.kind, ChatEventKind::Entry { entry } if entry.text == "c"))
        .unwrap();
    assert!(reset_pos < c_pos);
    let snap = log.snapshot();
    assert_eq!(
        snap.entries
            .iter()
            .map(|e| e.text.clone())
            .collect::<Vec<_>>(),
        vec!["c".to_string()]
    );
}

#[test]
fn clear_is_acknowledged_after_the_log_is_empty() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "private");
    assert!(rec.wait_for(|e| has_text(e, "private"), 3000));
    assert_eq!(log.clear(), Ok(()));
    assert!(log.snapshot().entries.is_empty(), "Ok means already empty");
    assert!(rec.wait_for(|e| e.iter().any(|x| x.kind == ChatEventKind::Clear), 3000));
    assert_seq_increasing(&rec.events());
}

#[test]
fn pending_entries_before_a_reset_never_appear_after_it() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "old1");
    log.say(ChatRole::User, "old2");
    log.reset();
    log.say(ChatRole::User, "new1");
    rec.set_open(true);
    assert!(rec.wait_for(|e| has_text(e, "new1"), 3000));
    let ev = rec.events();
    assert_seq_increasing(&ev);
    if let Some(reset_pos) = ev.iter().position(|x| x.kind == ChatEventKind::Reset) {
        for later in &ev[reset_pos..] {
            if let ChatEventKind::Entry { entry } = &later.kind {
                assert!(!entry.text.starts_with("old"), "old text after reset");
            }
        }
    }
    let snap = log.snapshot();
    assert_eq!(
        snap.entries
            .iter()
            .map(|e| e.text.clone())
            .collect::<Vec<_>>(),
        vec!["new1".to_string()]
    );
}

// ---- recording and state ------------------------------------------------------------------

#[test]
fn recording_off_blocks_new_lines_keeps_old_and_emits_config() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "kept");
    assert!(rec.wait_for(|e| has_text(e, "kept"), 3000));
    log.set_recording(false);
    assert!(!log.recording());
    log.say(ChatRole::User, "not recorded");
    assert!(rec.wait_for(
        |e| e
            .iter()
            .any(|x| x.kind == ChatEventKind::Config { recording: false }),
        3000
    ));
    thread::sleep(Duration::from_millis(100));
    assert!(!has_text(&rec.events(), "not recorded"));
    let snap = log.snapshot();
    assert!(!snap.recording);
    assert!(snap.entries.iter().any(|e| e.text == "kept"));
    log.set_recording(true);
    log.say(ChatRole::User, "again");
    assert!(rec.wait_for(|e| has_text(e, "again"), 3000));
    assert!(log.snapshot().recording);
}

#[test]
fn state_updates_land_and_snapshot_reports_them() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    log.set_state("listening", None);
    assert!(rec.wait_for(
        |e| e
            .iter()
            .any(|x| matches!(&x.kind, ChatEventKind::State { state, .. } if state == "listening")),
        3000
    ));
    log.set_state("idle", Some("user_ended"));
    assert!(rec.wait_for(
        |e| e.iter().any(|x| matches!(&x.kind, ChatEventKind::State { state, end } if state == "idle" && end.as_deref() == Some("user_ended"))),
        3000
    ));
    let s = log.snapshot();
    assert_eq!(s.state, "idle");
    assert_eq!(s.end.as_deref(), Some("user_ended"));
    assert_seq_increasing(&rec.events());
}

// ---- worker wakes from sleep on every kind of input (C10) ---------------------------------

#[test]
fn sleeping_worker_applies_each_single_stimulus_alone() {
    // entry only
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    thread::sleep(Duration::from_millis(50));
    log.say(ChatRole::User, "only-entry");
    assert!(rec.wait_for(|e| has_text(e, "only-entry"), 2000));
    // clear only
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(log.clear(), Ok(()));
    // state only
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    thread::sleep(Duration::from_millis(50));
    log.set_state("idle", Some("idle"));
    assert!(rec.wait_for(
        |e| e
            .iter()
            .any(|x| matches!(x.kind, ChatEventKind::State { .. })),
        2000
    ));
    // config only
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    thread::sleep(Duration::from_millis(50));
    log.set_recording(false);
    assert!(rec.wait_for(
        |e| e
            .iter()
            .any(|x| matches!(x.kind, ChatEventKind::Config { .. })),
        2000
    ));
    // reset only
    let rec = Rec::new(true);
    let log = ChatLog::start(rec.clone(), true);
    thread::sleep(Duration::from_millis(50));
    log.reset();
    assert!(rec.wait_for(|e| e.iter().any(|x| x.kind == ChatEventKind::Reset), 2000));
}

// ---- bounded and non-blocking under a stuck worker (C5, C9) --------------------------------

#[test]
fn hooks_never_wait_for_a_stuck_worker_and_memory_is_bounded() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    let start = Instant::now();
    for i in 0..5000 {
        log.say(ChatRole::User, &format!("e{i}"));
    }
    for _ in 0..10_000 {
        log.set_state("thinking", None);
        log.set_recording(true);
    }
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "hooks blocked on a stuck worker: {:?}",
        start.elapsed()
    );
    rec.set_open(true);
    assert!(
        rec.wait_for(|e| dropped_total(e) > 0, 5000),
        "a system:dropped marker must follow drops"
    );
    settle(&log, &rec);
    let ev = rec.events();
    assert_seq_increasing(&ev);
    let applied = texts(&ev).iter().filter(|t| t.starts_with('e')).count();
    assert!(applied <= 2 * PENDING_CAP, "applied {applied} > bound");
    assert_eq!(
        applied + dropped_total(&ev),
        5000,
        "every line is applied or counted"
    );
}

#[test]
fn reset_flood_coalesces() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "x");
    let start = Instant::now();
    for _ in 0..10_000 {
        log.reset();
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    rec.set_open(true);
    settle(&log, &rec);
    let resets = rec
        .events()
        .iter()
        .filter(|e| e.kind == ChatEventKind::Reset)
        .count();
    assert!(resets >= 1 && resets <= 3, "resets were {resets}");
}

#[test]
fn final_state_and_recording_win_after_saturation() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    for i in 0..1000 {
        log.set_state(if i % 2 == 0 { "listening" } else { "speaking" }, None);
        log.set_recording(i % 2 == 0);
    }
    log.set_state("idle", Some("turn_limit"));
    log.set_recording(false);
    rec.set_open(true);
    assert!(rec.wait_for(
        |e| e.iter().any(|x| matches!(&x.kind, ChatEventKind::State { state, end } if state == "idle" && end.as_deref() == Some("turn_limit"))),
        3000
    ));
    assert!(rec.wait_for(
        |e| matches!(
            e.iter()
                .rev()
                .find(|x| matches!(x.kind, ChatEventKind::Config { .. }))
                .map(|x| &x.kind),
            Some(ChatEventKind::Config { recording: false })
        ),
        3000
    ));
    let s = log.snapshot();
    assert_eq!(s.state, "idle");
    assert_eq!(s.end.as_deref(), Some("turn_limit"));
    assert!(!s.recording);
}

#[test]
fn clear_waiters_are_bounded_and_cleaned_up() {
    let rec = Rec::new(false);
    let cfg = ChatLogConfig {
        clear_timeout: Duration::from_millis(300),
    };
    let log = ChatLog::start_with(rec.clone(), true, cfg);
    log.say(ChatRole::User, "x");
    thread::sleep(Duration::from_millis(100)); // worker now stuck inside emit
    let barrier = Arc::new(Barrier::new(20));
    let mut handles = Vec::new();
    for _ in 0..20 {
        let log = log.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            log.clear()
        }));
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let busy = results
        .iter()
        .filter(|r| **r == Err(ClearError::Busy))
        .count();
    let timeout = results
        .iter()
        .filter(|r| **r == Err(ClearError::Timeout))
        .count();
    assert!(
        results.iter().all(|r| r.is_err()),
        "stuck worker cannot ack"
    );
    assert!(busy >= 20 - CLEAR_WAITERS_CAP, "busy was {busy}");
    assert!(timeout <= CLEAR_WAITERS_CAP, "timeouts were {timeout}");
    // earlier waiters are gone: a new request is admitted (Timeout), not Busy
    assert_eq!(log.clear(), Err(ClearError::Timeout));
    rec.set_open(true);
    assert_eq!(log.clear(), Ok(()));
}

// ---- shutdown (C10 / A13) ----------------------------------------------------------------

#[test]
fn shutdown_with_pending_work_completes_waiters_and_stops_worker() {
    let rec = Rec::new(false);
    let cfg = ChatLogConfig {
        clear_timeout: Duration::from_secs(5),
    };
    let log = ChatLog::start_with(rec.clone(), true, cfg);
    log.say(ChatRole::User, "x");
    thread::sleep(Duration::from_millis(100));
    let waiter = {
        let log = log.clone();
        thread::spawn(move || {
            let t = Instant::now();
            (log.clear(), t.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(100));
    log.shutdown();
    rec.set_open(true);
    let (res, elapsed) = waiter.join().unwrap();
    assert!(
        res == Ok(()) || res == Err(ClearError::Dead),
        "waiter must complete or fail explicitly, got {res:?}"
    );
    assert!(elapsed < Duration::from_secs(3), "waiter hung: {elapsed:?}");
    assert!(log.wait_stopped(Duration::from_secs(2)), "worker must exit");
    let before = rec.events().len();
    log.say(ChatRole::User, "late");
    log.set_state("speaking", None);
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        rec.events().len(),
        before,
        "hooks after shutdown are no-ops"
    );
}

#[test]
fn shutdown_of_an_idle_log_stops_the_worker() {
    let rec = Rec::new(true);
    let log = ChatLog::start(rec, true);
    thread::sleep(Duration::from_millis(50));
    log.shutdown();
    assert!(log.wait_stopped(Duration::from_secs(2)));
    assert_eq!(log.clear(), Err(ClearError::Dead));
}

// ---- panic isolation ----------------------------------------------------------------------

#[test]
fn emitter_panic_does_not_kill_the_worker_or_poison_snapshot() {
    let rec = Rec::new(true);
    *rec.panic_on.lock().unwrap() = Some("boom".to_string());
    let log = ChatLog::start(rec.clone(), true);
    log.say(ChatRole::User, "boom");
    log.say(ChatRole::User, "after");
    assert!(rec.wait_for(|e| has_text(e, "after"), 3000));
    let _ = log.snapshot();
    assert_eq!(log.clear(), Ok(()));
    log.say(ChatRole::User, "still alive");
    assert!(rec.wait_for(|e| has_text(e, "still alive"), 3000));
}

// ---- privacy: no I/O, no logging in the module ---------------------------------------------

#[test]
fn module_source_has_no_io_or_logging() {
    let src = include_str!("chat_log.rs");
    for (n, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        for bad in [
            "std::fs",
            "File::",
            "OpenOptions",
            "log::",
            "tracing::",
            "println!",
            "eprintln!",
            "dbg!",
            "debug!(",
            "info!(",
            "warn!(",
            "error!(",
            "trace!(",
        ] {
            assert!(
                !t.contains(bad),
                "chat_log.rs line {} uses `{bad}`: {line}",
                n + 1
            );
        }
    }
}

// ---- dropped count does not leak across a reset (session boundary) -------------------------------

/// Lines dropped before a reset belong to the old session: after the Reset event no
/// `system:dropped:` marker appears and no dropped count is announced for the new session.
#[test]
fn drops_before_a_reset_are_not_announced_in_the_next_session() {
    let rec = Rec::new(false);
    let log = ChatLog::start(rec.clone(), true);
    // The gate is closed, so the worker cannot release pending lines: at least 10 are dropped.
    for i in 0..PENDING_CAP + 10 {
        log.say(ChatRole::User, &format!("old{i}"));
    }
    log.reset();
    log.say(ChatRole::User, "new1");
    log.say(ChatRole::User, "new2");
    rec.set_open(true);
    assert!(
        rec.wait_for(|e| has_text(e, "new2"), 3000),
        "precondition: the new session's lines were applied"
    );
    settle(&log, &rec);
    let ev = rec.events();
    assert_seq_increasing(&ev);
    let reset_pos = ev
        .iter()
        .position(|x| x.kind == ChatEventKind::Reset)
        .expect("a Reset event follows the reset");
    let after = &ev[reset_pos..];
    assert!(
        !after.iter().any(|e| matches!(
            &e.kind,
            ChatEventKind::Entry { entry } if entry.text.starts_with("system:dropped:")
        )),
        "no dropped marker after the reset: {:?}",
        texts(after)
    );
    assert_eq!(
        dropped_total(after),
        0,
        "dropped count over the events after the reset"
    );
}
