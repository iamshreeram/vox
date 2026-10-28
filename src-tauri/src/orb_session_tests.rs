//! G4a spec: pure session machine for the wake-appear orb (DESIGN.md A7-1, A8-1, A9-2; R6-3 is
//! superseded where it conflicts, e.g. a late Entry keeps the session Lingering). Time is a fake
//! monotonic clock in ms. Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::orb_session`, new, no I/O, no clock reads):
//!   const LINGER_MS: u64 = 8000; const MAX_POST_END_EXTENSION_MS: u64 = 30000;
//!   const MAX_ACTIVE_MS: u64 = 900000;
//!   enum Phase { Idle, Active, Lingering }                  (Debug, Clone, Copy, PartialEq, Eq)
//!   struct SessionMachine: new(), started(sid, now), entry(token, now) -> bool, ended(sid, now),
//!     pin(), user_hid(), reset(), tick(now), phase() -> Phase, generation() -> u64,
//!     transient_shown() -> bool, pinned() -> bool, session_id() -> u64,
//!     desired_orb_visible(&self, persisted_visible: bool, supported: bool) -> bool
//!   enum OrbAction { Show, Hide }                           (Debug, Clone, Copy, PartialEq, Eq)
//!   fn reconcile(desired, actual, queued_generation, current_generation) -> Option<OrbAction>

use crate::orb_session::{
    reconcile, OrbAction, Phase, SessionMachine, LINGER_MS, MAX_ACTIVE_MS,
    MAX_POST_END_EXTENSION_MS,
};

const SID: u64 = 1;

fn started_at(sid: u64, now: u64) -> SessionMachine {
    let mut m = SessionMachine::new();
    m.started(sid, now);
    m
}

/// What the executor wants for an unpinned transient orb on a supported platform, with the
/// setting off (persisted false).
fn transient_visible(m: &SessionMachine) -> bool {
    m.desired_orb_visible(false, true)
}

fn bump_check(last: &mut u64, m: &SessionMachine, label: &str) {
    let g = m.generation();
    assert!(
        g > *last,
        "{label}: generation {g} did not grow past {last}"
    );
    *last = g;
}

#[test]
fn design_constants_match_the_spec() {
    assert_eq!(LINGER_MS, 8_000);
    assert_eq!(MAX_POST_END_EXTENSION_MS, 30_000);
    assert_eq!(MAX_ACTIVE_MS, 900_000);
}

#[test]
fn started_marks_active_and_wants_the_orb_even_when_not_persisted() {
    // A7-1 Started, A9-2 desired: a wake shows a hidden orb.
    let mut m = SessionMachine::new();
    assert_eq!(m.phase(), Phase::Idle);
    assert!(
        !transient_visible(&m),
        "an idle, unpersisted orb is not wanted"
    );
    m.started(SID, 1_000);
    assert_eq!(m.phase(), Phase::Active);
    assert_eq!(m.session_id(), SID);
    assert!(m.transient_shown(), "a started session is a transient show");
    assert!(!m.pinned());
    assert!(
        transient_visible(&m),
        "Started makes the hidden orb desired"
    );
    assert!(
        !m.desired_orb_visible(false, false),
        "an unsupported platform never shows the orb"
    );
}

#[test]
fn entry_extends_activity_and_a_wrong_token_is_rejected() {
    let mut m = started_at(SID, 0);
    assert!(m.entry(SID, 5_000), "the current token is accepted");
    assert!(
        !m.entry(SID + 1, 6_000),
        "a token of another session is rejected"
    );
    assert_eq!(m.phase(), Phase::Active);
    // The rejected entry changed nothing: the session ends with a deadline from the End time.
    m.ended(SID, 6_000);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(6_000 + LINGER_MS - 1);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(6_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle);
}

#[test]
fn entry_is_refused_while_idle() {
    let mut m = SessionMachine::new();
    assert!(!m.entry(SID, 0), "no session: nothing to extend");
    m.started(SID, 0);
    m.reset();
    assert!(!m.entry(SID, 10), "after reset the old token is refused");
}

#[test]
fn ended_moves_active_to_lingering_and_the_orb_stays_until_the_deadline() {
    // A7-1 Ended and Tick: inclusive deadline at ended_at + 8000.
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(1_000 + LINGER_MS - 1);
    assert_eq!(m.phase(), Phase::Lingering);
    assert!(transient_visible(&m), "still visible before the deadline");
    m.tick(1_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle);
    assert!(
        !transient_visible(&m),
        "at exactly 8000 ms the unpinned transient orb is no longer wanted"
    );
}

#[test]
fn session_without_entries_expires_eight_seconds_after_end() {
    // A21 and A7-1: no Entry at all still ends 8 s after End.
    let mut m = started_at(SID, 500);
    m.ended(SID, 2_000);
    m.tick(2_000 + LINGER_MS - 1);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(2_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle);
}

#[test]
fn late_entry_after_end_moves_the_deadline_to_now_plus_linger() {
    // A7-1 / A31: Entry while Lingering -> deadline = min(now + 8000, ended_at + 30000).
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000); // deadline 9000
    assert!(m.entry(SID, 5_000)); // min(13000, 31000) = 13000
    m.tick(12_999);
    assert_eq!(m.phase(), Phase::Lingering, "late entry keeps the orb");
    m.tick(13_000);
    assert_eq!(m.phase(), Phase::Idle);
}

#[test]
fn repeated_late_entries_cannot_strand_the_orb_beyond_thirty_seconds_after_end() {
    // A7-1 / A31: the total extension is capped at ended_at + 30000 = 31000.
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000); // deadline 9000
    assert!(m.entry(SID, 8_500)); // min(16500, 31000) = 16500
    m.tick(8_500);
    assert_eq!(m.phase(), Phase::Lingering);
    assert!(m.entry(SID, 16_000)); // min(24000, 31000) = 24000
    m.tick(16_000);
    assert_eq!(m.phase(), Phase::Lingering);
    assert!(m.entry(SID, 23_000)); // min(31000, 31000) = 31000
    assert!(m.entry(SID, 30_000)); // cap holds: still 31000
    m.tick(30_999);
    assert_eq!(m.phase(), Phase::Lingering, "one ms before the cap");
    m.tick(31_000);
    assert_eq!(m.phase(), Phase::Idle, "the cap is the hard end");
}

#[test]
fn persisted_visible_orb_is_never_hidden_by_expiry() {
    // A9-2: a persisted-visible orb stays desired after the session expires to Idle.
    let mut m = started_at(SID, 0);
    assert!(m.desired_orb_visible(true, true));
    m.ended(SID, 1_000);
    m.tick(1_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle, "the phase still goes Idle");
    assert!(
        m.desired_orb_visible(true, true),
        "a persisted-visible orb is never hidden by the machine"
    );
    assert_eq!(
        reconcile(
            m.desired_orb_visible(true, true),
            true,
            m.generation(),
            m.generation()
        ),
        None,
        "nothing to do for a persisted-visible orb that is already shown"
    );
}

#[test]
fn pin_during_a_transient_session_keeps_the_orb_after_expiry() {
    // A7-1 Pin, A9-2: pin clears the transient ownership; the pinned orb is never hidden.
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000);
    m.pin();
    assert!(m.pinned());
    assert!(!m.transient_shown(), "pin clears transient_shown");
    assert!(
        m.desired_orb_visible(true, true),
        "pinned orb is visible during Lingering"
    );
    m.tick(1_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle);
    assert!(
        m.desired_orb_visible(true, true),
        "A9-2: pinned orb is still desired after expiry to Idle"
    );
}

#[test]
fn unpinned_transient_orb_is_hidden_on_expiry() {
    // A7-1 Tick, A9-2: the transient show ends with a Hide on the executor.
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000);
    assert_eq!(
        reconcile(transient_visible(&m), false, m.generation(), m.generation()),
        Some(OrbAction::Show),
        "Lingering keeps the transient orb wanted, so a missing orb is shown"
    );
    m.tick(1_000 + LINGER_MS);
    assert!(!transient_visible(&m));
    assert_eq!(
        reconcile(transient_visible(&m), true, m.generation(), m.generation()),
        Some(OrbAction::Hide),
        "after expiry the shown transient orb is hidden"
    );
}

#[test]
fn started_asks_for_show_when_the_orb_is_hidden() {
    // A7-1 Started: ShowTransient (desired true, actual false, current generation).
    let mut m = SessionMachine::new();
    m.started(SID, 0);
    assert_eq!(
        reconcile(transient_visible(&m), false, m.generation(), m.generation()),
        Some(OrbAction::Show)
    );
}

#[test]
fn user_hid_stays_hidden_until_the_next_started_and_has_no_further_effects() {
    // A7-1 UserHid, A9-2: tray hide -> Idle, desired false even for a persisted orb.
    let mut m = started_at(SID, 0);
    let before_hide = m.generation();
    m.user_hid();
    assert!(m.generation() > before_hide);
    assert_eq!(m.phase(), Phase::Idle);
    assert!(!m.transient_shown());
    assert!(!m.pinned());
    assert!(!transient_visible(&m));
    assert!(
        !m.desired_orb_visible(true, true),
        "tray hide overrides the persisted orb for this session"
    );
    // No further actions: entries, ends and ticks do nothing.
    assert!(!m.entry(SID, 2_000));
    m.ended(SID, 3_000);
    assert_eq!(m.phase(), Phase::Idle);
    m.tick(100_000);
    assert_eq!(m.phase(), Phase::Idle);
    assert!(!m.desired_orb_visible(true, true));
    // The next session shows the orb again.
    m.started(2, 200_000);
    assert!(
        transient_visible(&m),
        "a new Started lifts the session hide"
    );
}

#[test]
fn show_queued_before_user_hid_is_dropped_when_executed() {
    // A8-1 / A32: Show(A) queued, user hides, executed -> stale generation -> nothing.
    let mut m = SessionMachine::new();
    m.started(SID, 0);
    let queued = m.generation();
    m.user_hid();
    let current = m.generation();
    assert_eq!(reconcile(true, false, queued, current), None);
    assert_eq!(
        reconcile(m.desired_orb_visible(false, true), false, queued, current),
        None
    );
}

#[test]
fn hide_queued_before_pin_is_dropped_when_executed() {
    // A8-1 / A32: Hide(A) queued, pin, executed -> nothing (orb must stay).
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000);
    let queued = m.generation();
    m.pin();
    let current = m.generation();
    assert_eq!(reconcile(false, true, queued, current), None);
    assert!(m.desired_orb_visible(true, true));
}

#[test]
fn duplicate_ended_is_ignored() {
    // A7-1: a second End must not reset ended_at or the deadline.
    let mut m = started_at(SID, 0);
    m.ended(SID, 1_000); // deadline 9000
    m.ended(SID, 5_000); // duplicate: ignored
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(1_000 + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle, "deadline still 9000, not 13000");
}

#[test]
fn stale_ended_for_an_older_session_is_ignored() {
    // A7-1: Ended{sid A} while B is Active changes nothing.
    let (a, b) = (1_u64, 2_u64);
    let mut m = started_at(a, 0);
    m.started(b, 1_000);
    m.ended(a, 2_000);
    assert_eq!(m.phase(), Phase::Active);
    assert_eq!(m.session_id(), b);
    m.tick(2_000);
    assert_eq!(m.phase(), Phase::Active, "B is still active");
    m.ended(b, 3_000);
    assert_eq!(m.phase(), Phase::Lingering);
}

#[test]
fn delayed_entry_for_an_older_session_changes_neither_activity_nor_deadline() {
    // A7-1 / A8-1: Entry(token A) during B is refused and leaves B's activity untouched.
    let (a, b) = (1_u64, 2_u64);
    let mut m = started_at(a, 0);
    m.started(b, 1_000);
    assert!(!m.entry(a, 500_000), "token A is not the current session");
    // If the stale entry had counted, B's activity would be at 500000 and the guard would not fire.
    m.tick(1_000 + MAX_ACTIVE_MS);
    assert_eq!(
        m.phase(),
        Phase::Lingering,
        "B's last activity is still 1000"
    );
}

#[test]
fn delayed_entry_for_an_older_session_does_not_move_the_deadline() {
    // A7-1 / A8-1: a stale entry while B is Lingering does not extend B's deadline.
    let (a, b) = (1_u64, 2_u64);
    let mut m = started_at(a, 0);
    m.started(b, 1_000);
    m.ended(b, 2_000); // deadline 10000
    assert!(!m.entry(a, 9_500));
    m.tick(10_000);
    assert_eq!(m.phase(), Phase::Idle, "deadline stays at 10000");
}

#[test]
fn started_during_lingering_cancels_the_pending_expiry() {
    // A7-1 Started: a new wake cancels the old deadline (no flicker, no Idle at the old time).
    let (a, b) = (1_u64, 2_u64);
    let mut m = started_at(a, 0);
    m.ended(a, 1_000); // old deadline 9000
    m.started(b, 5_000);
    assert_eq!(m.phase(), Phase::Active);
    assert_eq!(m.session_id(), b);
    m.tick(9_000);
    assert_eq!(m.phase(), Phase::Active, "no Idle at the old deadline");
    assert!(transient_visible(&m));
    m.ended(b, 10_000); // new deadline 18000
    m.tick(18_000 - 1);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(18_000);
    assert_eq!(m.phase(), Phase::Idle);
}

#[test]
fn active_without_entries_is_treated_as_ended_after_the_max_active_guard() {
    // A7-1 MAX_ACTIVE guard: no Entry for 900000 ms -> treated as ended at that tick, then lingers.
    let mut m = started_at(SID, 0);
    m.tick(MAX_ACTIVE_MS - 1);
    assert_eq!(m.phase(), Phase::Active);
    m.tick(MAX_ACTIVE_MS);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(MAX_ACTIVE_MS + LINGER_MS - 1);
    assert_eq!(m.phase(), Phase::Lingering);
    m.tick(MAX_ACTIVE_MS + LINGER_MS);
    assert_eq!(m.phase(), Phase::Idle);
}

#[test]
fn an_entry_resets_the_max_active_guard() {
    let mut m = started_at(SID, 0);
    assert!(m.entry(SID, 800_000));
    m.tick(1_000_000); // 200000 ms since the last entry
    assert_eq!(m.phase(), Phase::Active);
}

#[test]
fn tick_while_idle_is_a_no_op() {
    let mut m = SessionMachine::new();
    let before = m.generation();
    m.tick(1_000_000);
    assert_eq!(m.phase(), Phase::Idle);
    assert_eq!(m.generation(), before);
}

#[test]
fn reset_returns_to_idle_and_bumps_the_generation() {
    // A7-1 Reset: orb destroyed, presentation changed, or app exit.
    let mut m = started_at(SID, 0);
    let before = m.generation();
    m.reset();
    assert_eq!(m.phase(), Phase::Idle);
    assert!(m.generation() > before);
    assert!(!m.transient_shown());
    assert!(!transient_visible(&m));
    assert!(!m.entry(SID, 10));
}

#[test]
fn generation_strictly_increases_on_every_phase_or_flag_transition() {
    // A7-1 / A8-1: every transition that changes phase or flags bumps the generation.
    let mut m = SessionMachine::new();
    let mut last = m.generation();
    m.started(1, 0);
    bump_check(&mut last, &m, "started");
    m.ended(1, 1_000);
    bump_check(&mut last, &m, "ended");
    m.pin();
    bump_check(&mut last, &m, "pin");
    m.tick(1_000 + LINGER_MS);
    bump_check(&mut last, &m, "expiry");
    m.started(2, 20_000);
    bump_check(&mut last, &m, "second started");
    m.user_hid();
    bump_check(&mut last, &m, "user_hid");
    m.started(3, 30_000);
    bump_check(&mut last, &m, "third started");
    m.reset();
    bump_check(&mut last, &m, "reset");
}

#[test]
fn reconcile_ignores_stale_generations_and_matching_states() {
    assert_eq!(reconcile(true, false, 3, 4), None, "stale Show is dropped");
    assert_eq!(reconcile(false, true, 3, 4), None, "stale Hide is dropped");
    assert_eq!(reconcile(true, true, 4, 4), None, "already shown");
    assert_eq!(reconcile(false, false, 4, 4), None, "already hidden");
}

#[test]
fn reconcile_asks_for_the_difference_on_the_current_generation() {
    assert_eq!(reconcile(true, false, 4, 4), Some(OrbAction::Show));
    assert_eq!(reconcile(false, true, 4, 4), Some(OrbAction::Hide));
}
