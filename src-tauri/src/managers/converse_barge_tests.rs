//! Stage B1 spec (DESIGN.md Rev 8, section 12.2.4 / 12.4): the pure barge-in
//! policy. Written BEFORE the implementation; these tests are authoritative.
//!
//! Time model: frame `n` (1-based) ends at `now_ms = n * 20`. Every frame carries
//! 320 samples (20 ms @ 16 kHz) all equal to its frame number, so the exact audio
//! handed back in `Done` can be audited frame by frame.

use crate::managers::converse_barge::{
    BargePolicy, PolicyConfig, PolicyEvent, PolicyState, COOLDOWN_MS, GAP_MS, PREROLL_MS,
    SUSTAIN_MS,
};

const FRAME_MS: u64 = 20;
const FRAME_SAMPLES: usize = 320;

fn cfg() -> PolicyConfig {
    PolicyConfig {
        endpoint_silence_ms: 400,
        max_utterance_ms: 5000,
        frame_ms: FRAME_MS,
    }
}

struct Drive {
    policy: BargePolicy,
    now: u64,
    n: u32,
}

impl Drive {
    fn new() -> Self {
        Self::with(cfg())
    }

    fn with(cfg: PolicyConfig) -> Self {
        Self {
            policy: BargePolicy::new(cfg),
            now: 0,
            n: 0,
        }
    }

    fn frame(&mut self, voiced: bool, speaking: bool) -> (u32, Vec<PolicyEvent>) {
        self.now += FRAME_MS;
        self.n += 1;
        let samples = vec![self.n as f32; FRAME_SAMPLES];
        let events = self.policy.push_frame(self.now, voiced, speaking, &samples);
        (self.n, events)
    }

    fn missing(&mut self) -> (u32, Vec<PolicyEvent>) {
        self.now += FRAME_MS;
        self.n += 1;
        (self.n, self.policy.push_missing(self.now))
    }

    /// Feed `count` frames and return every event tagged with its frame number.
    fn run(&mut self, count: u32, voiced: bool, speaking: bool) -> Vec<(u32, PolicyEvent)> {
        let mut out = Vec::new();
        for _ in 0..count {
            let (n, events) = self.frame(voiced, speaking);
            out.extend(events.into_iter().map(|e| (n, e)));
        }
        out
    }
}

fn frames_of(samples: &[f32]) -> Vec<u32> {
    assert_eq!(samples.len() % FRAME_SAMPLES, 0, "whole frames only");
    samples
        .chunks(FRAME_SAMPLES)
        .map(|c| c[0] as u32)
        .collect()
}

fn done_frames(events: &[(u32, PolicyEvent)]) -> Option<(u32, Vec<u32>)> {
    events.iter().find_map(|(n, e)| match e {
        PolicyEvent::Done(samples) => Some((*n, frames_of(samples))),
        _ => None,
    })
}

fn kinds(events: &[(u32, PolicyEvent)]) -> Vec<(u32, &'static str)> {
    events
        .iter()
        .map(|(n, e)| {
            let k = match e {
                PolicyEvent::Duck => "duck",
                PolicyEvent::Restore => "restore",
                PolicyEvent::CancelReply => "cancel",
                PolicyEvent::Done(_) => "done",
            };
            (*n, k)
        })
        .collect()
}

#[test]
fn constants_match_the_design() {
    assert_eq!(SUSTAIN_MS, 600);
    assert_eq!(GAP_MS, 250);
    assert_eq!(COOLDOWN_MS, 300);
    assert_eq!(PREROLL_MS, 300);
}

#[test]
fn idle_ignores_voiced_audio_while_the_assistant_is_not_speaking() {
    let mut d = Drive::new();
    assert!(d.run(50, true, false).is_empty());
    assert_eq!(d.policy.state(), PolicyState::Idle);
}

#[test]
fn quiet_while_speaking_arms_without_events() {
    let mut d = Drive::new();
    assert!(d.run(100, false, true).is_empty());
    assert_eq!(d.policy.state(), PolicyState::Armed);
}

#[test]
fn backchannel_ducks_then_restores_and_never_cancels() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true); // frames 1..10
    ev.extend(d.run(15, true, true)); // frames 11..25 = 300 ms of speech
    ev.extend(d.run(20, false, true)); // frames 26..45
                                       // Duck at the first voiced frame; the gap exceeds 250 ms at the 13th silent
                                       // frame (260 ms after the last voiced frame, frame 25 -> frame 38).
    assert_eq!(kinds(&ev), vec![(11, "duck"), (38, "restore")]);
    assert_eq!(d.policy.state(), PolicyState::Cooldown);
}

#[test]
fn cooldown_ends_and_the_policy_rearms() {
    let mut d = Drive::new();
    d.run(10, false, true);
    d.run(15, true, true);
    d.run(13, false, true); // restore at frame 38 (now = 760)
    assert_eq!(d.policy.state(), PolicyState::Cooldown);
    d.run(20, false, true); // 400 ms of quiet passes the 300 ms cooldown
    assert_eq!(d.policy.state(), PolicyState::Armed);
}

#[test]
fn sustained_speech_cancels_the_reply_at_exactly_sustain_ms() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true); // 1..10
    ev.extend(d.run(40, true, true)); // 11..50 (800 ms)
                                      // t0 = frame 11 (now 220); now - t0 >= 600 first holds at frame 41 (now 820).
    assert_eq!(kinds(&ev), vec![(11, "duck"), (41, "cancel")]);
    assert_eq!(d.policy.state(), PolicyState::Capturing);
}

#[test]
fn a_short_gap_is_bridged_and_counts_as_elapsed_time() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true); // 1..10
    ev.extend(d.run(10, true, true)); // 11..20
    ev.extend(d.run(10, false, true)); // 21..30: gap of 200 ms at most, bridged
    ev.extend(d.run(30, true, true)); // 31..60
    assert_eq!(kinds(&ev), vec![(11, "duck"), (41, "cancel")]);
}

#[test]
fn a_long_gap_restores_and_the_cooldown_blocks_an_immediate_reduck() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true); // 1..10
    ev.extend(d.run(10, true, true)); // 11..20 (last voiced now = 400)
    ev.extend(d.run(13, false, true)); // 21..33: restore at 33 (now 660, gap 260)
    ev.extend(d.run(14, true, true)); // 34..47: inside the 300 ms cooldown (until now 960)
    assert_eq!(kinds(&ev), vec![(11, "duck"), (33, "restore")]);
    let after = d.run(1, true, true); // frame 48, now = 960: cooldown over
    assert_eq!(kinds(&after), vec![(48, "duck")]);
    assert_eq!(d.policy.state(), PolicyState::Ducked);
}

#[test]
fn speaking_ending_while_ducked_restores_and_goes_idle() {
    let mut d = Drive::new();
    d.run(3, false, true);
    let ev = d.run(5, true, true);
    assert_eq!(kinds(&ev), vec![(4, "duck")]);
    let (_, ended) = d.frame(false, false);
    assert_eq!(ended, vec![PolicyEvent::Restore]);
    assert_eq!(d.policy.state(), PolicyState::Idle);
}

#[test]
fn speaking_ending_while_armed_goes_idle_silently() {
    let mut d = Drive::new();
    d.run(5, false, true);
    assert!(d.run(5, false, false).is_empty());
    assert_eq!(d.policy.state(), PolicyState::Idle);
}

#[test]
fn capture_continues_after_cancel_even_though_speaking_turns_false() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true); // 1..10
    ev.extend(d.run(40, true, true)); // 11..50, cancel at 41
                                      // Our own stop silences the assistant: speaking is false from here on.
    ev.extend(d.run(10, true, false)); // 51..60, still voiced
    ev.extend(d.run(40, false, false)); // 61..100 silence
                                        // last voiced = frame 60 (now 1200); endpoint after 400 ms of silence -> frame 80.
    assert_eq!(kinds(&ev), vec![(11, "duck"), (41, "cancel"), (80, "done")]);
    assert_eq!(d.policy.state(), PolicyState::Finished);
}

#[test]
fn done_contains_every_frame_from_the_preroll_boundary_through_the_endpoint() {
    let mut d = Drive::new();
    let mut ev = d.run(25, false, true); // 1..25
    ev.extend(d.run(41, true, true)); // 26..66
    ev.extend(d.run(30, false, true)); // 67..96
                                       // PREROLL_MS / 20 = 15 frames of lead-in: 11..25. Endpoint: last voiced 66
                                       // (now 1320) + 400 ms -> frame 86.
    let (at, frames) = done_frames(&ev).expect("capture must finish");
    assert_eq!(at, 86);
    assert_eq!(frames, (11..=86).collect::<Vec<_>>());
}

#[test]
fn a_bridged_gap_inside_the_utterance_is_kept_in_the_buffer() {
    let mut d = Drive::new();
    let mut ev = d.run(25, false, true); // 1..25
    ev.extend(d.run(10, true, true)); // 26..35
    ev.extend(d.run(10, false, true)); // 36..45 (200 ms, bridged)
    ev.extend(d.run(25, true, true)); // 46..70
    ev.extend(d.run(30, false, true)); // 71..100
                                       // last voiced = 70 (now 1400); endpoint at 1800 -> frame 90; nothing is dropped.
    let (at, frames) = done_frames(&ev).expect("capture must finish");
    assert_eq!(at, 90);
    assert_eq!(frames, (11..=90).collect::<Vec<_>>());
    assert!(kinds(&ev).contains(&(56, "cancel")));
}

#[test]
fn the_lead_in_ring_is_bounded_by_preroll() {
    let mut d = Drive::new();
    let mut ev = d.run(1000, false, true); // 1..1000
    ev.extend(d.run(41, true, true)); // 1001..1041
    ev.extend(d.run(30, false, true));
    let (_, frames) = done_frames(&ev).expect("capture must finish");
    assert_eq!(frames.first(), Some(&986)); // 15 lead-in frames: 986..=1000
}

#[test]
fn a_restored_backchannel_does_not_leak_into_the_next_capture() {
    let mut d = Drive::new();
    d.run(10, false, true); // 1..10
    d.run(15, true, true); // 11..25 "yeah"
    d.run(13, false, true); // 26..38 restore
    let mut ev = d.run(40, false, true); // 39..78 quiet
    ev.extend(d.run(41, true, true)); // 79..119 real interruption
    ev.extend(d.run(30, false, true));
    let (_, frames) = done_frames(&ev).expect("capture must finish");
    assert_eq!(frames.first(), Some(&64)); // 15 frames before onset 79, none of the old "yeah"
}

#[test]
fn max_utterance_caps_the_capture() {
    let mut d = Drive::with(PolicyConfig {
        endpoint_silence_ms: 400,
        max_utterance_ms: 2000,
        frame_ms: FRAME_MS,
    });
    let mut ev = d.run(10, false, true); // 1..10
    ev.extend(d.run(200, true, true)); // never stops talking
                                       // t0 = frame 11 (now 220); now - t0 >= 2000 first holds at frame 111.
    let (at, frames) = done_frames(&ev).expect("must be capped");
    assert_eq!(at, 111);
    assert_eq!(frames, (1..=111).collect::<Vec<_>>());
}

#[test]
fn missing_frames_can_never_complete_a_sustain_window() {
    let mut d = Drive::new();
    d.run(10, false, true);
    let mut all = Vec::new();
    for _ in 0..10 {
        all.extend(d.run(10, true, true)); // 200 ms of speech...
        let (n, ev) = d.missing(); // ...then a lost frame resets continuity
        all.extend(ev.into_iter().map(|e| (n, e)));
    }
    assert!(
        !kinds(&all).iter().any(|(_, k)| *k == "cancel"),
        "a stalled or overflowing stream must never look like sustained speech: {all:?}"
    );
}

#[test]
fn a_missing_frame_while_ducked_restores() {
    let mut d = Drive::new();
    d.run(3, false, true);
    d.run(5, true, true); // ducked
    let (_, ev) = d.missing();
    assert_eq!(ev, vec![PolicyEvent::Restore]);
    assert_eq!(d.policy.state(), PolicyState::Cooldown);
}

#[test]
fn a_missing_frame_while_capturing_does_not_abort_the_capture() {
    let mut d = Drive::new();
    d.run(10, false, true);
    d.run(40, true, true); // cancel at 41
    assert_eq!(d.policy.state(), PolicyState::Capturing);
    let (_, ev) = d.missing();
    assert!(ev.is_empty());
    assert_eq!(d.policy.state(), PolicyState::Capturing);
}

#[test]
fn after_done_the_policy_ignores_everything() {
    let mut d = Drive::new();
    d.run(10, false, true);
    d.run(40, true, true);
    let ev = d.run(60, false, false); // endpoint
    assert!(done_frames(&ev).is_some());
    assert_eq!(d.policy.state(), PolicyState::Finished);
    assert!(d.run(30, true, true).is_empty());
    assert_eq!(d.policy.state(), PolicyState::Finished);
}

#[test]
fn after_cancel_only_done_is_ever_emitted() {
    let mut d = Drive::new();
    let mut ev = d.run(10, false, true);
    ev.extend(d.run(40, true, true));
    ev.extend(d.run(30, false, true));
    ev.extend(d.run(30, true, true));
    let k = kinds(&ev);
    let after_cancel: Vec<_> = k
        .iter()
        .skip_while(|(_, k)| *k != "cancel")
        .skip(1)
        .collect();
    assert!(
        after_cancel.iter().all(|(_, k)| *k == "done"),
        "no Duck/Restore/second Cancel after CancelReply: {k:?}"
    );
}
