//! Spec for the playback duck (step B2). Every test builds its OWN `DuckState` and its own
//! `PlaybackCtl` through the test seam and never mutates the process-wide duck, so these tests
//! are safe to run in parallel with the rest of the `natural_*` suite. Time is driven through
//! the explicit `now` argument; nothing sleeps.
use crate::managers::natural_sink::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

const RAMP_TOL: f32 = 1e-3;

fn frames_for_ms(rate: u32, ms: u64) -> usize {
    (rate as usize * ms as usize) / 1000
}

/// A private duck and a control that follows it, preloaded with `samples`.
fn fixture(rate: u32, samples: Vec<f32>) -> (Arc<DuckState>, PlaybackCtl) {
    let duck = Arc::new(DuckState::new());
    let ctl = PlaybackCtl::with_duck_state(rate, duck.clone());
    let g = ctl.generation();
    assert_eq!(ctl.commit(g, samples), CommitResult::Accepted);
    (duck, ctl)
}

fn render_mono(ctl: &PlaybackCtl, frames: usize, now: Instant) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    ctl.render(&mut out, now);
    out
}

// Signature contract of the public free functions. They are NOT called here (the global duck
// is process-wide state); this only pins the names and types.
#[test]
fn public_duck_api_signatures() {
    let _: fn() = duck;
    let _: fn() = restore;
    let _: fn() -> f32 = duck_target_gain;
}

// (1) default is unity and rendering is bit-identical to the input

#[test]
fn default_gain_is_unity_and_render_is_bit_identical() {
    let input: Vec<f32> = (0..960)
        .map(|i| (i as f32 * 0.37).sin() * 0.9 - 0.05)
        .collect();
    let (duck, ctl) = fixture(48_000, input.clone());
    assert_eq!(duck.target_gain_at(Instant::now()), 1.0);
    let out = render_mono(&ctl, input.len(), Instant::now());
    assert_eq!(out, input);
}

// (2) duck ramps down over 20 ms of audio, then holds at 0.25

#[test]
fn duck_ramps_down_over_20ms_then_holds() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    let now = Instant::now();
    let (duck, ctl) = fixture(rate, vec![1.0; n + 500]);
    duck.duck_at(now);
    let out = render_mono(&ctl, n + 500, now);
    assert!(out[0] > 0.25 && out[0] < 1.0, "first sample {}", out[0]);
    assert!(
        (out[n - 1] - 0.25).abs() <= RAMP_TOL,
        "sample at 20 ms {}",
        out[n - 1]
    );
    for (i, s) in out[n..].iter().enumerate() {
        assert!((s - 0.25).abs() <= 1e-6, "later frame {i} = {s}");
    }
}

// (3) restore ramps back to unity within 20 ms, then renders bit-identical

#[test]
fn restore_ramps_back_within_20ms_then_bit_identical() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    let now = Instant::now();
    let (duck, ctl) = fixture(rate, vec![1.0; 3 * n + 200]);
    duck.duck_at(now);
    let _ = render_mono(&ctl, n, now);
    duck.restore();
    let ramp = render_mono(&ctl, n, now);
    assert!(
        (ramp[n - 1] - 1.0).abs() <= RAMP_TOL,
        "sample at 20 ms after restore {}",
        ramp[n - 1]
    );
    let tail = render_mono(&ctl, 100, now);
    assert!(tail.iter().all(|s| *s == 1.0), "tail not bit-identical");
}

// (4) every channel of a frame shares the same gain

#[test]
fn all_channels_of_a_frame_share_the_same_gain() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    for channels in [2usize, 3] {
        let now = Instant::now();
        let (duck, ctl) = fixture(rate, vec![1.0; n + 200]);
        duck.duck_at(now);
        let mut out = vec![0.0f32; (n + 200) * channels];
        fill_interleaved_f32(&ctl, &mut out, channels, now);
        for (f, frame) in out.chunks_exact(channels).enumerate() {
            assert!(
                frame.iter().all(|s| *s == frame[0]),
                "channels={channels} frame {f} differs: {frame:?}"
            );
        }
        assert!(out[0] > 0.25 && out[0] < 1.0, "first sample {}", out[0]);
        let last = out[(n - 1) * channels];
        assert!((last - 0.25).abs() <= RAMP_TOL, "last ducked sample {last}");
    }
}

// (5) watchdog: no heartbeat past the 3000 ms deadline reverts to unity; heartbeat renews

#[test]
fn watchdog_reverts_target_after_deadline_and_output_ramps_back() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    let t0 = Instant::now();
    let (duck, ctl) = fixture(rate, vec![1.0; 3 * n]);
    duck.duck_at(t0);
    let _ = render_mono(&ctl, n, t0);
    let late = t0 + Duration::from_millis(3001);
    let out = render_mono(&ctl, n, late);
    assert!(out[0] > 0.25 && out[0] < 1.0, "first sample {}", out[0]);
    assert!(
        (out[n - 1] - 1.0).abs() <= RAMP_TOL,
        "sample at 20 ms {}",
        out[n - 1]
    );
    assert_eq!(duck.target_gain_at(late), 1.0);
    assert_eq!(duck.target_gain_at(late + Duration::from_secs(10)), 1.0);
}

#[test]
fn duck_heartbeat_renews_the_watchdog_deadline() {
    let duck = DuckState::new();
    let t0 = Instant::now();
    duck.duck_at(t0);
    duck.duck_at(t0 + Duration::from_millis(2000));
    assert_eq!(duck.target_gain_at(t0 + Duration::from_millis(3500)), 0.25);
    assert_eq!(duck.target_gain_at(t0 + Duration::from_millis(5001)), 1.0);
}

// (6) duck on an idle sink does not panic; a later commit is rendered ducked

#[test]
fn duck_during_silence_then_commit_is_rendered_ducked() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    let now = Instant::now();
    let duck = Arc::new(DuckState::new());
    let ctl = PlaybackCtl::with_duck_state(rate, duck.clone());
    duck.duck_at(now);
    let silence = render_mono(&ctl, n, now);
    assert!(silence.iter().all(|s| *s == 0.0));
    let g = ctl.generation();
    assert_eq!(ctl.commit(g, vec![1.0; n]), CommitResult::Accepted);
    let out = render_mono(&ctl, n, now);
    assert!(
        out.iter().all(|s| (s - 0.25).abs() <= RAMP_TOL),
        "utterance after silence not ducked: first {}",
        out[0]
    );
}

// (7) restore without a prior duck is a no-op

#[test]
fn restore_without_duck_is_a_noop() {
    let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.11).cos() * 0.5).collect();
    let (duck, ctl) = fixture(24_000, input.clone());
    duck.restore();
    assert_eq!(duck.target_gain_at(Instant::now()), 1.0);
    assert_eq!(render_mono(&ctl, input.len(), Instant::now()), input);
}

// (8) the 20 ms ramp holds for 24 kHz and 48 kHz

#[test]
fn twenty_ms_ramp_holds_for_24k_and_48k() {
    for rate in [24_000u32, 48_000] {
        let n = frames_for_ms(rate, 20);
        let now = Instant::now();
        let (duck, ctl) = fixture(rate, vec![1.0; 2 * n + 100]);
        duck.duck_at(now);
        let down = render_mono(&ctl, n, now);
        assert!(
            down[0] > 0.25 && down[0] < 1.0,
            "rate {rate} first {}",
            down[0]
        );
        assert!(
            (down[n - 1] - 0.25).abs() <= RAMP_TOL,
            "rate {rate} ducked at 20 ms: {}",
            down[n - 1]
        );
        duck.restore();
        let up = render_mono(&ctl, n, now);
        assert!(
            (up[n - 1] - 1.0).abs() <= RAMP_TOL,
            "rate {rate} restored at 20 ms: {}",
            up[n - 1]
        );
    }
}

// (9) isolation: a private duck never leaks into the global, and the global never leaks into
// a private control. The global is only READ here (never ducked), so this is safe in parallel.

#[test]
fn private_duck_does_not_leak_into_global_playback() {
    let rate = 48_000;
    let n = frames_for_ms(rate, 20);
    let input: Vec<f32> = (0..n + 100)
        .map(|i| (i as f32 * 0.21).sin() * 0.7 - 0.02)
        .collect();

    let now = Instant::now();
    let (private, ducked_ctl) = fixture(rate, vec![1.0; n + 100]);
    private.duck_at(now);
    let ducked = render_mono(&ducked_ctl, n + 100, now);
    assert!(
        (ducked[n - 1] - 0.25).abs() <= RAMP_TOL,
        "private duck not applied: {}",
        ducked[n - 1]
    );

    let global_ctl = PlaybackCtl::new(rate);
    let g = global_ctl.generation();
    assert_eq!(global_ctl.commit(g, input.clone()), CommitResult::Accepted);
    let out = render_mono(&global_ctl, input.len(), Instant::now());
    assert_eq!(out, input, "global playback was affected by a private duck");
}
