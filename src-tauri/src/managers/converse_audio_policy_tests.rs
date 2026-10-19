//! AUTHORITATIVE contract tests for the Smart Turn endpoint policy
//! (DESIGN.md D3c, AMENDMENTS-R3 A4'). Written before implementation (T5, red).
//!
//! Timeline used throughout: feeds on a 50 ms grid from t=0; speech-active
//! feeds at t=0..=1000 so `last_loud` = 1000; `silence_ms` = 1200 unless a
//! test says otherwise; `min_speech_ms` = 150.

use super::*;

const POLICY: Option<SmartTurnPolicy> = Some(SmartTurnPolicy {
    min_silence_ms: 200,
    incomplete_wait_ms: 2500,
});

const NTD0: EndpointEvent = EndpointEvent::NeedTurnDecision { pause_id: 0 };
const NTD1: EndpointEvent = EndpointEvent::NeedTurnDecision { pause_id: 1 };
const ENDED: EndpointEvent = EndpointEvent::SpeechEnded;

fn config(silence_ms: u64, policy: Option<SmartTurnPolicy>) -> EndpointConfig {
    let mut cfg = EndpointConfig::with_silence(silence_ms);
    cfg.smart_turn = policy;
    cfg
}

fn endpointer(policy: Option<SmartTurnPolicy>) -> Endpointer {
    Endpointer::new(config(1200, policy))
}

/// Feed a constant speech-activity flag every 50 ms over [from_ms, to_ms)
/// (both bounds on the 50 ms grid). Returns every event with its timestamp.
fn feed_range(
    e: &mut Endpointer,
    from_ms: u64,
    to_ms: u64,
    active: bool,
) -> Vec<(u64, EndpointEvent)> {
    let mut events = Vec::new();
    let mut t = from_ms;
    while t < to_ms {
        if let Some(ev) = e.feed_activity(t, active) {
            events.push((t, ev));
        }
        t += 50;
    }
    events
}

/// Speech-active feeds for t = 0..=1000 (so `last_loud` = 1000).
fn speak_until_1000(e: &mut Endpointer) {
    assert_eq!(feed_range(e, 0, 1050, true), vec![]);
}

// ---------------------------------------------------------------------
// Configuration surface
// ---------------------------------------------------------------------

#[test]
fn default_policy_is_200_ms_minimum_silence_and_2500_ms_incomplete_wait() {
    assert_eq!(
        SmartTurnPolicy::default_policy(),
        SmartTurnPolicy {
            min_silence_ms: 200,
            incomplete_wait_ms: 2500,
        }
    );
}

// ---------------------------------------------------------------------
// (1) policy None vs policy Some with answer_unavailable: same fixed endpoint
// ---------------------------------------------------------------------

#[test]
fn policy_none_ends_exactly_at_2200_and_emits_nothing_else() {
    let mut e = endpointer(None);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 2600, false), vec![(2200, ENDED)]);
}

#[test]
fn policy_some_with_unavailable_answer_ends_at_the_same_2200() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1201, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_unavailable(0, 1200), None);
    assert_eq!(feed_range(&mut e, 1250, 2600, false), vec![(2200, ENDED)]);
}

// ---------------------------------------------------------------------
// (2) NeedTurnDecision exactly once, at last_loud + 200
// ---------------------------------------------------------------------

#[test]
fn need_turn_decision_fires_once_at_exactly_min_silence_after_last_loud() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    // Pending (no answer) through 1950: exactly one NeedTurnDecision, no end.
    assert_eq!(feed_range(&mut e, 1050, 1951, false), vec![(1200, NTD0)]);
}

// ---------------------------------------------------------------------
// (3) answer done=true ends immediately, then everything is inert
// ---------------------------------------------------------------------

#[test]
fn answer_done_true_ends_at_the_decision_tick_and_finishes() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_turn(0, true, 1250), Some(ENDED));
    assert_eq!(feed_range(&mut e, 1250, 3000, false), vec![]);
    assert_eq!(e.answer_turn(0, true, 1300), None);
    assert_eq!(e.answer_turn(0, false, 1300), None);
    assert_eq!(e.answer_unavailable(0, 1300), None);
    assert_eq!(e.answer_turn(1, true, 1300), None);
}

// ---------------------------------------------------------------------
// (4) answer done=false: ends at last_loud + max(incomplete_wait, silence)
// ---------------------------------------------------------------------

#[test]
fn answer_done_false_waits_until_max_of_incomplete_and_silence() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_turn(0, false, 1250), None);
    // 1000 + max(2500, 1200) = 3500 and nothing earlier.
    assert_eq!(feed_range(&mut e, 1250, 3600, false), vec![(3500, ENDED)]);
}

// ---------------------------------------------------------------------
// (5) answer_unavailable: today's fixed rule, SpeechEnded at 2200
// ---------------------------------------------------------------------

#[test]
fn answer_unavailable_keeps_the_fixed_endpoint_at_2200() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_unavailable(0, 1250), None);
    assert_eq!(feed_range(&mut e, 1250, 2600, false), vec![(2200, ENDED)]);
}

// ---------------------------------------------------------------------
// (6) no answer at all while pending: SpeechEnded at 2200
// ---------------------------------------------------------------------

#[test]
fn no_answer_at_all_still_ends_at_2200() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(
        feed_range(&mut e, 1050, 2600, false),
        vec![(1200, NTD0), (2200, ENDED)]
    );
}

// ---------------------------------------------------------------------
// (7) stale pause: new speech cancels the pause; old pause_id is ignored
// ---------------------------------------------------------------------

#[test]
fn speech_resuming_cancels_the_pause_and_the_next_pause_gets_a_new_id() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(feed_range(&mut e, 1250, 1300, false), vec![]);
    // Active feed at 1300 => last_loud = 1300 and the pause is cancelled.
    assert_eq!(feed_range(&mut e, 1300, 1350, true), vec![]);
    // Next pause: NeedTurnDecision for pause 1 at 1300 + 200 = 1500.
    assert_eq!(feed_range(&mut e, 1350, 1550, false), vec![(1500, NTD1)]);
    assert_eq!(e.answer_turn(0, true, 1550), None, "stale pause_id");
    assert_eq!(e.answer_turn(1, true, 1550), Some(ENDED));
}

// ---------------------------------------------------------------------
// (8) min_speech not satisfied: no NeedTurnDecision, NoSpeech unchanged
// ---------------------------------------------------------------------

#[test]
fn single_blip_below_min_speech_never_asks_and_is_no_speech_at_4000() {
    let expected = vec![(4000, EndpointEvent::NoSpeech)];
    let mut with_policy = endpointer(POLICY);
    assert_eq!(with_policy.feed_activity(0, true), None);
    assert_eq!(feed_range(&mut with_policy, 50, 4100, false), expected);

    let mut plain = endpointer(None);
    assert_eq!(plain.feed_activity(0, true), None);
    assert_eq!(feed_range(&mut plain, 50, 4100, false), expected);
}

// ---------------------------------------------------------------------
// (9) inert boundaries: policy active only when silence_ms >= min + 250
// ---------------------------------------------------------------------

#[test]
fn silence_449_with_min_200_is_inert_and_identical_to_none() {
    let mut with_policy = Endpointer::new(config(449, POLICY));
    let mut plain = Endpointer::new(config(449, None));
    speak_until_1000(&mut with_policy);
    speak_until_1000(&mut plain);
    let got = feed_range(&mut with_policy, 1050, 2100, false);
    let want = feed_range(&mut plain, 1050, 2100, false);
    assert_eq!(got, want);
    // first feed >= 1000 + 449 is 1450
    assert_eq!(got, vec![(1450, ENDED)]);
}

#[test]
fn silence_450_with_min_200_is_active() {
    let mut e = Endpointer::new(config(450, POLICY));
    speak_until_1000(&mut e);
    // NeedTurnDecision at 1200; with no answer the fixed endpoint is 1000+450.
    assert_eq!(
        feed_range(&mut e, 1050, 1600, false),
        vec![(1200, NTD0), (1450, ENDED)]
    );
}

#[test]
fn silence_201_with_incomplete_201_is_inert_and_identical_to_none() {
    let policy = Some(SmartTurnPolicy {
        min_silence_ms: 200,
        incomplete_wait_ms: 201,
    });
    let mut with_policy = Endpointer::new(config(201, policy));
    let mut plain = Endpointer::new(config(201, None));
    speak_until_1000(&mut with_policy);
    speak_until_1000(&mut plain);
    let got = feed_range(&mut with_policy, 1050, 1400, false);
    assert_eq!(got, feed_range(&mut plain, 1050, 1400, false));
    // first feed >= 1000 + 201 is 1250
    assert_eq!(got, vec![(1250, ENDED)]);
}

// ---------------------------------------------------------------------
// (10) incomplete_wait < silence: done=false is raised to silence_ms
// ---------------------------------------------------------------------

#[test]
fn incomplete_wait_below_silence_is_raised_to_silence_ms() {
    let policy = Some(SmartTurnPolicy {
        min_silence_ms: 200,
        incomplete_wait_ms: 800,
    });
    let mut e = Endpointer::new(config(1200, policy));
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_turn(0, false, 1250), None);
    // max(800, 1200) = 1200 after last_loud => 2200, NOT 1800.
    assert_eq!(feed_range(&mut e, 1250, 2600, false), vec![(2200, ENDED)]);
}

// ---------------------------------------------------------------------
// (11) timing origin is last_loud (1000), not the first inactive feed (1050)
// ---------------------------------------------------------------------

#[test]
fn pause_origin_is_last_loud_not_the_first_inactive_feed() {
    let mut with_policy = endpointer(POLICY);
    let mut plain = endpointer(None);
    speak_until_1000(&mut with_policy);
    speak_until_1000(&mut plain);
    // The policy's pause decision is at last_loud + 200 = 1200, not 1250.
    assert_eq!(
        feed_range(&mut with_policy, 1050, 1201, false),
        vec![(1200, NTD0)]
    );
    assert_eq!(with_policy.answer_unavailable(0, 1200), None);
    let got = feed_range(&mut with_policy, 1250, 2600, false);
    let want = feed_range(&mut plain, 1050, 2600, false);
    assert_eq!(want, vec![(2200, ENDED)]);
    assert_eq!(got, want);
}

// ---------------------------------------------------------------------
// (12) MaxLength still fires under continuous activity with a policy
// ---------------------------------------------------------------------

#[test]
fn max_length_still_fires_at_30_seconds_with_policy_on() {
    let mut e = endpointer(POLICY);
    assert_eq!(
        feed_range(&mut e, 0, 30_050, true),
        vec![(30_000, EndpointEvent::MaxLength)]
    );
}

// ---------------------------------------------------------------------
// (13) answers after the fixed endpoint already ended => None
// ---------------------------------------------------------------------

#[test]
fn answers_after_the_fixed_endpoint_ended_are_none() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(
        feed_range(&mut e, 1050, 2300, false),
        vec![(1200, NTD0), (2200, ENDED)]
    );
    assert_eq!(e.answer_turn(0, true, 2250), None);
    assert_eq!(e.answer_turn(0, false, 2250), None);
    assert_eq!(e.answer_unavailable(0, 2250), None);
}

// ---------------------------------------------------------------------
// (14) feeding beyond the end never returns another event
// ---------------------------------------------------------------------

#[test]
fn feeding_beyond_the_fixed_end_never_returns_another_event() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(
        feed_range(&mut e, 1050, 2300, false),
        vec![(1200, NTD0), (2200, ENDED)]
    );
    assert_eq!(feed_range(&mut e, 2300, 10_000, false), vec![]);
    assert_eq!(feed_range(&mut e, 10_000, 40_000, true), vec![]);
}

#[test]
fn feeding_beyond_an_early_done_true_end_never_returns_another_event() {
    let mut e = endpointer(POLICY);
    speak_until_1000(&mut e);
    assert_eq!(feed_range(&mut e, 1050, 1250, false), vec![(1200, NTD0)]);
    assert_eq!(e.answer_turn(0, true, 1250), Some(ENDED));
    assert_eq!(feed_range(&mut e, 1250, 10_000, true), vec![]);
    assert_eq!(feed_range(&mut e, 10_000, 40_000, false), vec![]);
}

// ---------------------------------------------------------------------
// (15) property-style sweep over silence, answer kind and first-feed offset
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Answer {
    Done,
    NotDone,
    Unavailable,
    NoAnswer,
}

const ANSWERS: [Answer; 4] = [
    Answer::Done,
    Answer::NotDone,
    Answer::Unavailable,
    Answer::NoAnswer,
];

/// Returns the timestamp at which SpeechEnded fired, if any.
fn run_sweep_case(silence_ms: u64, answer: Answer, offset: u64) -> Option<u64> {
    let policy = SmartTurnPolicy::default_policy();
    let mut e = Endpointer::new(config(silence_ms, Some(policy)));
    // Speech-active feeds from `offset` up to `last_loud`, then silence.
    let last_loud = offset + 1000;
    let mut t = offset;
    while t <= last_loud + 6000 {
        let event = e.feed_activity(t, t <= last_loud);
        let ended = match event {
            Some(EndpointEvent::NeedTurnDecision { pause_id }) => {
                let answered = match answer {
                    Answer::Done => e.answer_turn(pause_id, true, t),
                    Answer::NotDone => e.answer_turn(pause_id, false, t),
                    Answer::Unavailable => e.answer_unavailable(pause_id, t),
                    Answer::NoAnswer => None,
                };
                answered == Some(ENDED)
            }
            other => other == Some(ENDED),
        };
        if ended {
            return Some(t);
        }
        t += 50;
    }
    None
}

#[test]
fn sweep_speech_ended_is_bounded_by_policy_for_every_answer_and_offset() {
    for silence_ms in [600u64, 1200, 4000] {
        for answer in ANSWERS {
            for offset in 0..50u64 {
                let policy = SmartTurnPolicy::default_policy();
                let last_loud = offset + 1000;
                let case = format!("silence={silence_ms} answer={answer:?} offset={offset}");
                let end = match run_sweep_case(silence_ms, answer, offset) {
                    Some(end) => end,
                    None => panic!("{case}: SpeechEnded never fired"),
                };
                let earliest = last_loud + policy.min_silence_ms;
                let latest = last_loud + policy.incomplete_wait_ms.max(silence_ms) + 50;
                assert!(end >= earliest, "{case}: ended at {end} < {earliest}");
                assert!(end <= latest, "{case}: ended at {end} > {latest}");
            }
        }
    }
}
