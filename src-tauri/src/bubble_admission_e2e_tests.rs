//! G4b spec: admission end to end with a real ChatLog (DESIGN.md A9-1, A33, A34; Revision 10
//! contract 1) and the total geometry of place_bubbles (A36, Revision 10 contract 4).
//! Deterministic fake time; the only waits are bounded condvar waits for a delivered event.
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.

use crate::bubble_hub::{BubbleRole, HubEntry, Snapshot};
use crate::bubble_shell_core::{BubbleShellCore, Effects, SessionSurface, ShellInputs};
use crate::managers::chat_log::{
    ChatEmitter, ChatEvent, ChatEventKind, ChatLog, ChatRole, GateState,
};
use crate::orb_geometry::{place_bubbles, Rect, Size};
use crate::orb_session::{OrbAction, LINGER_MS};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const T0: u64 = 1_000;
const WORK: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1920.0,
    h: 1080.0,
};
const ORB: Rect = Rect {
    x: 900.0,
    y: 500.0,
    w: 96.0,
    h: 96.0,
};
const FULL: Size = Size { w: 320.0, h: 240.0 };
const MIN: Size = Size { w: 200.0, h: 120.0 };
const GAP: f64 = 8.0;

// ---- an emitter that can hold delivery (the same control the chat-log tests use) ---------------

struct HeldState {
    open: bool,
    events: Vec<ChatEvent>,
}

/// Records events. While closed, `emit` blocks, so the worker cannot deliver anything: an entry
/// is in flight until the test opens the gate again.
struct Held {
    state: Mutex<HeldState>,
    cv: Condvar,
}

impl Held {
    fn new(open: bool) -> Arc<Held> {
        Arc::new(Held {
            state: Mutex::new(HeldState {
                open,
                events: Vec::new(),
            }),
            cv: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, HeldState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_open(&self, open: bool) {
        self.lock().open = open;
        self.cv.notify_all();
    }

    /// Bounded wait for the first Entry event with `text` (a delivery, not a sleep).
    fn wait_for_entry(&self, text: &str) -> Option<ChatEvent> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = self.lock();
        loop {
            if let Some(found) = state.events.iter().find(|e| entry_text(e) == Some(text)) {
                return Some(found.clone());
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let (next, _) = self
                .cv
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner());
            state = next;
        }
    }
}

impl ChatEmitter for Held {
    fn emit(&self, event: &ChatEvent) {
        let mut state = self.lock();
        while !state.open {
            state = self.cv.wait(state).unwrap_or_else(|e| e.into_inner());
        }
        state.events.push(event.clone());
        self.cv.notify_all();
    }
}

fn entry_text(event: &ChatEvent) -> Option<&str> {
    match &event.kind {
        ChatEventKind::Entry { entry } => Some(entry.text.as_str()),
        _ => None,
    }
}

/// The hub's view of a delivered Entry event, carrying the stamp taken when it was said.
fn to_hub(event: &ChatEvent) -> Option<HubEntry> {
    let ChatEventKind::Entry { entry } = &event.kind else {
        return None;
    };
    let role = match entry.role {
        ChatRole::User => BubbleRole::User,
        ChatRole::Assistant => BubbleRole::Assistant,
        ChatRole::System => return None,
    };
    Some(HubEntry {
        id: entry.id,
        role,
        text: entry.text.clone(),
        stamp: event.stamp?,
    })
}

fn start_log(open: bool) -> (Arc<Held>, Arc<ChatLog>) {
    let held = Held::new(open);
    let emitter: Arc<dyn ChatEmitter> = held.clone();
    let chat = ChatLog::start(emitter, true);
    (held, chat)
}

fn shell(now: u64, g: GateState) -> ShellInputs {
    ShellInputs {
        now,
        gate: g,
        orb_persisted_visible: false,
        supported: true,
        classic_visible: false,
        placement_ok: true,
    }
}

fn shows(snapshot: &Option<Snapshot>, text: &str) -> bool {
    snapshot
        .as_ref()
        .is_some_and(|s| s.items.iter().any(|item| item.text == text))
}

fn assert_not_shown(effects: &Effects, text: &str) {
    assert!(
        !shows(&effects.snapshot, text),
        "`{text}` must never be shown"
    );
    if let Some(s) = &effects.snapshot {
        assert!(
            s.items.is_empty(),
            "no bubble may show here, got {:?}",
            s.items
        );
    }
}

// ---- A9-1: a line said under an old tuple never reaches the bubbles ----------------------------

#[test]
fn control_line_without_any_toggle_reaches_the_bubbles() {
    // Positive control: the same path admits a line that is still current, so the refusals below
    // are not vacuous.
    let (held, chat) = start_log(true);
    let token = chat.gate().bump_session();
    let mut core = BubbleShellCore::new(true);
    let instance = core.window_created(T0);
    core.session_started(
        token,
        SessionSurface::OrbBubbles,
        true,
        &shell(T0, chat.gate().state()),
    );
    core.orb_visibility_changed(true, &shell(T0, chat.gate().state()));
    core.page_ready(instance, &shell(T0, chat.gate().state()));
    chat.say(ChatRole::User, "control line");
    let event = held
        .wait_for_entry("control line")
        .expect("the line is delivered");
    let hub_line = to_hub(&event).expect("a user entry");
    let eff = core.chat_entry(hub_line, &shell(T0 + 100, chat.gate().state()));
    assert!(
        shows(&eff.snapshot, "control line"),
        "the current line is admitted"
    );
    chat.shutdown();
}

#[test]
fn line_said_before_a_toggle_never_shows_even_with_late_readiness_and_no_deadline_extension() {
    // A9-1 / Rev 10 contract 1: say -> disable -> enable -> deliver. The line carries the old
    // epoch and is refused with the current gate. A refused line also never extends the
    // machine's deadline (A8-1), so the orb hides at the original deadline.
    let (held, chat) = start_log(false);
    let token = chat.gate().bump_session();
    let mut core = BubbleShellCore::new(true);
    let instance = core.window_created(T0);
    core.session_started(
        token,
        SessionSurface::OrbBubbles,
        true,
        &shell(T0, chat.gate().state()),
    );
    core.orb_visibility_changed(true, &shell(T0, chat.gate().state()));

    let said_under = chat.gate().stamp().expect("recording is on");
    chat.say(ChatRole::User, "race line");
    // Both toggles happen before the worker can deliver the entry (the emitter is held).
    chat.set_recording(false);
    core.recording_changed(false, &shell(T0 + 10, chat.gate().state()));
    chat.set_recording(true);
    core.recording_changed(true, &shell(T0 + 20, chat.gate().state()));

    let end = T0 + 500;
    let deadline = end + LINGER_MS;
    core.session_ended(token, &shell(end, chat.gate().state()));

    held.set_open(true);
    let event = held
        .wait_for_entry("race line")
        .expect("the line is delivered once the emitter opens");
    assert_eq!(
        event.stamp,
        Some(said_under),
        "the delivered line carries the stamp of the moment it was said"
    );

    let late = core.chat_entry(
        to_hub(&event).expect("a user entry"),
        &shell(deadline - 500, chat.gate().state()),
    );
    assert_not_shown(&late, "race line");

    // Readiness arrives late: the page still must not show the line.
    let ready = core.page_ready(instance, &shell(deadline - 400, chat.gate().state()));
    assert!(
        !shows(&ready, "race line"),
        "late readiness never shows the line"
    );

    // The refused line did not move the deadline: the orb hides exactly at the original one.
    assert_eq!(
        core.tick(&shell(deadline - 1, chat.gate().state())).orb,
        None,
        "not due before the original deadline"
    );
    assert_eq!(
        core.tick(&shell(deadline, chat.gate().state())).orb,
        Some(OrbAction::Hide),
        "A8-1: a refused line never extends the deadline"
    );
    chat.shutdown();
}

#[test]
fn line_said_before_a_session_token_bump_is_refused_after_it() {
    // A9-1 / A8-1: a session bump (bump_session) between say and delivery refuses the line.
    let (held, chat) = start_log(false);
    let token = chat.gate().bump_session();
    let mut core = BubbleShellCore::new(true);
    let instance = core.window_created(T0);
    core.session_started(
        token,
        SessionSurface::OrbBubbles,
        true,
        &shell(T0, chat.gate().state()),
    );
    core.orb_visibility_changed(true, &shell(T0, chat.gate().state()));
    chat.say(ChatRole::User, "old session line");
    let next = chat.gate().bump_session();
    assert_ne!(next, token, "the session token moved on");
    held.set_open(true);
    let event = held
        .wait_for_entry("old session line")
        .expect("the line is delivered");
    let eff = core.chat_entry(
        to_hub(&event).expect("a user entry"),
        &shell(T0 + 100, chat.gate().state()),
    );
    assert_not_shown(&eff, "old session line");
    let ready = core.page_ready(instance, &shell(T0 + 200, chat.gate().state()));
    assert!(!shows(&ready, "old session line"));
    chat.shutdown();
}

// ---- A36 / Rev 10 contract 4: place_bubbles is total (non-finite input gives None) -------------

#[test]
fn place_bubbles_finite_geometry_places_a_stack() {
    // Control: valid inputs place a stack, so the None results below mean something.
    assert!(place_bubbles(ORB, WORK, FULL, GAP, MIN).is_some());
}

#[test]
fn place_bubbles_nan_orb_returns_none() {
    // A36: a NaN orb rectangle has no placement. Predicted to FAIL against the merged code: its
    // sanitize step turns NaN into 0 and the Below side then fits (cargo not run; see NOTES).
    let orb = Rect {
        x: f64::NAN,
        y: f64::NAN,
        w: f64::NAN,
        h: f64::NAN,
    };
    assert_eq!(place_bubbles(orb, WORK, FULL, GAP, MIN), None);
}

#[test]
fn place_bubbles_nan_orb_coordinate_returns_none() {
    // A36: one NaN coordinate of the orb. Predicted to FAIL against the merged code (NaN -> 0).
    let orb = Rect { x: f64::NAN, ..ORB };
    assert_eq!(place_bubbles(orb, WORK, FULL, GAP, MIN), None);
}

#[test]
fn place_bubbles_infinite_work_area_returns_none() {
    // A36: an infinite work area is not a usable area.
    let work = Rect {
        w: f64::INFINITY,
        h: f64::INFINITY,
        ..WORK
    };
    assert_eq!(place_bubbles(ORB, work, FULL, GAP, MIN), None);
}

#[test]
fn place_bubbles_nan_size_returns_none() {
    // A36: a NaN bubble size cannot be placed.
    let size = Size {
        w: f64::NAN,
        h: f64::NAN,
    };
    assert_eq!(place_bubbles(ORB, WORK, size, GAP, MIN), None);
}

#[test]
fn place_bubbles_infinite_size_returns_none() {
    // A36: an infinite bubble size cannot be placed.
    let size = Size {
        w: f64::INFINITY,
        h: f64::INFINITY,
    };
    assert_eq!(place_bubbles(ORB, WORK, size, GAP, MIN), None);
}

#[test]
fn place_bubbles_negative_size_returns_none() {
    // A36: a negative bubble size cannot be placed.
    let size = Size {
        w: -320.0,
        h: -240.0,
    };
    assert_eq!(place_bubbles(ORB, WORK, size, GAP, MIN), None);
}

#[test]
fn place_bubbles_nan_gap_returns_none() {
    // A36: a NaN gap is not a position. Predicted to FAIL against the merged code (NaN -> 0).
    assert_eq!(place_bubbles(ORB, WORK, FULL, f64::NAN, MIN), None);
}

#[test]
fn place_bubbles_infinite_gap_returns_none() {
    // A36: an infinite gap is not a position. Predicted to FAIL against the merged code (inf -> 0).
    assert_eq!(place_bubbles(ORB, WORK, FULL, f64::INFINITY, MIN), None);
}

#[test]
fn place_bubbles_nan_minimum_returns_none() {
    // A36: a NaN minimum cannot be checked against. Predicted to FAIL against the merged code
    // (NaN -> 0 makes every side pass the minimum).
    let min = Size {
        w: f64::NAN,
        h: f64::NAN,
    };
    assert_eq!(place_bubbles(ORB, WORK, FULL, GAP, min), None);
}
