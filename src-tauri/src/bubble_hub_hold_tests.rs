//! Spec: a spoken assistant bubble stays up for as long as its reply is being spoken.
//!
//! Bug: a bubble's lifetime was a fixed timer (`ttl_ms`) started when the text was queued, so a
//! bubble could vanish before its audio began or while it was still playing. Authoritative: do
//! not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::bubble_hub`, additions; time is a fake clock in ms):
//!   const HOLD_MAX_MS: u64 = 120_000;   // deliberate cap: the one exception to "as long as spoken"
//!   const LINGER_MS: u64 = 2_000;       // how long a held bubble stays after its speech ends
//!   BubbleHub::speech_begin(&mut self, scope: u64, stamp: Stamp, gate: GateState, now: u64) -> bool
//!   BubbleHub::speech_end(&mut self, scope: u64, now: u64)
//!
//! Rules:
//!  * A speech SCOPE is one stretch of spoken reply, named by a unique id. The signals arrive in
//!    order with the chat lines (one worker stream), so a line admitted between a scope's begin
//!    and end belongs to that scope.
//!  * `speech_begin` is admitted exactly like a line: refused (returns false, changes nothing)
//!    when the hub is not recording and armed, or the stamp does not match the gate state.
//!    A repeated begin of the same scope is a no-op that returns true.
//!  * An ASSISTANT line admitted while at least one scope is active is HELD by the NEWEST active
//!    scope: it projects `ttl_ms == HOLD_MAX_MS` and is alive until `born + HOLD_MAX_MS`. User
//!    lines are never held.
//!  * `speech_end(scope)` removes that scope and releases the lines it holds: reborn at that
//!    instant with `ttl_ms == LINGER_MS` (so `age_ms == 0` in the next projection). An end for
//!    an unknown scope changes nothing. Lines held by another scope stay held. A line that was
//!    already pruned is never brought back.
//!  * A scope that never ends is forgotten `HOLD_MAX_MS` after its begin, so a lost end signal
//!    cannot make later, unrelated lines held. The hub prunes it lazily on every call.
//!  * A line admitted while no scope is active keeps the normal `ttl_ms(chars)` lifetime.
//!  * `on_reset` clears every scope as well as the lines. `on_clear` clears lines only.
//!  * While a line is held, the projection does not change with time alone, so `publish`
//!    returns `None` (the page keeps the same revision and the bubble is not restarted).

use crate::bubble_hub::{
    ttl_ms, BubbleHub, BubbleRole, HubEntry, FADE_MS, HOLD_MAX_MS, LINGER_MS, MAX_ITEMS,
};
use crate::managers::chat_log::AdmissionGate;

fn gate_on() -> AdmissionGate {
    AdmissionGate::new(true)
}

fn live() -> BubbleHub {
    let mut hub = BubbleHub::new(true);
    hub.set_armed(true);
    hub.set_orb_visible(true);
    hub.set_classic_visible(false);
    hub.set_placement_ok(true);
    let instance = hub.new_instance();
    hub.set_ready(instance);
    hub
}

fn add(hub: &mut BubbleHub, gate: &AdmissionGate, id: u64, role: BubbleRole, text: &str, now: u64) {
    let e = HubEntry {
        id,
        role,
        text: text.to_string(),
        stamp: gate.stamp().expect("recording is on"),
    };
    assert!(
        hub.on_entry(e, gate.state(), now),
        "line {id} must be admitted"
    );
}

fn begin(hub: &mut BubbleHub, gate: &AdmissionGate, scope: u64, now: u64) -> bool {
    let stamp = gate.stamp().expect("recording is on");
    hub.speech_begin(scope, stamp, gate.state(), now)
}

fn ids(hub: &BubbleHub, now: u64) -> Vec<u64> {
    hub.projection(now).into_iter().map(|i| i.id).collect()
}

fn ttl_of(hub: &BubbleHub, id: u64, now: u64) -> u64 {
    hub.projection(now)
        .into_iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("line {id} is not shown at {now}"))
        .ttl_ms
}

#[test]
fn constants_are_sane() {
    assert_eq!(HOLD_MAX_MS, 120_000);
    assert_eq!(LINGER_MS, 2_000);
    assert!(LINGER_MS > FADE_MS, "the linger must outlast the fade");
    let longest_plain = ttl_ms(10_000_000);
    assert!(
        HOLD_MAX_MS > longest_plain,
        "a hold outlasts the longest plain lifetime"
    );
}

#[test]
fn an_assistant_line_admitted_while_speaking_is_held() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 900));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Here is a long answer.",
        1_000,
    );
    let p = hub.projection(1_000);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].ttl_ms, HOLD_MAX_MS);
    assert_eq!(p[0].age_ms, 0);
}

#[test]
fn a_held_line_outlives_the_plain_lifetime() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(&mut hub, &gate, 1, BubbleRole::Assistant, "Short.", 0);
    // The plain lifetime of a short line is 4 s and the longest possible is 12 s.
    assert_eq!(ids(&hub, 4_000), vec![1], "still here after the 4 s floor");
    assert_eq!(ids(&hub, 12_000), vec![1], "still here after the 12 s cap");
    assert_eq!(
        ids(&hub, 60_000),
        vec![1],
        "still here while speech goes on"
    );
    assert_eq!(ids(&hub, HOLD_MAX_MS - 1), vec![1]);
}

#[test]
fn a_held_line_is_pruned_at_the_safety_cap_if_the_end_never_comes() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 500));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Lost end signal.",
        500,
    );
    assert!(
        ids(&hub, 500 + HOLD_MAX_MS).is_empty(),
        "alive strictly before born + cap"
    );
    hub.prune(500 + HOLD_MAX_MS);
    assert!(
        hub.projection(500 + HOLD_MAX_MS - 1).is_empty(),
        "pruned for good"
    );
}

#[test]
fn a_scope_whose_end_was_lost_stops_holding_later_lines() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    // No end ever comes. Long after the cap, an unrelated reply arrives.
    let later = HOLD_MAX_MS + 1_000;
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Unrelated reply.",
        later,
    );
    assert_eq!(
        ttl_of(&hub, 1, later),
        ttl_ms("Unrelated reply.".chars().count()),
        "the forgotten scope must not hold it"
    );
}

#[test]
fn speech_end_releases_a_held_line_with_a_short_linger() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Spoken at length.",
        0,
    );
    hub.speech_end(1, 30_000);
    let p = hub.projection(30_000);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].age_ms, 0, "reborn at the end of the speech");
    assert_eq!(p[0].ttl_ms, LINGER_MS);
    assert_eq!(ids(&hub, 30_000 + LINGER_MS - 1), vec![1]);
    assert!(ids(&hub, 30_000 + LINGER_MS).is_empty());
}

#[test]
fn an_end_after_the_line_was_pruned_does_not_bring_it_back() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Held too long.",
        0,
    );
    hub.prune(HOLD_MAX_MS + 10);
    hub.speech_end(1, HOLD_MAX_MS + 5_000);
    assert!(
        hub.projection(HOLD_MAX_MS + 5_000).is_empty(),
        "no resurrection"
    );
}

#[test]
fn publish_reports_the_hold_and_the_release_but_not_the_passing_of_time() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(&mut hub, &gate, 1, BubbleRole::Assistant, "Held.", 0);
    let first = hub.publish(0).expect("the new line is published");
    assert_eq!(first.items[0].ttl_ms, HOLD_MAX_MS);
    assert!(
        hub.publish(50_000).is_none(),
        "time alone changes nothing while held"
    );
    hub.speech_end(1, 60_000);
    let released = hub
        .publish(60_000)
        .expect("the release changes the projection");
    assert!(released.rev > first.rev);
    assert_eq!(released.items[0].ttl_ms, LINGER_MS);
    assert_eq!(released.items[0].age_ms, 0);
}

#[test]
fn user_lines_are_never_held() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(&mut hub, &gate, 1, BubbleRole::User, "What time is it", 0);
    assert_eq!(
        ttl_of(&hub, 1, 0),
        ttl_ms("What time is it".chars().count())
    );
    assert!(
        ids(&hub, 20_000).is_empty(),
        "a user line still expires normally"
    );
}

#[test]
fn a_line_admitted_while_not_speaking_keeps_the_plain_lifetime() {
    // Text-only conversation or spoken replies off: nothing changes.
    let gate = gate_on();
    let mut hub = live();
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "No voice here.",
        0,
    );
    assert_eq!(ttl_of(&hub, 1, 0), ttl_ms("No voice here.".chars().count()));
    assert!(ids(&hub, 12_000).is_empty());
}

#[test]
fn a_line_admitted_after_the_speech_ended_keeps_the_plain_lifetime() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    hub.speech_end(1, 100);
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Late arrival.",
        200,
    );
    assert_eq!(
        ttl_of(&hub, 1, 200),
        ttl_ms("Late arrival.".chars().count())
    );
}

#[test]
fn every_chunk_of_a_streamed_reply_is_held_and_released_together() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "First sentence.",
        0,
    );
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "Second sentence.",
        5_000,
    );
    add(
        &mut hub,
        &gate,
        3,
        BubbleRole::Assistant,
        "Third sentence.",
        10_000,
    );
    assert_eq!(MAX_ITEMS, 3);
    assert_eq!(ids(&hub, 40_000), vec![1, 2, 3], "all three are still up");
    hub.speech_end(1, 40_000);
    for item in hub.projection(40_000) {
        assert_eq!(item.age_ms, 0);
        assert_eq!(item.ttl_ms, LINGER_MS);
    }
    assert!(ids(&hub, 40_000 + LINGER_MS).is_empty());
}

#[test]
fn a_fourth_line_still_drops_the_oldest_even_if_held() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    for id in 1..=4 {
        add(
            &mut hub,
            &gate,
            id,
            BubbleRole::Assistant,
            &format!("Line {id}."),
            id * 100,
        );
    }
    assert_eq!(ids(&hub, 1_000), vec![2, 3, 4]);
}

#[test]
fn the_newest_active_scope_holds_a_new_line_and_each_end_releases_only_its_own() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Held by scope one.",
        10,
    );
    assert!(begin(&mut hub, &gate, 2, 20));
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "Held by scope two.",
        30,
    );
    hub.speech_end(1, 10_000);
    assert_eq!(
        ttl_of(&hub, 1, 10_000),
        LINGER_MS,
        "scope one's line is released"
    );
    assert_eq!(
        ttl_of(&hub, 2, 10_000),
        HOLD_MAX_MS,
        "scope two's line stays held"
    );
    assert_eq!(ids(&hub, 50_000), vec![2]);
    hub.speech_end(2, 50_000);
    assert_eq!(ttl_of(&hub, 2, 50_000), LINGER_MS);
}

#[test]
fn an_end_for_an_unknown_scope_changes_nothing() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 5, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Still being spoken.",
        10,
    );
    hub.speech_end(99, 20); // stale or never begun
    assert_eq!(
        ttl_of(&hub, 1, 20),
        HOLD_MAX_MS,
        "the other scope is not released"
    );
    // The unknown end did not poison a later scope with that id.
    hub.speech_end(5, 30);
    assert!(begin(&mut hub, &gate, 99, 40));
    add(&mut hub, &gate, 2, BubbleRole::Assistant, "Next reply.", 50);
    assert_eq!(ttl_of(&hub, 2, 50), HOLD_MAX_MS);
}

#[test]
fn a_repeated_begin_of_the_same_scope_is_idempotent() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    assert!(begin(&mut hub, &gate, 1, 5));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "One end is enough.",
        10,
    );
    hub.speech_end(1, 1_000);
    assert_eq!(ttl_of(&hub, 1, 1_000), LINGER_MS);
}

#[test]
fn a_begin_with_a_stale_stamp_is_refused() {
    let gate = gate_on();
    let mut hub = live();
    let stale = gate.stamp().expect("recording is on");
    gate.bump_session(); // a new session started after the signal was stamped
    assert!(!hub.speech_begin(1, stale, gate.state(), 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "New session line.",
        10,
    );
    assert_eq!(
        ttl_of(&hub, 1, 10),
        ttl_ms("New session line.".chars().count())
    );
}

#[test]
fn a_begin_is_refused_while_the_hub_is_not_armed_or_not_recording() {
    let gate = gate_on();
    let mut unarmed = BubbleHub::new(true);
    assert!(!begin(&mut unarmed, &gate, 1, 0));
    unarmed.speech_end(1, 10); // harmless
    assert!(unarmed.projection(10).is_empty());

    let mut not_recording = live();
    not_recording.set_recording(false);
    assert!(!begin(&mut not_recording, &gate, 1, 0));
}

#[test]
fn a_reset_clears_every_scope_and_every_line() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Before reset.",
        0,
    );
    hub.on_reset();
    assert!(hub.projection(0).is_empty());
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "After reset.",
        10,
    );
    assert_eq!(
        ttl_of(&hub, 2, 10),
        ttl_ms("After reset.".chars().count()),
        "no scope is active after a reset"
    );
    // The old session's late end cannot release or disturb the new session.
    assert!(begin(&mut hub, &gate, 7, 20));
    add(&mut hub, &gate, 3, BubbleRole::Assistant, "New reply.", 30);
    hub.speech_end(1, 40);
    assert_eq!(ttl_of(&hub, 3, 40), HOLD_MAX_MS);
}

#[test]
fn a_clear_removes_the_lines_but_the_scope_stays_active() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "Before clear.",
        0,
    );
    hub.on_clear();
    assert!(hub.projection(0).is_empty());
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "Still being spoken.",
        10,
    );
    assert_eq!(ttl_of(&hub, 2, 10), HOLD_MAX_MS);
}

#[test]
fn a_very_long_reply_is_held_with_its_capped_display_text() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    let long = "word ".repeat(400);
    add(&mut hub, &gate, 1, BubbleRole::Assistant, &long, 0);
    let p = hub.projection(0);
    assert!(p[0].text.chars().count() <= 240);
    assert_eq!(p[0].ttl_ms, HOLD_MAX_MS);
}

#[test]
fn any_script_is_held_the_same_way() {
    // The hold does not depend on how long the text is or which script it uses.
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(
        &mut hub,
        &gate,
        1,
        BubbleRole::Assistant,
        "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\u{3002}",
        0,
    );
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "\u{0645}\u{0631}\u{062d}\u{0628}\u{0627} \u{0628}\u{0643}",
        10,
    );
    add(
        &mut hub,
        &gate,
        3,
        BubbleRole::Assistant,
        "\u{0e2a}\u{0e27}\u{0e31}\u{0e2a}\u{0e14}\u{0e35}",
        20,
    );
    assert_eq!(ids(&hub, 60_000), vec![1, 2, 3]);
}

#[test]
fn a_second_reply_after_the_first_ended_is_held_again() {
    let gate = gate_on();
    let mut hub = live();
    assert!(begin(&mut hub, &gate, 1, 0));
    add(&mut hub, &gate, 1, BubbleRole::Assistant, "Turn one.", 0);
    hub.speech_end(1, 5_000);
    assert!(begin(&mut hub, &gate, 2, 5_500));
    add(
        &mut hub,
        &gate,
        2,
        BubbleRole::Assistant,
        "Turn two.",
        6_000,
    );
    assert_eq!(ttl_of(&hub, 2, 6_000), HOLD_MAX_MS);
    // Turn one's linger ended on its own while turn two is still being spoken.
    assert_eq!(ids(&hub, 30_000), vec![2]);
    hub.speech_end(2, 30_000);
    assert_eq!(ttl_of(&hub, 2, 30_000), LINGER_MS);
}
