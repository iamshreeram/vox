//! Spec for `natural_engine` (step S3). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it. Covers critic findings C1, C3, C5, C6, C9, C11.
use crate::managers::natural_audio::wpm_to_speed;
use crate::managers::natural_chunk::{split_for_synthesis, NATURAL_MAX_CHARS};
use crate::managers::natural_engine::*;
use crate::managers::natural_sink::ManagedSink;
use crate::managers::natural_test_support::{FakeDevice, FakeStreamFactory};
use crate::managers::tts_queue::{StopOutcome, Synth};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---- scripted fake engine ----------------------------------------------------------------------

struct Gate {
    open: Mutex<bool>,
    cv: Condvar,
    entered: AtomicUsize,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            cv: Condvar::new(),
            entered: AtomicUsize::new(0),
        })
    }
    fn wait_open(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let g = self.open.lock().unwrap();
        let _g = self.cv.wait_while(g, |o| !*o).unwrap();
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }
    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
struct Call {
    text: String,
    voice: String,
    speed: f32,
    at: Instant,
}

#[derive(Default)]
struct Log {
    calls: Vec<Call>,
    successes: Vec<String>,
    loads: usize,
    drops: usize,
}

#[derive(Default)]
struct Script {
    gate: Option<(usize, Arc<Gate>)>,
    fail_all_after: Option<usize>,
    fail_word: Option<String>,
    amplitudes: Vec<f32>,
    fixed_samples: Option<usize>,
    delay: Duration,
    fail_load: bool,
    panic_load: bool,
}

struct Shared {
    log: Mutex<Log>,
    script: Mutex<Script>,
}

struct FakeEngine {
    shared: Arc<Shared>,
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.shared.log.lock().unwrap().drops += 1;
    }
}

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// Samples the fake engine returns for `text`: 0.04 s per alphanumeric char at 24 kHz
/// (comfortably above the 0.03 truncation threshold).
fn samples_for(text: &str) -> usize {
    (alnum(text) as f32 * 0.04 * 24_000.0) as usize
}

impl SynthEngine for FakeEngine {
    fn synth(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String> {
        let idx = {
            let mut log = self.shared.log.lock().unwrap();
            log.calls.push(Call {
                text: text.to_string(),
                voice: voice.to_string(),
                speed,
                at: Instant::now(),
            });
            log.calls.len() - 1
        };
        let (gate, delay, fail_after, fail_word, amp, fixed) = {
            let s = self.shared.script.lock().unwrap();
            (
                s.gate.clone(),
                s.delay,
                s.fail_all_after,
                s.fail_word.clone(),
                s.amplitudes.get(idx).copied().unwrap_or(0.1),
                s.fixed_samples,
            )
        };
        if let Some((gi, g)) = gate {
            if gi == idx {
                g.wait_open();
            }
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if fail_after.is_some_and(|n| idx >= n) {
            return Err("scripted failure".into());
        }
        if fail_word.is_some_and(|w| text.contains(&w)) {
            return Err("scripted word failure".into());
        }
        self.shared
            .log
            .lock()
            .unwrap()
            .successes
            .push(text.to_string());
        Ok(vec![amp; fixed.unwrap_or_else(|| samples_for(text))])
    }
}

struct FakeLoader {
    shared: Arc<Shared>,
}

impl EngineLoader for FakeLoader {
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String> {
        let (fail, panic_now) = {
            let s = self.shared.script.lock().unwrap();
            (s.fail_load, s.panic_load)
        };
        self.shared.log.lock().unwrap().loads += 1;
        if panic_now {
            panic!("loader panicked on purpose");
        }
        if fail {
            return Err("model missing".into());
        }
        Ok(Box::new(FakeEngine {
            shared: self.shared.clone(),
        }))
    }
}

// ---- rig -------------------------------------------------------------------------------------------

struct Rig {
    synth: NaturalSynth,
    shared: Arc<Shared>,
    factory: Arc<FakeStreamFactory>,
    device: Option<FakeDevice>,
    health: Arc<NaturalHealth>,
    events: Arc<Mutex<Vec<FailureReason>>>,
}

fn test_config() -> NaturalConfig {
    NaturalConfig {
        idle_unload: Duration::from_secs(60),
        tick: ms(20),
        max_pending_chunks: NATURAL_MAX_PENDING_CHUNKS,
        flush_budget: ms(180),
    }
}

fn rig_with(script: Script, config: NaturalConfig, with_device: bool) -> Rig {
    let shared = Arc::new(Shared {
        log: Mutex::new(Log::default()),
        script: Mutex::new(script),
    });
    let factory = FakeStreamFactory::new(24_000);
    let sink = Arc::new(ManagedSink::new(factory.clone(), Duration::from_secs(3)));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    health.set_event_sink(Box::new(move |r| ev.lock().unwrap().push(r)));
    let synth = NaturalSynth::new(
        Box::new(FakeLoader {
            shared: shared.clone(),
        }),
        sink,
        health.clone(),
        config,
    );
    let device = with_device.then(|| FakeDevice::start(factory.clone(), 240, ms(10)));
    Rig {
        synth,
        shared,
        factory,
        device,
        health,
        events,
    }
}

fn rig(script: Script) -> Rig {
    rig_with(script, test_config(), true)
}

impl Rig {
    fn say(&mut self, text: &str) {
        self.synth.speak(text, None, None, 0.0);
    }
    fn start_device(&mut self) {
        self.device = Some(FakeDevice::start(self.factory.clone(), 240, ms(10)));
    }
    fn nonzero(&self) -> usize {
        self.device.as_ref().map_or(0, |d| d.nonzero())
    }
    fn calls(&self) -> Vec<Call> {
        self.shared.log.lock().unwrap().calls.clone()
    }
    fn successes(&self) -> Vec<String> {
        self.shared.log.lock().unwrap().successes.clone()
    }
    fn loads(&self) -> usize {
        self.shared.log.lock().unwrap().loads
    }
    fn drops(&self) -> usize {
        self.shared.log.lock().unwrap().drops
    }
    fn events(&self) -> Vec<FailureReason> {
        self.events.lock().unwrap().clone()
    }
    fn wait_idle(&mut self, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if !self.synth.is_speaking() {
                return true;
            }
            std::thread::sleep(ms(5));
        }
        false
    }
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(ms(5));
    }
    cond()
}

/// A sentence of exactly `len` chars whose first word is `marker` (never split by the chunker
/// when `len <= 80`; `len == 48` fits the first-chunk cap exactly).
fn sentence(marker: &str, len: usize) -> String {
    format!("{marker} {}.", "x".repeat(len - marker.len() - 2))
}

fn omega() -> String {
    format!("{}.", ["omega"; 10].join(" "))
}

const SHORT: &str = "Hello there.";

// ---- health ----------------------------------------------------------------------------------------

#[test]
fn health_applies_a_failure_once_and_emits_one_event() {
    let h = NaturalHealth::new(Duration::from_secs(300));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    h.set_event_sink(Box::new(move |r| ev.lock().unwrap().push(r)));
    assert!(h.is_ready());
    let r = h.revision();
    assert!(h.report_failure(r, FailureReason::Load));
    assert_eq!(h.state(), HealthState::Failed(FailureReason::Load));
    assert!(!h.is_ready());
    assert!(!h.report_failure(r, FailureReason::Synth), "already failed");
    assert_eq!(h.state(), HealthState::Failed(FailureReason::Load));
    assert_eq!(*events.lock().unwrap(), vec![FailureReason::Load]);
}

#[test]
fn health_ignores_stale_revision_failures_and_bump_recovers() {
    let h = NaturalHealth::new(Duration::from_secs(300));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    h.set_event_sink(Box::new(move |r| ev.lock().unwrap().push(r)));
    let old = h.revision();
    let new = h.bump();
    assert!(new > old);
    assert!(
        !h.report_failure(old, FailureReason::Sink),
        "stale revision"
    );
    assert!(h.is_ready());
    assert!(events.lock().unwrap().is_empty());
    assert!(h.report_failure(new, FailureReason::Sink));
    let again = h.bump();
    assert!(again > new);
    assert!(h.is_ready());
}

#[test]
fn health_auto_retry_after_the_cooldown_exactly_once() {
    let h = NaturalHealth::new(ms(50));
    let t0 = Instant::now();
    assert!(!h.maybe_auto_retry(t0), "nothing failed");
    let r = h.revision();
    assert!(h.report_failure(r, FailureReason::Synth));
    assert!(!h.maybe_auto_retry(Instant::now()));
    assert!(h.maybe_auto_retry(Instant::now() + ms(80)));
    assert!(h.is_ready());
    assert!(h.revision() > r);
    assert!(!h.maybe_auto_retry(Instant::now() + ms(200)));
}

// ---- speak / playback ----------------------------------------------------------------------------

#[test]
fn speak_returns_immediately_even_while_synthesis_blocks_and_counts_as_speaking() {
    let gate = Gate::new();
    let mut r = rig(Script {
        gate: Some((0, gate.clone())),
        ..Script::default()
    });
    let started = Instant::now();
    r.say(SHORT);
    assert!(started.elapsed() < ms(100), "speak must not block");
    assert!(
        r.synth.is_speaking(),
        "speaking from acceptance (start watchdog)"
    );
    assert!(wait_until(Duration::from_secs(2), || gate.entered() >= 1));
    assert!(r.synth.is_speaking());
    gate.release();
    assert!(r.wait_idle(5));
}

#[test]
fn plays_chunks_in_order_stays_speaking_until_drained_and_goes_idle_once() {
    let mut r = rig(Script::default());
    let text = format!("{} {}", sentence("alpha", 48), sentence("bravo", 59));
    let expected_chunks = split_for_synthesis(&text, NATURAL_MAX_CHARS);
    assert!(expected_chunks.len() >= 2);
    let expected_samples: usize = expected_chunks.iter().map(|c| samples_for(c)).sum();
    r.say(&text);
    let mut transitions = 0;
    let mut prev = r.synth.is_speaking();
    assert!(prev);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let now = r.synth.is_speaking();
        if now != prev {
            transitions += 1;
            prev = now;
        }
        if !now {
            break;
        }
        std::thread::sleep(ms(5));
    }
    assert!(!prev, "must become idle");
    assert_eq!(transitions, 1, "no flicker between chunks");
    let texts: Vec<String> = r.calls().into_iter().map(|c| c.text).collect();
    assert_eq!(texts, expected_chunks);
    assert_eq!(r.nonzero(), expected_samples);
}

#[test]
fn pipelines_the_next_chunk_while_the_previous_one_plays() {
    let mut r = rig(Script {
        delay: ms(40),
        ..Script::default()
    });
    let text = format!("{} {}", sentence("alpha", 48), sentence("bravo", 59));
    let t0 = Instant::now();
    r.say(&text);
    assert!(wait_until(Duration::from_secs(3), || r.calls().len() >= 2));
    let calls = r.calls();
    // Chunk 1 plays for > 1.8 s; chunk 2's synthesis must start long before that ends.
    assert!(
        calls[1].at.duration_since(t0) < ms(700),
        "second synth started after {:?}",
        calls[1].at.duration_since(t0)
    );
    r.synth.stop_immediate();
}

#[test]
fn voice_and_speed_are_forwarded_with_defaults() {
    let mut r = rig(Script::default());
    r.synth.speak(SHORT, Some("bf_emma"), Some(200), 0.0);
    assert!(r.wait_idle(5));
    r.synth.speak(SHORT, None, None, 0.0);
    assert!(r.wait_idle(5));
    let calls = r.calls();
    assert_eq!(calls[0].voice, "bf_emma");
    assert!((calls[0].speed - wpm_to_speed(Some(200))).abs() < 1e-3);
    assert_eq!(calls[1].voice, DEFAULT_NATURAL_VOICE);
    assert!((calls[1].speed - 1.0).abs() < 1e-3);
}

#[test]
fn empty_or_unspeakable_text_is_ignored_without_loading_the_engine() {
    let mut r = rig(Script::default());
    r.say("");
    r.say("   ");
    r.say("... !!!");
    assert!(!r.synth.is_speaking());
    std::thread::sleep(ms(150));
    assert_eq!(r.loads(), 0, "lazy: nothing to say, nothing loaded");
    assert!(r.calls().is_empty());
}

// ---- stop (C1, C3, C11) ---------------------------------------------------------------------------

#[test]
fn stop_while_idle_reports_already_idle() {
    let mut r = rig(Script::default());
    assert_eq!(r.synth.stop_immediate(), StopOutcome::AlreadyIdle);
}

#[test]
fn stop_during_playback_silences_quickly_and_discards_everything_pending() {
    let mut r = rig(Script::default());
    let text = [
        sentence("alpha", 48),
        sentence("bravo", 66),
        sentence("charlie", 66),
        sentence("delta", 66),
    ]
    .join(" ");
    r.say(&text);
    assert!(wait_until(Duration::from_secs(5), || r.nonzero() > 0));
    let t = Instant::now();
    assert_eq!(r.synth.stop_immediate(), StopOutcome::Stopped);
    assert!(t.elapsed() < ms(250), "stop took {:?}", t.elapsed());
    assert!(!r.synth.is_speaking());
    let played = r.nonzero();
    let calls_at_stop = r.calls().len();
    std::thread::sleep(ms(400));
    assert_eq!(r.nonzero(), played, "no audio after a confirmed stop");
    assert!(
        r.calls().len() <= calls_at_stop + 1,
        "no new synthesis after stop"
    );
}

#[test]
fn stale_synthesis_is_discarded_and_the_replacement_plays_without_flicker() {
    // C1 + C11: the in-flight synth cannot be aborted; its result must never be played, must not
    // clear the newer submission's busy state, and no recursive retries may start.
    let gate = Gate::new();
    let mut r = rig(Script {
        gate: Some((0, gate.clone())),
        amplitudes: vec![0.3, 0.5],
        ..Script::default()
    });
    r.say("First sentence here.");
    assert!(wait_until(Duration::from_secs(2), || gate.entered() >= 1));
    assert_eq!(r.synth.stop_immediate(), StopOutcome::Stopped);
    r.say("Second one here.");
    assert!(r.synth.is_speaking());
    gate.release();
    let until = Instant::now() + ms(100);
    while Instant::now() < until {
        assert!(
            r.synth.is_speaking(),
            "stale completion must not clear the new busy state"
        );
        std::thread::sleep(ms(5));
    }
    assert!(r.wait_idle(5));
    let calls = r.calls();
    assert_eq!(
        calls.len(),
        2,
        "no retry for the stale chunk: {:?}",
        calls.iter().map(|c| &c.text).collect::<Vec<_>>()
    );
    assert_eq!(calls[1].text, "Second one here.");
    let played = r.device.as_ref().unwrap().rendered();
    assert!(
        played.iter().all(|s| *s == 0.0 || *s == 0.5),
        "old-generation audio leaked"
    );
    assert_eq!(r.nonzero(), samples_for("Second one here."));
}

#[test]
fn a_stale_error_result_does_not_trigger_recursive_synthesis_or_failure() {
    // C11: block the first synth, stop, enqueue a replacement, then make the old call fail.
    let gate = Gate::new();
    let mut r = rig(Script {
        gate: Some((0, gate.clone())),
        fail_all_after: None,
        ..Script::default()
    });
    r.shared.script.lock().unwrap().fail_word = Some("First".into());
    r.say("First sentence here.");
    assert!(wait_until(Duration::from_secs(2), || gate.entered() >= 1));
    r.synth.stop_immediate();
    r.say("Second one here.");
    gate.release();
    assert!(r.wait_idle(5));
    let calls = r.calls();
    assert_eq!(calls.len(), 2, "old chunk must not be retried after stop");
    assert_eq!(calls[1].text, "Second one here.");
    assert!(
        r.health.is_ready(),
        "a cancelled failure is not a health failure"
    );
    assert!(r.events().is_empty());
}

#[test]
fn stop_is_failed_while_the_acoustic_tail_is_longer_than_the_budget_then_recovers() {
    // C3: never claim silence while old audio may be audible; recover once the tail drained.
    let mut r = rig(Script::default());
    let text = [
        sentence("alpha", 48),
        sentence("bravo", 66),
        sentence("charlie", 66),
    ]
    .join(" ");
    r.say(&text);
    assert!(wait_until(Duration::from_secs(5), || r.nonzero() > 0));
    r.factory.ctl().unwrap().set_tail_bound(ms(300));
    std::thread::sleep(ms(40)); // the next callbacks publish the longer tail
    let t = Instant::now();
    assert_eq!(r.synth.stop_immediate(), StopOutcome::Failed);
    assert!(t.elapsed() < ms(300));
    assert!(
        wait_until(Duration::from_secs(2), || !r.synth.is_speaking()),
        "tail must drain"
    );
    assert_eq!(
        r.synth.stop_immediate(),
        StopOutcome::AlreadyIdle,
        "recovers without a poll"
    );
}

// ---- bounds (C6) ---------------------------------------------------------------------------------------

#[test]
fn pending_chunks_are_capped_and_the_excess_is_counted() {
    let gate = Gate::new();
    let mut cfg = test_config();
    cfg.max_pending_chunks = 5;
    let mut r = rig_with(
        Script {
            gate: Some((0, gate.clone())),
            fixed_samples: Some(48_000), // 2 s per chunk: 5 chunks fit the 20 s sink bound
            ..Script::default()
        },
        cfg,
        false, // no device needed: only synthesis is checked
    );
    let text: Vec<String> = (0..100)
        .map(|i| sentence(&format!("s{i:03}"), 66))
        .collect();
    let text = text.join(" ");
    let total = split_for_synthesis(&text, NATURAL_MAX_CHARS).len();
    assert!(total > 100);
    r.say(&text);
    assert_eq!(r.synth.dropped_chunks(), total - 5);
    gate.release();
    assert!(wait_until(Duration::from_secs(10), || r.calls().len() == 5));
    std::thread::sleep(ms(200));
    assert_eq!(
        r.calls().len(),
        5,
        "only the accepted chunks are synthesised"
    );
    r.synth.stop_immediate();
}

#[test]
fn the_default_pending_cap_is_sixteen_chunks() {
    assert_eq!(NATURAL_MAX_PENDING_CHUNKS, 16);
    assert_eq!(NaturalConfig::default().max_pending_chunks, 16);
    assert_eq!(NaturalConfig::default().flush_budget, ms(180));
}

#[test]
fn backpressure_wait_is_cancel_aware_and_the_worker_stays_usable() {
    let mut r = rig_with(
        Script {
            fixed_samples: Some(360_000), // 15 s per chunk; the sink bound is 20 s
            ..Script::default()
        },
        test_config(),
        false, // no device: nothing drains the queue
    );
    let text = [
        sentence("alpha", 48),
        sentence("bravo", 59),
        sentence("charlie", 66),
    ]
    .join(" ");
    r.say(&text);
    assert!(wait_until(Duration::from_secs(3), || r.calls().len() >= 2));
    std::thread::sleep(ms(300)); // second commit is Full: the worker waits, no third synthesis
    assert_eq!(r.calls().len(), 2);
    assert!(r.synth.is_speaking());
    let t = Instant::now();
    assert_eq!(r.synth.stop_immediate(), StopOutcome::Stopped);
    assert!(t.elapsed() < ms(250));
    r.shared.script.lock().unwrap().fixed_samples = None;
    r.start_device();
    r.say("Hi there friend.");
    assert!(r.wait_idle(5));
    assert_eq!(
        r.nonzero(),
        samples_for("Hi there friend."),
        "only the new utterance was played"
    );
}

#[test]
fn idle_unload_is_driven_by_the_worker_and_the_next_speak_reloads() {
    let mut cfg = test_config();
    cfg.idle_unload = ms(300);
    let mut r = rig_with(Script::default(), cfg, true);
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert_eq!((r.loads(), r.drops()), (1, 0));
    std::thread::sleep(ms(900)); // NO is_speaking polling in this window
    assert_eq!(r.drops(), 1, "engine dropped after the idle period");
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert_eq!(r.loads(), 2);
    assert_eq!(r.nonzero(), 2 * samples_for(SHORT));
}

#[test]
fn the_engine_is_not_unloaded_while_audio_is_playing() {
    let mut cfg = test_config();
    cfg.idle_unload = ms(200);
    let mut r = rig_with(
        Script {
            fixed_samples: Some(48_000), // 2 s of audio
            ..Script::default()
        },
        cfg,
        true,
    );
    r.say(SHORT);
    std::thread::sleep(ms(1_000));
    assert_eq!(r.drops(), 0, "still playing");
    assert!(r.wait_idle(5));
    std::thread::sleep(ms(600));
    assert_eq!(r.drops(), 1);
}

// ---- failures and recovery (C5) ------------------------------------------------------------------------

#[test]
fn load_failure_marks_health_failed_once_and_the_accounting_drains() {
    let mut r = rig(Script {
        fail_load: true,
        ..Script::default()
    });
    r.say(SHORT);
    assert!(r.wait_idle(3), "must not stay busy forever");
    assert_eq!(r.health.state(), HealthState::Failed(FailureReason::Load));
    r.say(SHORT);
    assert!(r.wait_idle(3));
    assert_eq!(
        r.events(),
        vec![FailureReason::Load],
        "one event per transition"
    );
}

#[test]
fn a_first_chunk_that_cannot_be_synthesised_marks_health_failed() {
    let mut r = rig(Script {
        fail_all_after: Some(0),
        ..Script::default()
    });
    r.say(SHORT);
    assert!(r.wait_idle(3));
    assert_eq!(r.health.state(), HealthState::Failed(FailureReason::Synth));
    assert_eq!(r.events(), vec![FailureReason::Synth]);
}

#[test]
fn a_later_chunk_failure_drops_the_rest_of_that_submission_but_stays_ready() {
    let mut r = rig(Script {
        fail_word: Some("omega".into()),
        ..Script::default()
    });
    let text = format!(
        "{} {} {}",
        sentence("alpha", 48),
        omega(),
        sentence("third", 66)
    );
    r.say(&text);
    assert!(r.wait_idle(5));
    assert!(
        !r.calls().iter().any(|c| c.text.contains("third")),
        "rest of the submission dropped"
    );
    assert_eq!(r.successes().len(), 1);
    assert!(r.health.is_ready());
    r.shared.script.lock().unwrap().fail_word = None;
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert!(
        r.successes().contains(&SHORT.to_string()),
        "still works afterwards"
    );
}

#[test]
fn three_consecutive_later_chunk_failures_mark_health_failed() {
    let mut r = rig(Script {
        fail_word: Some("omega".into()),
        ..Script::default()
    });
    let text = format!("{} {}", sentence("alpha", 48), omega());
    for round in 1..=3 {
        r.say(&text);
        assert!(r.wait_idle(5), "round {round}");
        if round < 3 {
            assert!(
                r.health.is_ready(),
                "still ready after {round} later-chunk failures"
            );
        }
    }
    assert_eq!(r.health.state(), HealthState::Failed(FailureReason::Synth));
    assert_eq!(r.events(), vec![FailureReason::Synth]);
}

#[test]
fn a_worker_that_dies_drains_the_accounting_and_fails_health() {
    let mut r = rig(Script {
        panic_load: true,
        ..Script::default()
    });
    r.say(SHORT);
    assert!(r.wait_idle(3), "dead worker must not leave the synth busy");
    assert!(matches!(r.health.state(), HealthState::Failed(_)));
    r.say(SHORT);
    assert!(
        r.wait_idle(3),
        "still not busy forever after a second speak"
    );
}

#[test]
fn a_device_error_fails_health_drains_and_recover_restores_playback() {
    let mut r = rig(Script::default());
    let text = [
        sentence("alpha", 48),
        sentence("bravo", 66),
        sentence("charlie", 66),
    ]
    .join(" ");
    r.say(&text);
    assert!(wait_until(Duration::from_secs(5), || r.nonzero() > 0));
    r.factory.ctl().unwrap().fail(); // what the cpal error callback does
    assert!(wait_until(Duration::from_secs(2), || r.health.state()
        == HealthState::Failed(FailureReason::Sink)));
    assert!(r.wait_idle(3), "no permanently busy route");
    assert_eq!(r.events(), vec![FailureReason::Sink]);
    // Recovery trigger: the caller bumps health and asks the synth to recover.
    r.health.bump();
    r.synth.recover();
    let before = r.nonzero();
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert!(r.nonzero() >= before + samples_for(SHORT));
    assert_eq!(r.factory.opens(), 2, "stream re-created");
    assert_eq!(r.events().len(), 1);
}

#[test]
fn numbers_and_symbols_are_spelled_out_before_synthesis() {
    // The Kokoro crate mangles digits (first number read as Chinese numerals, later digits
    // dropped and logged), so the engine must hand it plain English words.
    let mut r = rig(Script::default());
    r.say("I have 3 apples and 50% off.");
    assert!(r.wait_idle(5));
    let texts: Vec<String> = r.calls().into_iter().map(|c| c.text).collect();
    assert_eq!(texts, vec!["I have three apples and fifty percent off."]);
    for t in &texts {
        assert!(!t.chars().any(|c| c.is_ascii_digit() || c == '%'), "{t:?}");
    }
}

#[test]
fn text_that_normalises_to_nothing_is_ignored_without_failing() {
    let mut r = rig(Script::default());
    r.say("*** ~~~ ___");
    assert!(r.wait_idle(3));
    assert!(
        r.calls().is_empty(),
        "nothing speakable, nothing synthesised"
    );
    assert_eq!(r.health.state(), HealthState::Ready);
    // and the engine still works afterwards
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert_eq!(r.calls().len(), 1);
}

#[test]
fn a_health_bump_alone_makes_the_next_speak_recover_by_itself() {
    // Recovery triggers (retry button, settings change, download, auto-retry) only bump the
    // health revision: the synth must notice and recover on the next speak without any call.
    let mut r = rig(Script::default());
    let text = [
        sentence("alpha", 48),
        sentence("bravo", 66),
        sentence("charlie", 66),
    ]
    .join(" ");
    r.say(&text);
    assert!(wait_until(Duration::from_secs(5), || r.nonzero() > 0));
    r.factory.ctl().unwrap().fail();
    assert!(wait_until(Duration::from_secs(2), || r.health.state()
        == HealthState::Failed(FailureReason::Sink)));
    assert!(r.wait_idle(3));
    r.health.bump(); // no synth.recover() call
    let before = r.nonzero();
    r.say(SHORT);
    assert!(r.wait_idle(5));
    assert!(
        r.nonzero() >= before + samples_for(SHORT),
        "the next speak must play"
    );
    assert_eq!(r.factory.opens(), 2);
}
