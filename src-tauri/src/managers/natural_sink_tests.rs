//! Spec for `natural_sink` (step S2). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it. Covers critic findings C1, C3 (incl. round 3), C5.
use crate::managers::natural_sink::*;
use crate::managers::natural_test_support::*;
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---- PlaybackCtl: ordering, generations ----------------------------------------------------

#[test]
fn commit_then_render_outputs_samples_in_order_then_zeros() {
    let ctl = PlaybackCtl::new(1000);
    let g = ctl.generation();
    assert_eq!(ctl.commit(g, vec![0.1, 0.2, 0.3]), CommitResult::Accepted);
    assert_eq!(ctl.commit(g, vec![0.4]), CommitResult::Accepted);
    let mut out = [9.0f32; 6];
    ctl.render(&mut out, Instant::now());
    assert_eq!(out, [0.1, 0.2, 0.3, 0.4, 0.0, 0.0]);
    assert_eq!(ctl.queued_secs(), 0.0);
}

#[test]
fn stale_generation_is_rejected_and_never_rendered() {
    let ctl = PlaybackCtl::new(1000);
    let g0 = ctl.generation();
    let g1 = ctl.invalidate();
    assert!(g1 > g0);
    assert_eq!(ctl.commit(g0, vec![0.5; 10]), CommitResult::Stale);
    let mut out = [9.0f32; 4];
    ctl.render(&mut out, Instant::now());
    assert_eq!(out, [0.0; 4]);
}

#[test]
fn invalidate_clears_queued_audio() {
    let ctl = PlaybackCtl::new(1000);
    let g = ctl.generation();
    assert_eq!(ctl.commit(g, vec![0.5; 500]), CommitResult::Accepted);
    assert!(ctl.queued_secs() > 0.4);
    ctl.invalidate();
    assert_eq!(ctl.queued_secs(), 0.0);
    let mut out = [9.0f32; 4];
    ctl.render(&mut out, Instant::now());
    assert_eq!(out, [0.0; 4]);
}

#[test]
fn barrier_race_never_leaks_old_generation_audio() {
    // C1: a commit validated against generation g racing with invalidate() must either be
    // rejected or be cleared; old audio must never be playable after invalidate() returned.
    for _ in 0..300 {
        let ctl = Arc::new(PlaybackCtl::new(48_000));
        let g = ctl.generation();
        let gate = Arc::new(Barrier::new(2));
        let committer = {
            let (ctl, gate) = (ctl.clone(), gate.clone());
            std::thread::spawn(move || {
                gate.wait();
                ctl.commit(g, vec![0.5; 4_800])
            })
        };
        let stopper = {
            let (ctl, gate) = (ctl.clone(), gate.clone());
            std::thread::spawn(move || {
                gate.wait();
                ctl.invalidate();
                // After invalidate has returned, committing the old generation is always Stale.
                assert_eq!(ctl.commit(g, vec![0.5; 4_800]), CommitResult::Stale);
            })
        };
        committer.join().unwrap();
        stopper.join().unwrap();
        let mut out = vec![0.0f32; 9_600];
        ctl.render(&mut out, Instant::now());
        assert!(out.iter().all(|x| *x == 0.0), "old-generation audio leaked");
    }
}

#[test]
fn full_when_the_queue_bound_would_be_exceeded() {
    let ctl = PlaybackCtl::new(1000); // 1 kHz: 1000 samples = 1 s
    let g = ctl.generation();
    assert_eq!(ctl.commit(g, vec![0.1; 25_000]), CommitResult::Full); // can never fit
    assert_eq!(ctl.commit(g, vec![0.1; 15_000]), CommitResult::Accepted);
    assert_eq!(ctl.commit(g, vec![0.1; 6_000]), CommitResult::Full); // 21 s > 20 s
    let mut out = vec![0.0f32; 6_000];
    ctl.render(&mut out, Instant::now());
    assert_eq!(ctl.commit(g, vec![0.1; 6_000]), CommitResult::Accepted);
}

// ---- audible tail, truthful flush ----------------------------------------------------------

#[test]
fn audible_while_queued_and_for_the_tail_after_the_last_audible_callback() {
    let ctl = PlaybackCtl::new(48_000);
    ctl.set_tail_bound(ms(300));
    assert_eq!(ctl.tail_bound(), ms(300));
    let g = ctl.generation();
    assert!(!ctl.audible(Instant::now()));
    ctl.commit(g, vec![0.5; 480]);
    assert!(ctl.audible(Instant::now()), "queued audio is audible");
    let t0 = Instant::now();
    let mut out = [0.0f32; 480]; // 10 ms
    ctl.render(&mut out, t0);
    assert!(ctl.audible(t0 + ms(300)));
    assert!(!ctl.audible(t0 + ms(315)));
    // A later callback that only emits silence must not extend the tail.
    let t1 = t0 + ms(400);
    ctl.render(&mut out, t1);
    assert!(!ctl.audible(t1));
}

fn spawn_callbacks(
    ctl: Arc<PlaybackCtl>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            let mut out = [0.0f32; 96]; // 2 ms at 48 kHz
            ctl.render(&mut out, Instant::now());
            std::thread::sleep(ms(2));
        }
    })
}

#[test]
fn flush_is_unconfirmed_when_the_tail_is_longer_than_the_budget_even_with_live_callbacks() {
    // C3: success must never be reported while old audio may still be audible.
    let ctl = Arc::new(PlaybackCtl::new(48_000));
    ctl.set_tail_bound(ms(300));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cb = spawn_callbacks(ctl.clone(), stop.clone());
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 9_600]);
    std::thread::sleep(ms(30)); // some audible output has happened
    let started = Instant::now();
    assert_eq!(ctl.flush(ms(180)), FlushOutcome::Unconfirmed);
    assert!(started.elapsed() < ms(300));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    cb.join().unwrap();
}

#[test]
fn flush_confirms_within_the_budget_when_the_tail_is_short() {
    let ctl = Arc::new(PlaybackCtl::new(48_000));
    ctl.set_tail_bound(ms(30));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cb = spawn_callbacks(ctl.clone(), stop.clone());
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 9_600]);
    std::thread::sleep(ms(30));
    let started = Instant::now();
    assert_eq!(ctl.flush(ms(180)), FlushOutcome::Confirmed);
    assert!(started.elapsed() <= ms(230), "took {:?}", started.elapsed());
    assert!(!ctl.audible(Instant::now()));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    cb.join().unwrap();
}

#[test]
fn flush_confirms_immediately_when_nothing_was_ever_audible_even_without_callbacks() {
    let ctl = PlaybackCtl::new(48_000);
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 480]); // queued, never rendered, no callback thread at all
    let started = Instant::now();
    assert_eq!(ctl.flush(ms(100)), FlushOutcome::Confirmed);
    assert!(started.elapsed() < ms(50));
    assert_eq!(ctl.queued_secs(), 0.0);
}

#[test]
fn stalled_callbacks_stay_unconfirmed_then_recover_without_any_poll() {
    // C3 round 3: after callbacks died, a later flush must recover once the tail elapsed.
    let ctl = PlaybackCtl::new(48_000);
    ctl.set_tail_bound(ms(150));
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 480]);
    let mut out = [0.0f32; 480];
    ctl.render(&mut out, Instant::now()); // last audible callback, then the callbacks die
    assert_eq!(ctl.flush(ms(100)), FlushOutcome::Unconfirmed);
    std::thread::sleep(ms(120)); // the 150 ms tail is now over
    let started = Instant::now();
    assert_eq!(ctl.flush(ms(100)), FlushOutcome::Confirmed);
    assert!(started.elapsed() < ms(30));
}

// ---- failure and recovery (C5) --------------------------------------------------------------

#[test]
fn fail_clears_the_queue_rejects_commits_keeps_the_tail_and_recover_restores() {
    let ctl = PlaybackCtl::new(48_000);
    ctl.set_tail_bound(ms(100));
    let g0 = ctl.generation();
    ctl.commit(g0, vec![0.5; 480]);
    let t0 = Instant::now();
    let mut out = [0.0f32; 480];
    ctl.render(&mut out, t0); // audible_until = t0 + 10 ms + 100 ms
    ctl.commit(g0, vec![0.5; 4_800]); // still queued when the device dies
    ctl.fail();
    assert!(ctl.is_errored());
    assert_eq!(ctl.queued_secs(), 0.0);
    assert!(ctl.generation() > g0);
    assert_eq!(ctl.commit(ctl.generation(), vec![0.5; 10]), CommitResult::Errored);
    assert!(ctl.audible(t0 + ms(50)), "in-flight tail is still honest");
    assert!(!ctl.audible(t0 + ms(115)), "but bounded by the tail");
    ctl.recover();
    assert!(!ctl.is_errored());
    assert_eq!(ctl.commit(ctl.generation(), vec![0.5; 10]), CommitResult::Accepted);
    assert_eq!(ctl.commit(g0, vec![0.5; 10]), CommitResult::Stale);
}

// ---- callback contention and stall detection -------------------------------------------------

#[test]
fn render_never_blocks_when_the_lock_is_contended() {
    let ctl = Arc::new(PlaybackCtl::new(1000));
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 100]);
    let guard = ctl.lock_for_test();
    let (tx, rx) = mpsc::channel();
    let c2 = ctl.clone();
    std::thread::spawn(move || {
        let mut out = [9.0f32; 8];
        c2.render(&mut out, Instant::now());
        tx.send(out).unwrap();
    });
    let out = rx
        .recv_timeout(ms(200))
        .expect("render must not block on a contended lock");
    assert_eq!(out, [0.0; 8], "contended callback writes silence");
    drop(guard);
    let mut out = [9.0f32; 4];
    ctl.render(&mut out, Instant::now());
    assert_eq!(out, [0.5; 4], "queued audio is still there once the lock is free");
}

#[test]
fn stalled_needs_queued_audio_and_a_silent_callback() {
    let ctl = PlaybackCtl::new(1000);
    assert!(!ctl.stalled(Instant::now() + Duration::from_secs(10)), "nothing queued");
    let g = ctl.generation();
    ctl.commit(g, vec![0.5; 1_000]);
    assert!(!ctl.stalled(Instant::now() + ms(100)));
    assert!(ctl.stalled(Instant::now() + ms(600)));
    let t2 = Instant::now();
    let mut out = [0.0f32; 10];
    ctl.render(&mut out, t2); // callback alive, audio still queued
    assert!(!ctl.stalled(t2 + ms(100)));
    assert!(ctl.stalled(t2 + ms(600)));
}

// ---- device-side helpers ------------------------------------------------------------------------

#[test]
fn fill_interleaved_duplicates_mono_to_every_channel() {
    let ctl = PlaybackCtl::new(1000);
    let g = ctl.generation();
    ctl.commit(g, vec![0.1, 0.2, 0.3]);
    let mut out = [9.0f32; 8];
    fill_interleaved_f32(&ctl, &mut out, 2, Instant::now());
    assert_eq!(out, [0.1, 0.1, 0.2, 0.2, 0.3, 0.3, 0.0, 0.0]);
    ctl.commit(g, vec![0.5]);
    let mut six = [9.0f32; 6];
    fill_interleaved_f32(&ctl, &mut six, 6, Instant::now());
    assert_eq!(six, [0.5; 6]);
}

#[test]
fn fill_interleaved_with_zero_channels_is_a_noop() {
    let ctl = PlaybackCtl::new(1000);
    let mut out = [9.0f32; 4];
    fill_interleaved_f32(&ctl, &mut out, 0, Instant::now());
    assert_eq!(out, [9.0; 4]);
}

#[test]
fn sample_conversions_scale_clip_and_survive_nan() {
    assert_eq!(f32_to_i16(0.0), 0);
    assert_eq!(f32_to_i16(1.0), 32767);
    assert_eq!(f32_to_i16(2.0), 32767);
    assert!(f32_to_i16(-1.0) <= -32767);
    assert!(f32_to_i16(-2.0) <= -32767);
    assert_eq!(f32_to_i16(f32::NAN), 0);
    assert_eq!(f32_to_u16(0.0), 32768);
    assert_eq!(f32_to_u16(1.0), 65535);
    assert_eq!(f32_to_u16(2.0), 65535);
    assert_eq!(f32_to_u16(-1.0), 0);
    assert_eq!(f32_to_u16(f32::NAN), 32768);
}

// ---- ManagedSink over a fake stream factory -------------------------------------------------

fn managed(rate: u32, idle: Duration) -> (Arc<FakeStreamFactory>, ManagedSink) {
    let factory = FakeStreamFactory::new(rate);
    let sink = ManagedSink::new(factory.clone(), idle);
    (factory, sink)
}

#[test]
fn sink_opens_lazily_and_only_once() {
    let (factory, sink) = managed(48_000, Duration::from_secs(3));
    assert_eq!(factory.opens(), 0);
    assert!(!sink.audible());
    let g = sink.generation();
    assert_eq!(sink.commit(g, vec![0.5; 480]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 1);
    assert_eq!(sink.commit(g, vec![0.5; 480]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 1);
    assert!(sink.audible());
}

#[test]
fn sink_device_rate_comes_from_the_factory_and_errors_propagate() {
    let (factory, sink) = managed(44_100, Duration::from_secs(3));
    assert_eq!(sink.device_rate(), Ok(44_100));
    factory.set_fail_rate(true);
    assert!(sink.device_rate().is_err());
}

#[test]
fn sink_invalidate_makes_older_commits_stale() {
    let (_factory, sink) = managed(48_000, Duration::from_secs(3));
    let g = sink.generation();
    let g2 = sink.invalidate();
    assert!(g2 > g);
    assert_eq!(sink.commit(g, vec![0.5; 10]), CommitResult::Stale);
    assert_eq!(sink.commit(g2, vec![0.5; 10]), CommitResult::Accepted);
}

#[test]
fn sink_open_failure_is_errored_and_recovers_when_the_device_is_back() {
    let (factory, sink) = managed(48_000, Duration::from_secs(3));
    factory.set_fail_open(true);
    let g = sink.generation();
    assert_eq!(sink.commit(g, vec![0.5; 10]), CommitResult::Errored);
    assert!(sink.is_errored());
    factory.set_fail_open(false);
    sink.recover();
    assert!(!sink.is_errored());
    assert_eq!(sink.commit(sink.generation(), vec![0.5; 10]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 1);
}

#[test]
fn sink_device_error_with_queued_audio_fails_cleanly_and_the_stream_is_recreated() {
    // C5: no permanently busy route, no stale audio after Retry, stream re-created.
    let (factory, sink) = managed(48_000, Duration::from_secs(3));
    let g0 = sink.generation();
    assert_eq!(sink.commit(g0, vec![0.5; 48_000]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 1);
    factory.ctl().unwrap().fail(); // what the cpal error callback does
    assert!(sink.is_errored());
    assert_eq!(sink.commit(sink.generation(), vec![0.5; 10]), CommitResult::Errored);
    assert!(!sink.audible(), "queued audio of the dead stream is gone and nothing rendered");
    assert_eq!(sink.queued_secs(), 0.0);
    sink.recover();
    assert!(sink.generation() > g0);
    assert_eq!(sink.commit(g0, vec![0.5; 10]), CommitResult::Stale);
    assert_eq!(sink.commit(sink.generation(), vec![0.5; 10]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 2);
    assert!(factory.closes() >= 1);
}

#[test]
fn sink_maintain_closes_an_idle_stream_and_the_next_commit_reopens() {
    let (factory, sink) = managed(1_000, Duration::from_secs(1));
    let g = sink.generation();
    sink.commit(g, vec![0.5; 100]);
    let mut out = vec![0.0f32; 1_000];
    factory.ctl().unwrap().render(&mut out, Instant::now()); // device drains it
    sink.maintain(Instant::now() + ms(500));
    assert_eq!(factory.closes(), 0, "not idle long enough");
    sink.maintain(Instant::now() + Duration::from_secs(2));
    assert_eq!(factory.closes(), 1, "idle stream closed");
    assert_eq!(sink.commit(sink.generation(), vec![0.5; 100]), CommitResult::Accepted);
    assert_eq!(factory.opens(), 2);
}

#[test]
fn sink_maintain_keeps_the_stream_while_audio_is_queued() {
    let (factory, sink) = managed(1_000, Duration::from_secs(1));
    let g = sink.generation();
    sink.commit(g, vec![0.5; 5_000]); // 5 s queued
    let later = Instant::now() + Duration::from_secs(10);
    let mut out = [0.0f32; 10];
    factory.ctl().unwrap().render(&mut out, later - ms(100)); // callback alive
    sink.maintain(later);
    assert_eq!(factory.closes(), 0);
    assert!(!sink.is_errored());
}

#[test]
fn sink_maintain_flags_a_stalled_stream_as_a_device_failure() {
    let (factory, sink) = managed(1_000, Duration::from_secs(3));
    sink.commit(sink.generation(), vec![0.5; 1_000]);
    sink.maintain(Instant::now() + ms(600)); // queued audio, no callback for > 500 ms
    assert!(sink.is_errored());
    assert_eq!(factory.ctl().unwrap().queued_secs(), 0.0);
}

#[test]
fn fake_device_plays_what_the_sink_commits() {
    // Sanity for the shared test support the engine tests rely on.
    let (factory, sink) = managed(48_000, Duration::from_secs(3));
    let g = sink.generation();
    sink.commit(g, vec![0.5; 4_800]);
    let device = FakeDevice::start(factory.clone(), 96, ms(2));
    std::thread::sleep(ms(300));
    assert_eq!(device.nonzero(), 4_800);
}
