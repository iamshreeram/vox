//! G4a spec: BubbleHub pure core (DESIGN.md A7-2, A8-2, A9-1; R6-4 is superseded where it
//! conflicts). Time is a fake clock in ms. Authoritative: do not weaken an assertion; if a test is
//! wrong, report it.
//!
//! Contract (module `crate::bubble_hub`, new; Rust-side pure core, no Tauri types):
//!   const MAX_ITEMS: usize = 3; const FADE_MS: u64 = 600;
//!   fn ttl_ms(chars: usize) -> u64            // clamp(3500 + 45 * chars, 4000, 12000)
//!   fn cap_display_text(text: &str) -> String // <= 240 chars, word-boundary cut with an ellipsis
//!   enum BubbleRole { User, Assistant }       (Debug, Clone, Copy, PartialEq, Eq)
//!   struct HubEntry { id: u64, role: BubbleRole, text: String, stamp: Stamp }  (pub fields)
//!   struct ProjItem { id, role, text, age_ms, ttl_ms }   struct Snapshot { instance, rev, items }
//!   struct BubbleHub: new(recording), set_recording, set_armed, set_ready(instance),
//!     new_instance() -> u64, set_orb_visible, set_classic_visible, set_placement_ok,
//!     on_entry(HubEntry, GateState, now) -> bool, on_reset(), on_clear(), prune(now),
//!     projection(now) -> Vec<ProjItem>, publish(now) -> Option<Snapshot>,
//!     snapshot_for_ready(instance, now) -> Option<Snapshot>
//!   Stamp / AdmissionGate / GateState come from crate::managers::chat_log (see the stamp spec).
//!   Stamps are only obtained from AdmissionGate::stamp() in these tests (Stamp is Copy).

use crate::bubble_hub::{
    cap_display_text, ttl_ms, BubbleHub, BubbleRole, HubEntry, FADE_MS, MAX_ITEMS,
};
use crate::managers::chat_log::{AdmissionGate, Stamp};

/// Emoji written as escapes so the source stays plain text.
const EMOJI: &str = "\u{1F600}";

fn make(id: u64, role: BubbleRole, text: &str, stamp: Stamp) -> HubEntry {
    HubEntry {
        id,
        role,
        text: text.to_string(),
        stamp,
    }
}

/// An entry stamped by the gate as it is right now (recording must be on).
fn entry(gate: &AdmissionGate, id: u64, role: BubbleRole, text: &str) -> HubEntry {
    make(id, role, text, gate.stamp().expect("recording is on"))
}

fn admit(hub: &mut BubbleHub, gate: &AdmissionGate, id: u64, text: &str, now: u64) -> bool {
    let e = entry(gate, id, BubbleRole::User, text);
    hub.on_entry(e, gate.state(), now)
}

/// A hub that is armed, recording, visible, placed and not yet ready for its instance.
fn armed() -> (BubbleHub, u64) {
    let mut hub = BubbleHub::new(true);
    hub.set_armed(true);
    hub.set_orb_visible(true);
    hub.set_classic_visible(false);
    hub.set_placement_ok(true);
    let instance = hub.new_instance();
    (hub, instance)
}

/// [`armed`] plus the page readiness call: the only state in which bubbles project (A8-2).
fn live() -> (BubbleHub, u64) {
    let (mut hub, instance) = armed();
    hub.set_ready(instance);
    (hub, instance)
}

fn texts(hub: &BubbleHub, now: u64) -> Vec<String> {
    hub.projection(now).into_iter().map(|i| i.text).collect()
}

// ---- pure helpers -------------------------------------------------------------------------

#[test]
fn constants_match_the_spec() {
    assert_eq!(MAX_ITEMS, 3);
    assert_eq!(FADE_MS, 600);
}

#[test]
fn ttl_is_clamped_between_four_and_twelve_seconds() {
    // A7-2 / R6-4 lifetime rule.
    assert_eq!(ttl_ms(0), 4_000);
    assert_eq!(ttl_ms(10), 4_000, "3950 is raised to the floor");
    assert_eq!(ttl_ms(12), 4_040);
    assert_eq!(ttl_ms(100), 8_000);
    assert_eq!(ttl_ms(188), 11_960);
    assert_eq!(ttl_ms(189), 12_000, "12005 is capped");
    assert_eq!(ttl_ms(240), 12_000);
    assert_eq!(ttl_ms(10_000_000), 12_000);
}

#[test]
fn cap_display_text_collapses_whitespace_and_trims() {
    assert_eq!(cap_display_text(""), "");
    assert_eq!(cap_display_text("   \n\t "), "");
    assert_eq!(cap_display_text("  hello \n  world\t!  "), "hello world !");
}

#[test]
fn cap_display_text_keeps_text_of_exactly_the_limit() {
    let exact = "a".repeat(240);
    assert_eq!(cap_display_text(&exact), exact);
    let padded = format!("  {}  ", "b".repeat(240));
    assert_eq!(cap_display_text(&padded), "b".repeat(240));
}

/// True when `body` is one or more copies of `word` separated by single spaces.
fn only_word(body: &str, word: &str) -> bool {
    !body.is_empty() && body.split(' ').all(|w| w == word)
}

#[test]
fn cap_display_text_cuts_at_a_word_boundary_with_an_ellipsis() {
    let long = "word ".repeat(100); // 500 chars
    let out = cap_display_text(&long);
    assert!(out.chars().count() <= 240, "at most 240 chars");
    assert!(out.ends_with('…'), "trailing ellipsis: {out:?}");
    let body = out.trim_end_matches('…').trim_end();
    assert!(
        only_word(body, "word"),
        "cut only between whole words: {body:?}"
    );
    assert_eq!(cap_display_text(&out), out, "idempotent");
}

#[test]
fn cap_display_text_is_utf8_safe_for_multibyte_and_emoji() {
    let accents = "é ".repeat(200);
    let out = cap_display_text(&accents);
    assert!(out.chars().count() <= 240);
    assert!(out.ends_with('…'));
    assert!(only_word(out.trim_end_matches('…').trim_end(), "é"));

    let emoji = format!("{EMOJI} ").repeat(200);
    let out = cap_display_text(&emoji);
    assert!(out.chars().count() <= 240);
    assert!(out.ends_with('…'));
    assert!(only_word(out.trim_end_matches('…').trim_end(), EMOJI));
}

// ---- admission ----------------------------------------------------------------------------

#[test]
fn ids_not_greater_than_the_highest_seen_are_ignored() {
    // A7-2: duplicate or older ids are ignored; the highest id survives a reset.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 5, "five", 0));
    assert!(
        !admit(&mut hub, &gate, 5, "duplicate", 0),
        "equal id ignored"
    );
    assert!(!admit(&mut hub, &gate, 4, "older", 0), "older id ignored");
    assert!(admit(&mut hub, &gate, 6, "six", 0));
    assert_eq!(texts(&hub, 0), vec!["five".to_string(), "six".to_string()]);
    hub.on_reset();
    assert!(
        !admit(&mut hub, &gate, 6, "again", 0),
        "the highest seen id survives a reset"
    );
    assert!(admit(&mut hub, &gate, 7, "seven", 0));
}

#[test]
fn a_fourth_entry_drops_the_oldest() {
    // A7-2: at most MAX_ITEMS newest, oldest first in the projection.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    for id in 1..=4 {
        assert!(admit(&mut hub, &gate, id, &format!("m{id}"), 0), "id {id}");
    }
    let items = hub.projection(0);
    assert_eq!(items.len(), MAX_ITEMS);
    let ids: Vec<u64> = items.iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![2, 3, 4], "oldest first, newest kept");
}

#[test]
fn projected_text_is_capped_and_roles_are_kept() {
    // A7-2: stored text is cap_display_text(text); the role is carried through.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    let raw = format!("  hi  {}  ", "z ".repeat(300));
    assert!(hub.on_entry(
        entry(&gate, 1, BubbleRole::Assistant, &raw),
        gate.state(),
        0
    ));
    let items = hub.projection(0);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, cap_display_text(&raw));
    assert!(items[0].text.chars().count() <= 240);
    assert_eq!(items[0].role, BubbleRole::Assistant);
}

#[test]
fn reset_and_clear_empty_everything() {
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "one", 0));
    assert!(admit(&mut hub, &gate, 2, "two", 0));
    assert_eq!(hub.projection(0).len(), 2);
    hub.on_reset();
    assert!(hub.projection(0).is_empty(), "reset empties the projection");
    assert!(admit(&mut hub, &gate, 3, "three", 0));
    hub.on_clear();
    assert!(hub.projection(0).is_empty(), "clear empties the projection");
}

#[test]
fn stamp_from_an_older_session_token_is_rejected() {
    // A8-1 / A9-1: admission compares the entry tuple with the current gate tuple.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    let old = make(
        1,
        BubbleRole::User,
        "old session",
        gate.stamp().expect("on"),
    );
    gate.bump_session();
    assert!(
        !hub.on_entry(old, gate.state(), 0),
        "older session token rejected"
    );
    assert!(admit(&mut hub, &gate, 2, "new session", 0));
    assert_eq!(texts(&hub, 0), vec!["new session".to_string()]);
}

#[test]
fn stamp_from_an_older_recording_epoch_is_rejected_even_when_recording_is_back_on() {
    // A8-1 / A9-1: off then on bumps the epoch twice; the old stamp can never match again.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    let old = make(
        1,
        BubbleRole::User,
        "before toggle",
        gate.stamp().expect("on"),
    );
    gate.set_recording(false);
    gate.set_recording(true);
    hub.set_recording(false);
    hub.set_recording(true);
    assert!(!hub.on_entry(old, gate.state(), 0), "older epoch rejected");
    assert!(admit(&mut hub, &gate, 2, "after toggle", 0));
    assert_eq!(texts(&hub, 0), vec!["after toggle".to_string()]);
}

#[test]
fn entries_are_refused_while_the_hub_recording_flag_is_off() {
    // A8-1: the hub's own recording flag closes admission even for a current stamp.
    let gate = AdmissionGate::new(true);
    let mut hub = BubbleHub::new(false);
    hub.set_armed(true);
    hub.set_orb_visible(true);
    hub.set_placement_ok(true);
    let instance = hub.new_instance();
    hub.set_ready(instance);
    assert!(!admit(&mut hub, &gate, 1, "refused", 0));
    assert!(hub.projection(0).is_empty());
}

// ---- recording ----------------------------------------------------------------------------

#[test]
fn recording_off_clears_and_publishes_an_empty_snapshot() {
    // A8-1 / A34: recording off -> gate closed, ring cleared, empty DOM snapshot.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "one", 0));
    assert!(admit(&mut hub, &gate, 2, "two", 0));
    let first = hub.publish(0).expect("populated snapshot first");
    assert_eq!(first.items.len(), 2);

    gate.set_recording(false);
    hub.set_recording(false);
    let cleared = hub.publish(0).expect("recording off publishes");
    assert!(cleared.items.is_empty());
    assert!(cleared.rev > first.rev);
    assert!(hub.projection(0).is_empty());
}

#[test]
fn recording_back_on_never_restores_old_text() {
    // A34: re-enabling the chat log does not bring back cleared lines.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    let kept_stamp = gate.stamp().expect("on");
    assert!(admit(&mut hub, &gate, 1, "secret one", 0));
    hub.publish(0).expect("shown");

    gate.set_recording(false);
    hub.set_recording(false);
    let _ = hub.publish(0);
    gate.set_recording(true);
    hub.set_recording(true);

    assert!(hub.projection(0).is_empty(), "old text is not restored");
    assert!(
        hub.publish(0).is_none(),
        "nothing new to publish after the empty state"
    );
    assert!(
        !hub.on_entry(
            make(2, BubbleRole::User, "late old stamp", kept_stamp),
            gate.state(),
            0
        ),
        "a line stamped before the toggle is refused"
    );
    assert!(hub.projection(0).is_empty());
    assert!(admit(&mut hub, &gate, 3, "fresh", 0));
    assert_eq!(texts(&hub, 0), vec!["fresh".to_string()]);
}

// ---- projection gates ---------------------------------------------------------------------

#[test]
fn projection_is_empty_until_the_page_is_ready() {
    // A7-2 / A8-2: nothing is shown before the page calls back for this instance.
    let gate = AdmissionGate::new(true);
    let (mut hub, instance) = armed();
    assert!(admit(&mut hub, &gate, 1, "waiting", 0));
    assert!(hub.projection(0).is_empty(), "not ready yet");
    hub.set_ready(instance);
    assert_eq!(texts(&hub, 0), vec!["waiting".to_string()]);
}

#[test]
fn every_projection_gate_must_hold() {
    // A8-2: armed && recording && ready && orb visible && !classic visible && placement ok.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "shown", 0));
    assert_eq!(hub.projection(0).len(), 1);

    hub.set_armed(false);
    assert!(hub.projection(0).is_empty(), "disarmed");
    hub.set_armed(true);
    hub.set_orb_visible(false);
    assert!(hub.projection(0).is_empty(), "orb hidden");
    hub.set_orb_visible(true);
    hub.set_classic_visible(true);
    assert!(hub.projection(0).is_empty(), "classic window visible");
    hub.set_classic_visible(false);
    hub.set_placement_ok(false);
    assert!(hub.projection(0).is_empty(), "no placement");
    hub.set_placement_ok(true);
    assert_eq!(hub.projection(0).len(), 1, "all gates open again");
}

/// Suppression and restoration through one gate. `set(hub, true)` means the gate is open.
/// Items must survive a suppression: suppression publishes empty, restoration publishes a newer
/// populated snapshot (A7-2 / A8-2).
fn suppresses_and_restores(set: fn(&mut BubbleHub, bool)) {
    let gate = AdmissionGate::new(true);
    let (mut hub, instance) = live();
    assert!(admit(&mut hub, &gate, 1, "one", 0));
    assert!(admit(&mut hub, &gate, 2, "two", 0));
    let shown = hub.publish(0).expect("populated");
    assert_eq!(shown.items.len(), 2);

    set(&mut hub, false);
    let hidden = hub.publish(1_000).expect("suppression publishes");
    assert!(hidden.items.is_empty());
    assert!(hidden.rev > shown.rev);

    let during = hub
        .snapshot_for_ready(instance, 1_000)
        .expect("current instance answers while suppressed");
    assert!(
        during.items.is_empty(),
        "readiness during suppression is empty"
    );
    assert!(during.rev >= hidden.rev, "never an older rev");

    set(&mut hub, true);
    let back = hub.publish(1_000).expect("restoration publishes");
    assert_eq!(back.items.len(), 2, "lines still alive before their ttl");
    assert!(back.rev > hidden.rev, "restoration is strictly newer");
    assert_eq!(
        back.items[0].age_ms, 1_000,
        "age comes from the original entry time"
    );
}

#[test]
fn opening_the_classic_window_suppresses_and_closing_it_restores() {
    // The classic flag is inverted: open = suppressed.
    suppresses_and_restores(|hub, open_gate| hub.set_classic_visible(!open_gate));
}

#[test]
fn orb_hide_suppresses_and_show_restores() {
    suppresses_and_restores(|hub, open_gate| hub.set_orb_visible(open_gate));
}

#[test]
fn placement_failure_suppresses_and_recovery_restores() {
    suppresses_and_restores(|hub, open_gate| hub.set_placement_ok(open_gate));
}

// ---- ttl and publish ----------------------------------------------------------------------

#[test]
fn ttl_expiry_removes_items_and_publishes_empty() {
    // A7-2: items expire at born + ttl (exclusive); the publish after expiry is empty.
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "hi", 0));
    let shown = hub.publish(0).expect("shown");
    assert_eq!(shown.items[0].ttl_ms, 4_000);
    assert_eq!(
        hub.projection(3_999).len(),
        1,
        "alive one ms before the ttl"
    );
    let gone = hub.publish(4_000).expect("expiry publishes");
    assert!(gone.items.is_empty());
    assert!(gone.rev > shown.rev);
    assert!(hub.projection(4_000).is_empty());
}

#[test]
fn projection_reports_age_and_ttl() {
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "hi", 2_000));
    let items = hub.projection(3_500);
    assert_eq!(items[0].age_ms, 1_500);
    assert_eq!(items[0].ttl_ms, 4_000);
}

#[test]
fn publish_is_none_when_nothing_changed() {
    let gate = AdmissionGate::new(true);
    let (mut hub, _) = live();
    assert!(admit(&mut hub, &gate, 1, "once", 0));
    assert!(hub.publish(0).is_some());
    assert!(hub.publish(0).is_none(), "same projection, same time");
}

// ---- readiness and instances --------------------------------------------------------------

#[test]
fn stale_instance_gets_nothing_and_a_new_instance_starts_unready() {
    // A7-2: a window replacement gets a new instance id; readiness resets; stale callers get None.
    let gate = AdmissionGate::new(true);
    let (mut hub, first) = live();
    assert!(admit(&mut hub, &gate, 1, "kept", 0));
    assert_eq!(hub.projection(0).len(), 1);

    let second = hub.new_instance();
    assert_ne!(first, second);
    assert!(hub.projection(0).is_empty(), "new window is not ready");
    assert!(
        hub.snapshot_for_ready(first, 0).is_none(),
        "stale instance answers nothing"
    );
    hub.set_ready(first);
    assert!(
        hub.projection(0).is_empty(),
        "a stale ready does not make the new window ready"
    );

    hub.set_ready(second);
    let published = hub.publish(0).expect("populated for the new window");
    let snap = hub
        .snapshot_for_ready(second, 0)
        .expect("current instance answers");
    assert_eq!(snap.instance, second);
    assert!(
        snap.rev >= published.rev,
        "readiness snapshot is never older"
    );
    assert_eq!(snap.items.len(), 1, "the retained entry is replayed");
}

// ---- permutations -------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Step {
    EntryA,
    Clear,
    EntryB,
    RecOff,
    RecOn,
    Ready,
    Suppress,
    Unsuppress,
}

fn permutations(items: &[Step]) -> Vec<Vec<Step>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, head);
            out.push(tail);
        }
    }
    out
}

/// Checks the projected texts of one moment. Nothing is projected while recording is off; once
/// "secret-a" was admitted and then cleared it must never be projected again (each line is
/// admitted at most once, so it cannot come back by re-admission).
fn check_moment(texts: &[String], forbidden: bool, recording: bool, order: &[Step]) {
    assert!(
        recording || texts.is_empty(),
        "items while recording is off in {order:?}"
    );
    if forbidden {
        assert!(
            texts.iter().all(|t| t != "secret-a"),
            "cleared text reappeared in {order:?}"
        );
    }
}

/// Applies one ordering of the steps and checks the invariants after every step: revs never
/// decrease, and every projected moment satisfies [`check_moment`].
fn run_order(order: &[Step]) {
    let gate = AdmissionGate::new(true);
    let mut hub = BubbleHub::new(true);
    hub.set_armed(true);
    hub.set_orb_visible(true);
    hub.set_classic_visible(false);
    hub.set_placement_ok(true);
    let instance = hub.new_instance();
    let stamp = gate.stamp().expect("on");

    let mut last_rev = 0_u64;
    let mut recording = true;
    let mut a_admitted = false;
    let mut forbidden = false;

    for step in order {
        match step {
            Step::EntryA => {
                let e = make(1, BubbleRole::User, "secret-a", stamp);
                if hub.on_entry(e, gate.state(), 0) {
                    a_admitted = true;
                }
            }
            Step::Clear => {
                hub.on_clear();
                if a_admitted {
                    forbidden = true;
                }
            }
            Step::EntryB => {
                let e = make(2, BubbleRole::Assistant, "bee", stamp);
                let _ = hub.on_entry(e, gate.state(), 0);
            }
            Step::RecOff => {
                gate.set_recording(false);
                hub.set_recording(false);
                recording = false;
            }
            Step::RecOn => {
                gate.set_recording(true);
                hub.set_recording(true);
                recording = true;
            }
            Step::Ready => {
                hub.set_ready(instance);
                if let Some(s) = hub.snapshot_for_ready(instance, 0) {
                    assert!(s.rev >= last_rev, "ready rev went backwards in {order:?}");
                    last_rev = s.rev;
                    let t: Vec<String> = s.items.iter().map(|i| i.text.clone()).collect();
                    check_moment(&t, forbidden, recording, order);
                }
            }
            Step::Suppress => hub.set_classic_visible(true),
            Step::Unsuppress => hub.set_classic_visible(false),
        }
        if let Some(s) = hub.publish(0) {
            assert!(s.rev >= last_rev, "publish rev went backwards in {order:?}");
            last_rev = s.rev;
            let t: Vec<String> = s.items.iter().map(|i| i.text.clone()).collect();
            check_moment(&t, forbidden, recording, order);
        }
        check_moment(&texts(&hub, 0), forbidden, recording, order);
    }
}

#[test]
fn every_ordering_keeps_cleared_text_gone_and_revs_monotonic() {
    // A33 / A8-2: permutation test over all 8! orders of the steps.
    let steps = [
        Step::EntryA,
        Step::Clear,
        Step::EntryB,
        Step::RecOff,
        Step::RecOn,
        Step::Ready,
        Step::Suppress,
        Step::Unsuppress,
    ];
    let all = permutations(&steps);
    assert_eq!(all.len(), 40_320);
    for order in &all {
        run_order(order);
    }
}
