//! G4b validator round 1 fixes (task G4b.fix). Authoritative: the existing specs are not edited.
//!
//! MAJOR 1: ORB_TRANSIENT holds only the session's own share (`orb_session::transient_share`);
//!          the saved setting is never stored as transient, and the apply condition is
//!          `orb_native::orb_wanted(persisted, share, supported)`.
//! MAJOR 2: the bubble executor only shows or hides the orb (`orb_native::show_hide_only`) and
//!          re-targets its geometry only when `orb_native::executor_needs_geometry` says so.
//! MAJOR 3: a session start is recorded synchronously (`session_begin`); an end that arrives
//!          before the start (`last_ended_sid`) makes the start Lingering, and the windows are
//!          reported afterwards through `windows_ready`.
//! M1:      a fallback destroys the bubble window (`destroy_bubbles`: close, then `reset`), so the next session gets a
//!          new instance that is accepted.
//! M4:      the bubble window is forced topmost on Windows when shown.

use crate::bubble_hub::{BubbleRole, HubEntry, Snapshot};
use crate::bubble_shell_core::{BubbleShellCore, SessionSurface, ShellInputs, READY_TIMEOUT_MS};
use crate::managers::chat_log::{GateState, Stamp};
use crate::orb_native::{executor_needs_geometry, orb_wanted};
use crate::orb_session::{transient_share, OrbAction, Phase, SessionMachine, LINGER_MS};

const SID: u64 = 7;
const T0: u64 = 1_000;

fn gate(token: u64, epoch: u64, recording: bool) -> GateState {
    GateState {
        session_token: token,
        rec_epoch: epoch,
        recording,
    }
}

fn stamp(token: u64, epoch: u64) -> Stamp {
    Stamp {
        session_token: token,
        rec_epoch: epoch,
    }
}

/// Inputs with the saved orb setting off, the orb supported and placement available.
fn inp(now: u64, g: GateState) -> ShellInputs {
    inp_persisted(now, g, false)
}

fn inp_persisted(now: u64, g: GateState, persisted: bool) -> ShellInputs {
    ShellInputs {
        now,
        gate: g,
        orb_persisted_visible: persisted,
        supported: true,
        classic_visible: false,
        placement_ok: true,
    }
}

fn line(id: u64, role: BubbleRole, text: &str, s: Stamp) -> HubEntry {
    HubEntry {
        id,
        role,
        text: text.to_string(),
        stamp: s,
    }
}

fn shows(snapshot: &Option<Snapshot>, text: &str) -> bool {
    snapshot
        .as_ref()
        .is_some_and(|s| s.items.iter().any(|item| item.text == text))
}

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the item whose signature starts with `signature`, up to the closing brace at the same
/// indentation as the signature line. Panics when the signature is missing.
fn fn_body<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..start];
    let closing = format!("\n{indent}}}\n");
    let rest = &text[start..];
    let end = rest
        .find(closing.as_str())
        .map(|i| i + closing.len())
        .unwrap_or(rest.len());
    &rest[..end]
}

// ---- MAJOR 1: the session's own share of the orb --------------------------------------------

#[test]
fn transient_share_is_only_the_sessions_own_share() {
    let idle = SessionMachine::new();
    assert!(!transient_share(&idle), "Idle owns no share");

    let mut active = SessionMachine::new();
    active.started_as(SID, T0, true);
    assert!(
        transient_share(&active),
        "an Active transient session owns a share"
    );

    let mut persisted = SessionMachine::new();
    persisted.started_as(SID, T0, false);
    assert!(
        !transient_share(&persisted),
        "a persisted-visible start owns no share"
    );

    let mut pinned = SessionMachine::new();
    pinned.started_as(SID, T0, true);
    pinned.pin();
    assert!(
        !transient_share(&pinned),
        "a pinned orb is never the session's"
    );

    let mut hidden = SessionMachine::new();
    hidden.started_as(SID, T0, true);
    hidden.user_hid();
    assert!(!transient_share(&hidden), "a tray hide removes the share");

    let mut lingering = SessionMachine::new();
    lingering.started_as(SID, T0, true);
    lingering.ended(SID, T0 + 100);
    assert!(
        transient_share(&lingering),
        "a lingering session keeps its share"
    );

    lingering.tick(T0 + 100 + LINGER_MS);
    assert!(
        !transient_share(&lingering),
        "after expiry the share is gone"
    );
}

#[test]
fn orb_wanted_is_the_settings_or_the_share_on_a_supported_platform() {
    assert!(!orb_wanted(false, false, true));
    assert!(orb_wanted(true, false, true));
    assert!(orb_wanted(false, true, true));
    assert!(!orb_wanted(true, true, false), "unsupported never shows");
    assert!(!orb_wanted(false, true, false), "unsupported never shows");
}

#[test]
fn turning_the_saved_orb_off_after_a_session_started_on_hides_it() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let on = inp_persisted(T0, g, true);
    core.session_begin(SID, SessionSurface::OrbBubbles, &on);
    core.windows_ready(SID, true, &on);
    assert!(
        !core.orb_share(),
        "a persisted-visible session must not store the total as its share"
    );
    core.orb_visibility_changed(true, &on);
    core.session_ended(SID, &inp_persisted(T0 + 1_000, g, true));
    core.tick(&inp_persisted(T0 + 1_000 + LINGER_MS, g, true));
    assert!(!core.orb_share(), "after expiry the share is gone");

    // The saved setting is turned off: the stale share must not keep the orb up.
    assert!(
        !orb_wanted(false, core.orb_share(), true),
        "orb_wanted must be false once the setting is off and the session is over"
    );
}

#[test]
fn bubble_native_never_stores_the_total_desired_visibility_as_transient() {
    let text = src("src/bubble_native.rs");
    assert!(
        !text.contains("set_orb_transient(desired"),
        "the total desired visibility must not be stored in ORB_TRANSIENT"
    );
    assert!(text.contains("set_orb_transient(share"));
}

#[test]
fn apply_on_main_decides_with_orb_wanted() {
    let orb = src("src/orb_native.rs");
    let body = fn_body(&orb, "pub(crate) fn apply_on_main(");
    assert!(
        body.contains("orb_wanted("),
        "apply_on_main must use orb_wanted"
    );
}

// ---- MAJOR 2: a drag is never fought by the bubble executor ----------------------------------

#[test]
fn executor_needs_geometry_table() {
    assert!(
        executor_needs_geometry(false, false, false),
        "no window: build and place"
    );
    assert!(
        executor_needs_geometry(false, true, true),
        "no window: build and place"
    );
    assert!(
        executor_needs_geometry(true, false, false),
        "hidden and idle: place then show"
    );
    assert!(
        !executor_needs_geometry(true, false, true),
        "hidden during a gesture: keep place"
    );
    assert!(
        !executor_needs_geometry(true, true, false),
        "shown: keep place"
    );
    assert!(
        !executor_needs_geometry(true, true, true),
        "shown during a gesture: keep place"
    );
}

#[test]
fn bubble_executor_body_never_retargets_the_orb() {
    let text = src("src/bubble_native.rs");
    let body = fn_body(&text, "fn execute_on_main(");
    for banned in ["apply_geometry(", "target_rect(", "apply_on_main("] {
        assert!(
            !body.contains(banned),
            "the executor must not call {banned}"
        );
    }
    assert!(
        body.contains("show_hide_only("),
        "the executor shows or hides only"
    );
}

#[test]
fn orb_moves_never_reach_apply_geometry_through_the_bubble_path() {
    let bubble = src("src/bubble_native.rs");
    for banned in ["apply_geometry(", "target_rect(", "apply_on_main("] {
        assert!(
            !bubble.contains(banned),
            "bubble_native must not contain {banned}"
        );
    }
    let changed = fn_body(&bubble, "pub fn orb_geometry_changed(");
    assert!(
        !changed.contains("apply_rect("),
        "an orb move only schedules the executor"
    );

    let orb = src("src/orb_native.rs");
    let geometry = fn_body(&orb, "fn on_geometry_event(");
    assert!(geometry.contains("bubble_native::orb_geometry_changed("));

    let show_hide = fn_body(&orb, "pub(crate) fn show_hide_only(");
    let gate_at = show_hide
        .find("executor_needs_geometry(")
        .expect("show_hide_only must gate geometry");
    let place_at = show_hide
        .find("apply_geometry(")
        .expect("show_hide_only places when needed");
    assert!(gate_at < place_at, "geometry is placed only after the gate");
}

// ---- MAJOR 3: a session that ends before its windows are ready --------------------------------

#[test]
fn an_end_that_arrives_before_the_start_still_ends_the_session() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_ended(SID, &inp(T0, g));
    let started = core.session_begin(SID, SessionSurface::OrbBubbles, &inp(T0 + 10, g));
    core.windows_ready(SID, true, &inp(T0 + 10, g));
    assert_eq!(
        started.orb,
        Some(OrbAction::Show),
        "the orb shows at the wake"
    );
    assert_eq!(
        core.phase(),
        Phase::Lingering,
        "a late start of an ended sid lingers"
    );

    core.orb_visibility_changed(true, &inp(T0 + 10, g));
    let early = core.tick(&inp(T0 + 10 + LINGER_MS - 1, g));
    assert_ne!(
        early.orb,
        Some(OrbAction::Hide),
        "not hidden before the deadline"
    );
    let due = core.tick(&inp(T0 + 10 + LINGER_MS, g));
    assert_eq!(due.orb, Some(OrbAction::Hide), "hidden at the deadline");
    assert!(core.is_idle());
}

#[test]
fn a_session_that_ends_before_its_windows_exist_is_idle_eight_seconds_later() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let started = core.session_begin(SID, SessionSurface::OrbBubbles, &inp(T0, g));
    assert_eq!(started.orb, Some(OrbAction::Show));
    core.orb_visibility_changed(true, &inp(T0, g));
    let ended = core.session_ended(SID, &inp(T0 + 100, g));
    assert!(!ended.open_classic);

    let almost = core.tick(&inp(T0 + 100 + LINGER_MS - 1, g));
    assert!(!almost.open_classic);
    assert!(!core.is_idle(), "still lingering just before the deadline");

    let done = core.tick(&inp(T0 + 100 + LINGER_MS, g));
    assert_eq!(done.orb, Some(OrbAction::Hide));
    assert!(core.is_idle());
    assert!(!done.open_classic, "no fallback without a waiting line");
}

#[test]
fn the_first_line_said_right_after_the_start_is_admitted() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_begin(SID, SessionSurface::OrbBubbles, &inp(T0, g));
    let instance = core.window_created(T0 + 1);
    core.windows_ready(SID, true, &inp(T0 + 1, g));
    core.orb_visibility_changed(true, &inp(T0 + 1, g));
    core.chat_entry(
        line(1, BubbleRole::User, "first line", stamp(SID, 0)),
        &inp(T0 + 2, g),
    );
    let ready = core.page_ready(instance, &inp(T0 + 3, g));
    assert!(
        shows(&ready, "first line"),
        "the first line is admitted and reaches the bubbles"
    );
}

#[test]
fn a_failed_window_build_resets_the_session_and_opens_classic_once() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_begin(SID, SessionSurface::OrbBubbles, &inp(T0, g));
    core.orb_visibility_changed(true, &inp(T0, g));
    let failed = core.windows_ready(SID, false, &inp(T0 + 5, g));
    assert!(
        failed.open_classic,
        "the classic window opens for this session"
    );
    assert_eq!(
        failed.orb,
        Some(OrbAction::Hide),
        "the orb does not stay up"
    );
    assert!(core.is_idle());
    let again = core.windows_ready(SID, false, &inp(T0 + 6, g));
    assert!(!again.open_classic, "exactly once per session");
}

// ---- M1: a fallback destroys the bubble window; the next session gets a new instance ----------

#[test]
fn after_a_ready_timeout_the_next_session_gets_a_new_instance_that_is_accepted() {
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let first_instance = core.window_created(T0);
    core.session_begin(SID, SessionSurface::OrbBubbles, &inp(T0, g));
    core.windows_ready(SID, true, &inp(T0, g));
    core.chat_entry(
        line(1, BubbleRole::User, "waiting", stamp(SID, 0)),
        &inp(T0 + 10, g),
    );
    let timed_out = core.tick(&inp(T0 + READY_TIMEOUT_MS + 10, g));
    assert!(
        timed_out.open_classic,
        "setup: the ready timeout falls back"
    );

    let destroyed = core.reset(&inp(T0 + READY_TIMEOUT_MS + 20, g));
    assert!(
        !destroyed.open_classic,
        "destroying the window opens nothing"
    );

    let sid2 = SID + 1;
    let g2 = gate(sid2, 0, true);
    let second = core.session_begin(sid2, SessionSurface::OrbBubbles, &inp(T0 + 3_000, g2));
    assert!(!second.open_classic);
    let new_instance = core.window_created(T0 + 3_000);
    assert_ne!(new_instance, first_instance, "a new window instance");
    core.windows_ready(sid2, true, &inp(T0 + 3_000, g2));
    assert!(
        core.page_ready(new_instance, &inp(T0 + 3_001, g2))
            .is_some(),
        "the new instance's ready is accepted"
    );
    assert!(
        !core.tick(&inp(T0 + 3_500, g2)).open_classic,
        "session 2 does not fall back"
    );
}

// ---- M4: Windows forces the bubble window topmost when it is shown ----------------------------

#[test]
fn bubble_window_is_forced_topmost_on_windows_when_shown() {
    let text = src("src/bubble_native.rs");
    assert!(
        text.contains(
            "#[cfg(target_os = \"windows\")]\n        crate::overlay::force_overlay_topmost(&window);"
        ),
        "show_bubble_window must call force_overlay_topmost on Windows"
    );
}
