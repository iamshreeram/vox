//! G4b spec: pure shell core that wires the wake-appear orb and the fading bubbles (DESIGN.md
//! A20-A23, A25, A26, A30-A35, A38 and the Revision 10 contracts). Time, the chat-log gate and the
//! window facts are inputs, so every test runs on a fake clock with no sleeps.
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::bubble_shell_core`, new, no Tauri types, no I/O):
//!   enum SessionSurface { OrbBubbles, Window }     (Debug, Clone, Copy, PartialEq, Eq)
//!   struct ShellInputs { now, gate: GateState, orb_persisted_visible, supported, classic_visible,
//!     placement_ok }
//!   struct Effects { orb: Option<OrbAction>, bubbles_visible: Option<bool>,
//!     snapshot: Option<Snapshot>, open_classic: bool }                         (Default)
//!   const READY_TIMEOUT_MS: u64 = 2000;
//!   struct BubbleShellCore: new(recording), window_created(now) -> u64,
//!     session_started(sid, surface, windows_ok, &inp) -> Effects, session_ended(sid, &inp),
//!     chat_entry(HubEntry, &inp), chat_reset(&inp), chat_clear(&inp), recording_changed(on, &inp),
//!     page_ready(instance, &inp) -> Option<Snapshot>, orb_visibility_changed(shown, &inp),
//!     classic_visibility_changed(visible, &inp), user_hid_orb(&inp), user_showed_orb(&inp),
//!     tick(&inp)                                   (Effects unless noted)
//!   fn session_surface(Option<Presentation>, &PlatformEnv) -> SessionSurface
//!   Inputs are taken by reference (assumption, see report NOTES).

use crate::bubble_hub::{BubbleRole, HubEntry, Snapshot};
use crate::bubble_shell_core::{
    session_surface, BubbleShellCore, Effects, SessionSurface, ShellInputs, READY_TIMEOUT_MS,
};
use crate::chat_presentation::{LinuxBackend, Os, PlatformEnv, Presentation};
use crate::managers::chat_log::{GateState, Stamp};
use crate::orb_session::{OrbAction, LINGER_MS};

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

/// Inputs with the classic window hidden, the orb supported and placement available.
fn inp(now: u64, g: GateState) -> ShellInputs {
    ShellInputs {
        now,
        gate: g,
        orb_persisted_visible: false,
        supported: true,
        classic_visible: false,
        placement_ok: true,
    }
}

fn inp_classic(now: u64, g: GateState, classic: bool) -> ShellInputs {
    ShellInputs {
        classic_visible: classic,
        ..inp(now, g)
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

/// No snapshot published by this call may carry a bubble.
fn assert_no_items(effects: &Effects) {
    if let Some(s) = &effects.snapshot {
        assert!(
            s.items.is_empty(),
            "no bubble may show here, got {:?}",
            s.items
        );
    }
}

/// A session whose orb is shown, whose page is ready and whose gates are open (A20, A25).
fn open_session() -> (BubbleShellCore, u64) {
    let mut core = BubbleShellCore::new(true);
    let instance = core.window_created(T0);
    let g = gate(SID, 0, true);
    let started = core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    assert_eq!(
        started.orb,
        Some(OrbAction::Show),
        "setup: the wake shows the orb"
    );
    core.orb_visibility_changed(true, &inp(T0, g));
    core.page_ready(instance, &inp(T0, g));
    (core, instance)
}

fn env(os: Os, backend: LinuxBackend, composited: bool) -> PlatformEnv {
    PlatformEnv {
        os,
        linux_backend: backend,
        composited,
    }
}

// ---- A20: wake shows the orb, never the classic window ----------------------------------------

#[test]
fn wake_shows_a_hidden_orb_and_never_opens_the_classic_window() {
    // A20: SessionStarted shows a hidden orb; the conversation window does not open.
    let mut core = BubbleShellCore::new(true);
    let eff = core.session_started(
        SID,
        SessionSurface::OrbBubbles,
        true,
        &inp(T0, gate(SID, 0, true)),
    );
    assert_eq!(eff.orb, Some(OrbAction::Show));
    assert!(
        !eff.open_classic,
        "A20: the classic window must not open on wake"
    );
}

#[test]
fn window_surface_changes_nothing_and_never_arms_the_hub() {
    // The existing dispatcher opens the classic window for the Window surface; the core is inert.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let eff = core.session_started(SID, SessionSurface::Window, true, &inp(T0, g));
    assert_eq!(eff.orb, None);
    assert_eq!(eff.bubbles_visible, None);
    assert!(eff.snapshot.is_none());
    assert!(!eff.open_classic);
    // Disarmed: a line offered in this session is refused.
    let offered = core.chat_entry(
        line(1, BubbleRole::User, "hi", stamp(SID, 0)),
        &inp(T0 + 10, g),
    );
    assert_no_items(&offered);
}

// ---- A21 / A22 / A23: orb lifetime --------------------------------------------------------------

#[test]
fn orb_auto_hides_8s_after_the_last_activity_when_transient() {
    // A21: an entry at end-time is the last activity; the orb hides at the deadline, not before.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    core.orb_visibility_changed(true, &inp(T0, g));
    let last = T0 + 3_000;
    core.chat_entry(
        line(1, BubbleRole::User, "hi", stamp(SID, 0)),
        &inp(last, g),
    );
    let ended = core.session_ended(SID, &inp(last, g));
    assert_eq!(ended.orb, None, "lingering keeps the orb");
    let before = core.tick(&inp(last + LINGER_MS - 1, g));
    assert_eq!(
        before.orb, None,
        "no hide before 8 s after the last activity"
    );
    let at = core.tick(&inp(last + LINGER_MS, g));
    assert_eq!(
        at.orb,
        Some(OrbAction::Hide),
        "A21: transient orb hides at the deadline"
    );
}

#[test]
fn persisted_visible_orb_is_never_hidden() {
    // A22: a persisted-visible orb is never hidden by the session.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let mut persisted = inp(T0, g);
    persisted.orb_persisted_visible = true;
    core.orb_visibility_changed(true, &persisted);
    let started = core.session_started(SID, SessionSurface::OrbBubbles, true, &persisted);
    assert_eq!(
        started.orb, None,
        "already visible and wanted: nothing to do"
    );
    let ended = core.session_ended(SID, &persisted);
    assert_eq!(ended.orb, None);
    let after_deadline = core.tick(&ShellInputs {
        now: T0 + 1_000 + LINGER_MS,
        ..persisted
    });
    assert_eq!(
        after_deadline.orb, None,
        "A22: a persisted-visible orb is never hidden"
    );
    let much_later = core.tick(&ShellInputs {
        now: T0 + 600_000,
        ..persisted
    });
    assert_eq!(much_later.orb, None);
}

#[test]
fn second_session_during_linger_cancels_the_hide() {
    // A23: a wake during Lingering cancels the pending hide; no Hide at the old deadline.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    core.orb_visibility_changed(true, &inp(T0, g));
    core.session_ended(SID, &inp(T0 + 1_000, g));
    let second = core.session_started(
        SID + 1,
        SessionSurface::OrbBubbles,
        true,
        &inp(T0 + 5_000, gate(SID + 1, 0, true)),
    );
    assert_eq!(
        second.orb, None,
        "A23: the orb is still visible: no flicker"
    );
    let old_deadline = core.tick(&inp(T0 + 1_000 + LINGER_MS, gate(SID + 1, 0, true)));
    assert_eq!(
        old_deadline.orb, None,
        "A23: no Hide at the cancelled deadline"
    );
    let active_later = core.tick(&inp(T0 + 60_000, gate(SID + 1, 0, true)));
    assert_eq!(active_later.orb, None, "the second session is still active");
}

#[test]
fn typed_session_without_entries_lingers_8s_then_hides() {
    // Typed sessions start and end without any transcript entry; the same linger applies.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let started = core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    assert_eq!(started.orb, Some(OrbAction::Show));
    core.orb_visibility_changed(true, &inp(T0, g));
    let end = T0 + 500;
    core.session_ended(SID, &inp(end, g));
    assert_eq!(core.tick(&inp(end + LINGER_MS - 1, g)).orb, None);
    assert_eq!(
        core.tick(&inp(end + LINGER_MS, g)).orb,
        Some(OrbAction::Hide)
    );
}

#[test]
fn late_entries_extend_the_linger_but_never_past_30s() {
    // A31: a late entry after End keeps the orb, capped at ended + 30 s.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    core.orb_visibility_changed(true, &inp(T0, g));
    let ended = T0 + 1_000;
    core.session_ended(SID, &inp(ended, g));
    // Each late entry moves the deadline to now + 8 s; the first keeps it at 15 s.
    let late = [7_000u64, 14_000, 21_000, 28_000];
    for (i, offset) in late.iter().enumerate() {
        core.chat_entry(
            line(i as u64 + 1, BubbleRole::Assistant, "late", stamp(SID, 0)),
            &inp(T0 + offset, g),
        );
    }
    // Deadline = min(28 s + 8 s, ended + 30 s) = ended + 30 s = T0 + 31 s.
    assert_eq!(
        core.tick(&inp(T0 + 30_999, g)).orb,
        None,
        "A31: capped, not yet due"
    );
    assert_eq!(
        core.tick(&inp(T0 + 31_000, g)).orb,
        Some(OrbAction::Hide),
        "A31: late entries cannot strand the orb"
    );
}

// ---- A25: bubbles flow in role order, at most three ---------------------------------------------

#[test]
fn user_and_assistant_bubbles_flow_in_order_and_at_most_three() {
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    let roles = [
        BubbleRole::User,
        BubbleRole::Assistant,
        BubbleRole::User,
        BubbleRole::Assistant,
    ];
    let mut last_rev = None;
    for (i, role) in roles.iter().enumerate() {
        let id = i as u64 + 1;
        let text = format!("line {id}");
        let eff = core.chat_entry(
            line(id, *role, &text, stamp(SID, 0)),
            &inp(T0 + 100 * id, g),
        );
        let snap = eff
            .snapshot
            .expect("A25: every admitted line changes the projection");
        if let Some(prev) = last_rev {
            assert!(snap.rev > prev, "revisions grow");
        }
        last_rev = Some(snap.rev);
        assert!(snap.items.len() <= 3, "A25: at most three bubbles");
        if id == 4 {
            // Oldest dropped: lines 2, 3, 4 remain, oldest first, roles in order.
            let ids: Vec<u64> = snap.items.iter().map(|i| i.id).collect();
            assert_eq!(ids, vec![2, 3, 4]);
            let got_roles: Vec<BubbleRole> = snap.items.iter().map(|i| i.role).collect();
            assert_eq!(
                got_roles,
                vec![
                    BubbleRole::Assistant,
                    BubbleRole::User,
                    BubbleRole::Assistant
                ]
            );
            assert_eq!(snap.items[0].text, "line 2");
        }
    }
}

// ---- A34: recording gate ------------------------------------------------------------------------

#[test]
fn recording_off_at_start_never_shows_and_re_enabling_never_restores() {
    // A34 / A26: nothing said while off is ever shown, and turning recording on never restores it.
    let mut core = BubbleShellCore::new(false);
    let instance = core.window_created(T0);
    let off = gate(SID, 0, false);
    core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, off));
    core.orb_visibility_changed(true, &inp(T0, off));
    let ready = core.page_ready(instance, &inp(T0, off));
    if let Some(s) = &ready {
        assert!(s.items.is_empty(), "A26: no bubble with recording off");
    }
    let refused = core.chat_entry(
        line(1, BubbleRole::User, "said while off", stamp(SID, 0)),
        &inp(T0 + 10, off),
    );
    assert_no_items(&refused);
    let on = gate(SID, 1, true);
    assert_no_items(&core.recording_changed(true, &inp(T0 + 20, on)));
    let fresh = core.chat_entry(
        line(2, BubbleRole::Assistant, "after on", stamp(SID, 1)),
        &inp(T0 + 30, on),
    );
    let snap = fresh
        .snapshot
        .expect("a line said after re-enabling is shown");
    assert!(shows(&Some(snap.clone()), "after on"));
    assert!(
        !snap.items.iter().any(|i| i.text == "said while off"),
        "A34: earlier text never comes back"
    );
    let later = core.page_ready(instance, &inp(T0 + 40, on));
    if let Some(s) = &later {
        assert!(!s.items.iter().any(|i| i.text == "said while off"));
    }
}

#[test]
fn recording_off_publishes_an_empty_snapshot_and_clears() {
    // A34: turning recording off clears the stack at once and publishes that.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    let kept = core.chat_entry(
        line(1, BubbleRole::User, "kept", stamp(SID, 0)),
        &inp(T0 + 100, g),
    );
    assert!(shows(&kept.snapshot, "kept"));
    let off = core.recording_changed(false, &inp(T0 + 200, gate(SID, 1, false)));
    let snap = off.snapshot.expect("A34: clearing is published");
    assert!(snap.items.is_empty());
    assert_eq!(off.bubbles_visible, Some(false));
}

// ---- A30 / A33 / A35 / A9-1 stamps ----------------------------------------------------------------

#[test]
fn stale_session_token_is_rejected_without_touching_the_deadline() {
    // A9-1: an entry stamped with an older session token is refused and does not move the
    // deadline of the newer session.
    let (mut core, _instance) = open_session();
    let b = SID + 1;
    core.session_started(
        b,
        SessionSurface::OrbBubbles,
        true,
        &inp(T0 + 200, gate(b, 0, true)),
    );
    core.session_ended(b, &inp(T0 + 1_000, gate(b, 0, true)));
    let stale = core.chat_entry(
        line(5, BubbleRole::User, "stale", stamp(SID, 0)),
        &inp(T0 + 8_000, gate(b, 0, true)),
    );
    assert_no_items(&stale);
    assert_eq!(
        core.tick(&inp(T0 + 1_000 + LINGER_MS - 1, gate(b, 0, true)))
            .orb,
        None
    );
    assert_eq!(
        core.tick(&inp(T0 + 1_000 + LINGER_MS, gate(b, 0, true)))
            .orb,
        Some(OrbAction::Hide),
        "A9-1: the stale entry did not extend the deadline"
    );
}

#[test]
fn older_rec_epoch_is_rejected_without_touching_the_deadline() {
    // A9-1 / A8-1: a line stamped before a recording toggle is refused after it, and the toggle
    // does not extend the machine's deadline.
    let (mut core, _instance) = open_session();
    let g0 = gate(SID, 0, true);
    core.session_ended(SID, &inp(T0 + 1_000, g0));
    core.recording_changed(false, &inp(T0 + 2_000, gate(SID, 1, false)));
    core.recording_changed(true, &inp(T0 + 3_000, gate(SID, 2, true)));
    let stale = core.chat_entry(
        line(3, BubbleRole::User, "old epoch", stamp(SID, 0)),
        &inp(T0 + 8_000, gate(SID, 2, true)),
    );
    assert_no_items(&stale);
    assert_eq!(
        core.tick(&inp(T0 + 1_000 + LINGER_MS - 1, gate(SID, 2, true)))
            .orb,
        None
    );
    assert_eq!(
        core.tick(&inp(T0 + 1_000 + LINGER_MS, gate(SID, 2, true)))
            .orb,
        Some(OrbAction::Hide)
    );
}

// ---- A8-2 / A35: classic window and orb suppression -----------------------------------------------

#[test]
fn classic_window_suppresses_bubbles_and_reappearing_is_a_newer_revision() {
    // A8-2: suppression publishes an empty snapshot; closing before expiry publishes a NEWER
    // populated snapshot.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    let shown = core.chat_entry(
        line(1, BubbleRole::User, "hello", stamp(SID, 0)),
        &inp(T0 + 100, g),
    );
    let before = shown.snapshot.expect("populated before suppression");
    assert!(shows(&Some(before.clone()), "hello"));

    let sup = core.classic_visibility_changed(true, &inp_classic(T0 + 1_000, g, true));
    assert_eq!(sup.bubbles_visible, Some(false));
    let suppressed = sup.snapshot.expect("A8-2: suppression is published");
    assert!(suppressed.items.is_empty());
    assert!(suppressed.rev > before.rev);

    let during = core.page_ready(_instance, &inp_classic(T0 + 1_100, g, true));
    if let Some(s) = &during {
        assert!(
            s.items.is_empty(),
            "A8-2: readiness during suppression gets empty"
        );
    }

    let back = core.classic_visibility_changed(false, &inp_classic(T0 + 2_000, g, false));
    assert_eq!(back.bubbles_visible, Some(true));
    let restored = back.snapshot.expect("A8-2: restoration is published");
    assert_eq!(restored.items.len(), 1);
    assert_eq!(restored.items[0].text, "hello");
    assert!(
        restored.rev > suppressed.rev,
        "A8-2: restoration is a newer revision"
    );
}

#[test]
fn orb_going_hidden_clears_bubbles() {
    // A35: bubbles are hidden whenever the orb is hidden.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    core.chat_entry(
        line(1, BubbleRole::User, "visible", stamp(SID, 0)),
        &inp(T0 + 100, g),
    );
    let hidden = core.orb_visibility_changed(false, &inp(T0 + 200, g));
    assert_eq!(hidden.bubbles_visible, Some(false));
    let snap = hidden
        .snapshot
        .expect("A35: the empty projection is published");
    assert!(snap.items.is_empty());
}

#[test]
fn orb_hidden_by_the_user_clears_bubbles() {
    // A35 / A9-2: a tray hide clears the bubbles at once.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    core.chat_entry(
        line(1, BubbleRole::User, "gone soon", stamp(SID, 0)),
        &inp(T0 + 100, g),
    );
    let hid = core.user_hid_orb(&inp(T0 + 200, g));
    assert_eq!(hid.bubbles_visible, Some(false));
    let snap = hid.snapshot.expect("the emptied stack is published");
    assert!(snap.items.is_empty());
    core.orb_visibility_changed(false, &inp(T0 + 210, g));
    let late = core.chat_entry(
        line(2, BubbleRole::Assistant, "after hide", stamp(SID, 0)),
        &inp(T0 + 300, g),
    );
    assert_no_items(&late);
}

#[test]
fn user_hide_then_show_leaves_the_orb_visible() {
    // A9-2 / Rev 10: an explicit show beats the session hide; reconcile never hides it again.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    core.user_hid_orb(&inp(T0 + 100, g));
    core.orb_visibility_changed(false, &inp(T0 + 100, g));
    let mut shown = inp(T0 + 200, g);
    shown.orb_persisted_visible = true;
    let show = core.user_showed_orb(&shown);
    assert_ne!(
        show.orb,
        Some(OrbAction::Hide),
        "a show never produces a hide"
    );
    assert_eq!(show.orb, Some(OrbAction::Show));
    core.orb_visibility_changed(true, &shown);
    let later = core.tick(&ShellInputs {
        now: T0 + 200 + LINGER_MS + 1_000,
        ..shown
    });
    assert_ne!(
        later.orb,
        Some(OrbAction::Hide),
        "the shown orb is not hidden by reconcile"
    );
    let much_later = core.tick(&ShellInputs {
        now: T0 + 600_000,
        ..shown
    });
    assert_eq!(much_later.orb, None);
}

// ---- Clear and reset: a seen line cannot come back ---------------------------------------------

#[test]
fn clear_and_reset_empty_the_stack_and_a_seen_line_cannot_return() {
    // A33: cleared text never reappears, even when the same line is delivered again.
    let (mut core, _instance) = open_session();
    let g = gate(SID, 0, true);
    let first = core.chat_entry(
        line(1, BubbleRole::User, "first", stamp(SID, 0)),
        &inp(T0 + 100, g),
    );
    assert!(shows(&first.snapshot, "first"));
    let cleared = core.chat_clear(&inp(T0 + 200, g));
    let snap = cleared.snapshot.expect("the clear is published");
    assert!(snap.items.is_empty());
    assert_eq!(cleared.bubbles_visible, Some(false));
    let again = core.chat_entry(
        line(1, BubbleRole::User, "first", stamp(SID, 0)),
        &inp(T0 + 300, g),
    );
    assert_no_items(&again);
    let reset = core.chat_reset(&inp(T0 + 400, g));
    assert_no_items(&reset);
}

// ---- A30 / A38: ready timeout and window failure -----------------------------------------------

#[test]
fn ready_timeout_falls_back_to_the_classic_window_exactly_once() {
    // A30 / A7-4: a waiting entry and no ready page within READY_TIMEOUT_MS opens classic once.
    let mut core = BubbleShellCore::new(true);
    let instance = core.window_created(T0);
    let g = gate(SID, 0, true);
    let started = core.session_started(SID, SessionSurface::OrbBubbles, true, &inp(T0, g));
    assert_eq!(started.orb, Some(OrbAction::Show));
    let waiting = core.chat_entry(
        line(1, BubbleRole::User, "waiting", stamp(SID, 0)),
        &inp(T0 + 10, g),
    );
    assert_no_items(&waiting);
    assert!(!core.tick(&inp(T0 + READY_TIMEOUT_MS - 1, g)).open_classic);
    assert!(core.tick(&inp(T0 + READY_TIMEOUT_MS, g)).open_classic);
    assert!(
        !core.tick(&inp(T0 + READY_TIMEOUT_MS + 1, g)).open_classic,
        "A30: exactly once per session"
    );
    assert!(
        core.page_ready(instance, &inp(T0 + 2_500, g)).is_none(),
        "A30: a later ready is ignored"
    );
    let after = core.chat_entry(
        line(2, BubbleRole::Assistant, "after", stamp(SID, 0)),
        &inp(T0 + 2_600, g),
    );
    assert_no_items(&after);
}

#[test]
fn ready_timeout_needs_a_waiting_entry() {
    // A30: no entry waiting means no fallback, however long the page takes.
    let mut core = BubbleShellCore::new(true);
    core.window_created(T0);
    core.session_started(
        SID,
        SessionSurface::OrbBubbles,
        true,
        &inp(T0, gate(SID, 0, true)),
    );
    let t = core.tick(&inp(T0 + READY_TIMEOUT_MS + 500, gate(SID, 0, true)));
    assert!(!t.open_classic);
}

#[test]
fn readiness_is_per_instance() {
    // A7-2: a new window creation makes the old instance's readiness void.
    let mut core = BubbleShellCore::new(true);
    let first = core.window_created(T0);
    let second = core.window_created(T0 + 10);
    assert!(core
        .page_ready(first, &inp(T0 + 20, gate(SID, 0, true)))
        .is_none());
    assert!(core
        .page_ready(second, &inp(T0 + 20, gate(SID, 0, true)))
        .is_some());
}

#[test]
fn windows_failure_opens_classic_once_and_never_shows_bubbles_for_that_session() {
    // A30 / A38: a failed orb or bubble window gives exactly one surface for this session.
    let mut core = BubbleShellCore::new(true);
    let g = gate(SID, 0, true);
    let first = core.session_started(SID, SessionSurface::OrbBubbles, false, &inp(T0, g));
    assert!(first.open_classic, "A30: the classic window opens");
    assert_ne!(
        first.orb,
        Some(OrbAction::Show),
        "a failed orb is not shown"
    );
    let second = core.session_started(SID, SessionSurface::OrbBubbles, false, &inp(T0 + 10, g));
    assert!(!second.open_classic, "A38: no duplicate surface");
    let offered = core.chat_entry(
        line(1, BubbleRole::User, "no bubbles", stamp(SID, 0)),
        &inp(T0 + 20, g),
    );
    assert_no_items(&offered);
    let t = core.tick(&inp(T0 + READY_TIMEOUT_MS + 100, g));
    assert!(!t.open_classic, "A38: the fallback is not repeated");
    // The next session is a fresh attempt.
    let next = core.session_started(
        SID + 1,
        SessionSurface::OrbBubbles,
        true,
        &inp(T0 + 3_000, gate(SID + 1, 0, true)),
    );
    assert_eq!(next.orb, Some(OrbAction::Show));
    assert!(!next.open_classic);
}

// ---- Typed session pairing ----------------------------------------------------------------------

// (Typed-session start/end without entries is covered above.)

// ---- session_surface: the platform decision (A20, Rev 6 R6-2, A7-4) -----------------------------

#[test]
fn macos_default_is_orb_bubbles() {
    // DELIBERATE default flip for G4b: the macOS default (no setting) is the orb with bubbles.
    assert_eq!(
        session_surface(None, &env(Os::MacOs, LinuxBackend::Unknown, false)),
        SessionSurface::OrbBubbles
    );
}

#[test]
fn windows_and_x11_composited_defaults_stay_window() {
    assert_eq!(
        session_surface(None, &env(Os::Windows, LinuxBackend::Unknown, false)),
        SessionSurface::Window
    );
    assert_eq!(
        session_surface(None, &env(Os::Linux, LinuxBackend::X11, true)),
        SessionSurface::Window
    );
}

#[test]
fn explicit_window_is_window_everywhere() {
    let envs = [
        env(Os::MacOs, LinuxBackend::Unknown, false),
        env(Os::Windows, LinuxBackend::Unknown, false),
        env(Os::Linux, LinuxBackend::X11, true),
        env(Os::Linux, LinuxBackend::Wayland, true),
    ];
    for e in envs {
        assert_eq!(
            session_surface(Some(Presentation::Window), &e),
            SessionSurface::Window,
            "{e:?}"
        );
    }
}

#[test]
fn explicit_floating_on_unsupported_platforms_is_window() {
    let envs = [
        env(Os::Linux, LinuxBackend::Wayland, true),
        env(Os::Linux, LinuxBackend::Unknown, true),
        env(Os::Linux, LinuxBackend::X11, false),
        env(Os::Other, LinuxBackend::Unknown, true),
    ];
    for e in envs {
        assert_eq!(
            session_surface(Some(Presentation::Floating), &e),
            SessionSurface::Window,
            "{e:?}"
        );
    }
}

#[test]
fn explicit_floating_on_windows_and_macos_is_orb_bubbles() {
    assert_eq!(
        session_surface(
            Some(Presentation::Floating),
            &env(Os::Windows, LinuxBackend::Unknown, false)
        ),
        SessionSurface::OrbBubbles
    );
    assert_eq!(
        session_surface(
            Some(Presentation::Floating),
            &env(Os::MacOs, LinuxBackend::Unknown, false)
        ),
        SessionSurface::OrbBubbles
    );
    assert_eq!(
        session_surface(
            Some(Presentation::Floating),
            &env(Os::Linux, LinuxBackend::X11, true)
        ),
        SessionSurface::OrbBubbles,
        "a composited X11 session may float"
    );
}
