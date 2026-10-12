//! Authoritative tests for `tts_queue` (core, handle, backend). Do not edit to make code pass.

use super::*;
use crate::managers::emotion::Emotion;
use std::time::Instant;

#[derive(Default)]
pub(crate) struct FakeState {
    pub spoken: Vec<(String, Option<String>, Option<u32>, f32)>,
    pub speaking: bool,
    pub stop_calls: usize,
    pub stop_result: Option<StopOutcome>, // None => Stopped
    pub is_speaking_calls: usize,
    pub reset_calls: usize,
}

#[derive(Clone, Default)]
pub(crate) struct Fake(pub Arc<Mutex<FakeState>>);

impl Fake {
    pub fn set_speaking(&self, v: bool) {
        self.0.lock().unwrap().speaking = v;
    }
    pub fn spoken_texts(&self) -> Vec<String> {
        self.0.lock().unwrap().spoken.iter().map(|s| s.0.clone()).collect()
    }
    pub fn stop_calls(&self) -> usize {
        self.0.lock().unwrap().stop_calls
    }
    pub fn reset_calls(&self) -> usize {
        self.0.lock().unwrap().reset_calls
    }
    pub fn set_stop_result(&self, r: StopOutcome) {
        self.0.lock().unwrap().stop_result = Some(r);
    }
}

impl Synth for Fake {
    fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32) {
        self.0
            .lock()
            .unwrap()
            .spoken
            .push((text.to_string(), voice.map(str::to_string), rate_wpm, pitch));
    }
    fn is_speaking(&mut self) -> bool {
        let mut s = self.0.lock().unwrap();
        s.is_speaking_calls += 1;
        s.speaking
    }
    fn reset(&mut self) {
        self.0.lock().unwrap().reset_calls += 1;
    }
    fn stop_immediate(&mut self) -> StopOutcome {
        let mut s = self.0.lock().unwrap();
        s.stop_calls += 1;
        match s.stop_result.unwrap_or(StopOutcome::Stopped) {
            StopOutcome::Stopped => {
                s.speaking = false;
                StopOutcome::Stopped
            }
            StopOutcome::AlreadyIdle => {
                s.speaking = false;
                StopOutcome::AlreadyIdle
            }
            StopOutcome::Failed => StopOutcome::Failed,
        }
    }
}

fn core() -> (SpeakerCore<Fake>, Fake, Instant) {
    let f = Fake::default();
    (SpeakerCore::new(f.clone()), f, Instant::now())
}

fn ms(t: Instant, n: u64) -> Instant {
    t + Duration::from_millis(n)
}

// ---- SpeakerCore -------------------------------------------------------------------

#[test]
fn fresh_core_is_idle_and_never_touches_the_synth_to_stop() {
    let (mut c, f, t0) = core();
    assert!(!c.poll(t0));
    assert_eq!(c.pending(), 0);
    assert_eq!(c.stop(t0), StopOutcome::AlreadyIdle);
    assert_eq!(f.stop_calls(), 0, "idle + quiet synth: stop must not call the synth");
}

#[test]
fn enqueue_marks_busy_immediately_and_forwards_arguments_verbatim() {
    let (mut c, f, t0) = core();
    c.enqueue("  Hello there.  ", Some("Samantha"), Some(190), 1.12, t0).unwrap();
    assert!(c.poll(t0), "busy even before the synth reports speaking");
    assert_eq!(c.pending(), 1);
    let spoken = f.0.lock().unwrap().spoken.clone();
    assert_eq!(spoken, vec![("Hello there.".to_string(), Some("Samantha".to_string()), Some(190), 1.12)]);
}

#[test]
fn empty_text_is_rejected_without_side_effects() {
    let (mut c, f, t0) = core();
    assert_eq!(c.enqueue("", None, None, 1.0, t0), Err(QueueError::Empty));
    assert_eq!(c.enqueue("  \n\t ", None, None, 1.0, t0), Err(QueueError::Empty));
    assert!(f.spoken_texts().is_empty());
    assert!(!c.poll(t0));
    assert_eq!(c.pending(), 0);
}

#[test]
fn queue_runs_to_completion_in_order_then_goes_idle() {
    let (mut c, f, t0) = core();
    for s in ["One.", "Two.", "Three."] {
        c.enqueue(s, None, None, 1.0, t0).unwrap();
    }
    assert_eq!(f.spoken_texts(), vec!["One.", "Two.", "Three."]);
    assert_eq!(c.pending(), 3);
    assert!(c.poll(ms(t0, 10)), "queued, synth not started yet");
    f.set_speaking(true);
    assert!(c.poll(ms(t0, 50)));
    assert!(c.poll(ms(t0, 2000)), "still busy while the synth keeps speaking (no gap between sentences)");
    f.set_speaking(false);
    assert!(!c.poll(ms(t0, 2100)));
    assert_eq!(c.pending(), 0);
    assert_eq!(c.take_start_failures(), 0);
}

#[test]
fn start_watchdog_declares_a_silent_synth_failed() {
    let (mut c, _f, t0) = core();
    c.enqueue("Hi.", None, None, 1.0, t0).unwrap();
    assert!(c.poll(t0 + START_TIMEOUT - Duration::from_millis(1)), "not yet");
    assert!(!c.poll(t0 + START_TIMEOUT + Duration::from_millis(1)), "expired => idle");
    assert_eq!(c.pending(), 0);
    assert_eq!(c.take_start_failures(), 1);
    assert_eq!(c.take_start_failures(), 0, "counter is cleared by take");
}

#[test]
fn watchdog_origin_is_the_enqueue_that_found_the_core_idle() {
    let (mut c, f, t0) = core();
    c.enqueue("A.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    assert!(c.poll(ms(t0, 100)));
    f.set_speaking(false);
    assert!(!c.poll(ms(t0, 5000)));
    // second burst much later: its own origin, no stale failure from the first
    let t1 = ms(t0, 10_000);
    c.enqueue("B.", None, None, 1.0, t1).unwrap();
    assert!(c.poll(ms(t1, 2900)));
    assert!(!c.poll(ms(t1, 3100)));
    assert_eq!(c.take_start_failures(), 1);
}

#[test]
fn enqueue_while_busy_keeps_the_original_watchdog_origin() {
    let (mut c, _f, t0) = core();
    c.enqueue("A.", None, None, 1.0, t0).unwrap();
    c.enqueue("B.", None, None, 1.0, ms(t0, 2500)).unwrap();
    assert!(!c.poll(ms(t0, 3100)), "origin stays at the first enqueue");
    assert_eq!(c.take_start_failures(), 1);
}

#[test]
fn stop_stops_everything_and_allows_new_speech() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    c.enqueue("Two.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    assert_eq!(c.stop(ms(t0, 100)), StopOutcome::Stopped);
    assert_eq!(f.stop_calls(), 1);
    assert!(!c.poll(ms(t0, 110)));
    assert_eq!(c.pending(), 0);
    c.enqueue("Three.", None, None, 1.0, ms(t0, 200)).unwrap();
    assert!(c.poll(ms(t0, 210)));
    assert_eq!(f.spoken_texts(), vec!["One.", "Two.", "Three."]);
}

#[test]
fn stop_with_a_synth_that_says_already_idle_is_still_idle() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::AlreadyIdle);
    assert_eq!(c.stop(t0), StopOutcome::AlreadyIdle);
    assert!(!c.poll(ms(t0, 5)));
    assert_eq!(c.pending(), 0);
}

#[test]
fn failed_stop_stays_busy_refuses_new_speech_and_recovers_when_quiet() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert_eq!(c.stop(ms(t0, 10)), StopOutcome::Failed);
    assert!(c.poll(ms(t0, 20)), "still busy: the synth is still speaking");
    assert_eq!(c.enqueue("Two.", None, None, 1.0, ms(t0, 30)), Err(QueueError::Stuck));
    assert_eq!(f.spoken_texts(), vec!["One."], "nothing new reached the synth while stuck");
    // the synth eventually goes quiet by itself
    f.set_speaking(false);
    assert!(!c.poll(ms(t0, 400)));
    c.enqueue("Three.", None, None, 1.0, ms(t0, 410)).unwrap();
    assert_eq!(f.spoken_texts(), vec!["One.", "Three."]);
}

#[test]
fn stuck_clears_even_if_polled_only_via_enqueue_after_the_synth_is_quiet() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    let _ = c.stop(ms(t0, 10));
    f.set_speaking(false);
    // no explicit poll: enqueue must itself notice the synth is quiet and accept the speech
    assert_eq!(c.enqueue("Two.", None, None, 1.0, ms(t0, 50)), Ok(()));
}

#[test]
fn pitch_mapping_is_bounded_and_neutral_is_identity() {
    assert_eq!(pitch_for(Emotion::Neutral), 1.0);
    assert!(pitch_for(Emotion::Cheerful) > 1.0);
    assert!(pitch_for(Emotion::Serious) < 1.0);
    for e in [
        Emotion::Neutral,
        Emotion::Cheerful,
        Emotion::Excited,
        Emotion::Calm,
        Emotion::Concerned,
        Emotion::Apologetic,
        Emotion::Serious,
    ] {
        let p = pitch_for(e);
        assert!((0.5..=2.0).contains(&p), "{e:?} -> {p}");
    }
}

// ---- reset hook: one fresh synthesizer per speaking burst ----------------------------------

#[test]
fn reset_is_called_exactly_once_when_a_burst_ends_naturally() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    c.enqueue("Two.", None, None, 1.0, t0).unwrap();
    assert_eq!(f.reset_calls(), 0, "never reset while busy or when enqueuing more");
    f.set_speaking(true);
    assert!(c.poll(ms(t0, 100)));
    assert_eq!(f.reset_calls(), 0);
    f.set_speaking(false);
    assert!(!c.poll(ms(t0, 200)));
    assert_eq!(f.reset_calls(), 1);
    assert!(!c.poll(ms(t0, 300)), "polling an idle core does not reset again");
    assert_eq!(f.reset_calls(), 1);
}

#[test]
fn reset_is_called_when_the_start_watchdog_expires() {
    let (mut c, f, t0) = core();
    c.enqueue("Hi.", None, None, 1.0, t0).unwrap();
    assert!(!c.poll(t0 + START_TIMEOUT + Duration::from_millis(1)));
    assert_eq!(f.reset_calls(), 1);
}

#[test]
fn reset_follows_a_confirmed_stop_but_not_a_failed_one() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    f.set_speaking(true);
    assert_eq!(c.stop(ms(t0, 10)), StopOutcome::Stopped);
    assert_eq!(f.reset_calls(), 1);

    // failed stop: no reset while the synth may still be speaking ...
    c.enqueue("Two.", None, None, 1.0, ms(t0, 20)).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert_eq!(c.stop(ms(t0, 30)), StopOutcome::Failed);
    assert_eq!(f.reset_calls(), 1);
    // ... and exactly one reset once it has genuinely gone quiet
    f.set_speaking(false);
    assert!(!c.poll(ms(t0, 400)));
    assert_eq!(f.reset_calls(), 2);
}

#[test]
fn stopping_an_idle_quiet_core_does_not_reset() {
    let (mut c, f, t0) = core();
    assert_eq!(c.stop(t0), StopOutcome::AlreadyIdle);
    assert_eq!(f.reset_calls(), 0);
}

#[test]
fn stop_inside_the_start_window_resets_and_the_next_burst_starts_clean() {
    let (mut c, f, t0) = core();
    c.enqueue("One.", None, None, 1.0, t0).unwrap();
    // synth never reported speaking: stop right away (start window)
    assert_ne!(c.stop(ms(t0, 5)), StopOutcome::Failed);
    assert_eq!(f.stop_calls(), 1, "busy core always asks the synth to stop, even if it is not speaking yet");
    assert_eq!(f.reset_calls(), 1);
    c.enqueue("Two.", None, None, 1.0, ms(t0, 10)).unwrap();
    f.set_speaking(true);
    assert!(c.poll(ms(t0, 50)));
}

// ---- QueueHandle / QueueBackend ---------------------------------------------------------

fn backend() -> (QueueBackend<Fake>, SharedCore<Fake>, Fake) {
    let f = Fake::default();
    let core: SharedCore<Fake> = Arc::new(Mutex::new(SpeakerCore::new(f.clone())));
    (QueueBackend::new(core.clone()), core, f)
}

#[test]
fn backend_reports_queue_support_and_enqueue_appends_without_replacing() {
    let (b, _core, f) = backend();
    assert!(b.supports_queue());
    let mut h1 = b.enqueue_styled("One.", None, Some(180), Emotion::Neutral).unwrap();
    let mut h2 = b.enqueue_styled("Two.", None, Some(180), Emotion::Cheerful).unwrap();
    assert!(h1.queue_capable() && h2.queue_capable());
    assert_eq!(f.spoken_texts(), vec!["One.", "Two."]);
    assert_eq!(f.stop_calls(), 0);
    let spoken = f.0.lock().unwrap().spoken.clone();
    assert_eq!(spoken[0].3, 1.0);
    assert!(spoken[1].3 > 1.0, "emotion reaches the synth as pitch");
    assert!(h1.is_running() && h2.is_running(), "handles share the queue state");
}

#[test]
fn spawn_and_spawn_styled_enqueue_one_utterance_each() {
    let (b, _core, f) = backend();
    let mut h = b.spawn("Plain.", None, None).unwrap();
    assert!(h.is_running());
    let _ = b.spawn_styled("Styled.", Some("Daniel"), Some(200), Emotion::Calm).unwrap();
    assert_eq!(f.spoken_texts(), vec!["Plain.", "Styled."]);
}

#[test]
fn handle_kill_is_a_truthful_queue_wide_stop() {
    let (b, _core, f) = backend();
    let mut h = b.enqueue_styled("One.", None, None, Emotion::Neutral).unwrap();
    f.set_speaking(true);
    assert!(h.is_running());
    assert!(h.kill());
    assert_eq!(f.stop_calls(), 1);
    assert!(!h.is_running());

    // a failing OS stop is reported as false and the handle keeps running
    let mut h = b.enqueue_styled("Two.", None, None, Emotion::Neutral).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert!(!h.kill());
    assert!(h.is_running());
}

#[test]
fn backend_surfaces_empty_and_stuck_as_errors() {
    let (b, _core, f) = backend();
    assert!(b.enqueue_styled("   ", None, None, Emotion::Neutral).is_err());
    let mut h = b.enqueue_styled("One.", None, None, Emotion::Neutral).unwrap();
    f.set_speaking(true);
    f.set_stop_result(StopOutcome::Failed);
    assert!(!h.kill());
    assert!(b.enqueue_styled("Two.", None, None, Emotion::Neutral).is_err(), "stuck");
    assert_eq!(f.spoken_texts(), vec!["One."]);
}

#[test]
fn concurrent_enqueues_are_serialized_and_complete() {
    let (b, _core, f) = backend();
    let b = Arc::new(b);
    let mut threads = vec![];
    for i in 0..8 {
        let b = b.clone();
        threads.push(std::thread::spawn(move || {
            for j in 0..10 {
                b.enqueue_styled(&format!("t{i}-{j}"), None, None, Emotion::Neutral).unwrap();
            }
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(f.spoken_texts().len(), 80);
}

// ---- SwitchBackend ------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct CountingHandle;
impl SpeechHandle for CountingHandle {
    fn kill(&mut self) -> bool {
        true
    }
    fn is_running(&mut self) -> bool {
        false
    }
}

#[derive(Default)]
struct Counting {
    spawns: AtomicUsize,
    enqueues: AtomicUsize,
    queue: bool,
}

impl SpeechBackend for Arc<Counting> {
    fn spawn(&self, _t: &str, _v: Option<&str>, _r: Option<u32>) -> io::Result<Box<dyn SpeechHandle>> {
        self.spawns.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingHandle))
    }
    fn supports_queue(&self) -> bool {
        self.queue
    }
    fn enqueue_styled(&self, _t: &str, _v: Option<&str>, _r: Option<u32>, _e: Emotion) -> io::Result<Box<dyn SpeechHandle>> {
        self.enqueues.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingHandle))
    }
}

#[test]
fn switch_backend_routes_each_call_by_the_live_toggle() {
    let slow = Arc::new(Counting::default());
    let fast = Arc::new(Counting { queue: true, ..Default::default() });
    let flag = Arc::new(AtomicBool::new(false));
    let f2 = flag.clone();
    let sw = SwitchBackend::new(
        Box::new(slow.clone()),
        Box::new(fast.clone()),
        Arc::new(move || f2.load(Ordering::SeqCst)),
    );

    // toggle off: legacy backend only, no queue
    assert!(!sw.supports_queue());
    sw.spawn("a", None, None).unwrap();
    sw.spawn_styled("b", None, None, Emotion::Calm).unwrap();
    assert!(sw.enqueue_styled("c", None, None, Emotion::Neutral).is_err(), "slow side never queues");
    assert_eq!((slow.spawns.load(Ordering::SeqCst), fast.spawns.load(Ordering::SeqCst)), (2, 0));
    assert_eq!(fast.enqueues.load(Ordering::SeqCst), 0);

    // toggle on (takes effect immediately, no rebuild)
    flag.store(true, Ordering::SeqCst);
    assert!(sw.supports_queue());
    sw.spawn("d", None, None).unwrap();
    sw.enqueue_styled("e", None, None, Emotion::Neutral).unwrap();
    assert_eq!((slow.spawns.load(Ordering::SeqCst), fast.spawns.load(Ordering::SeqCst)), (2, 1));
    assert_eq!(fast.enqueues.load(Ordering::SeqCst), 1);

    // and back off again
    flag.store(false, Ordering::SeqCst);
    assert!(!sw.supports_queue());
    sw.spawn("f", None, None).unwrap();
    assert_eq!(slow.spawns.load(Ordering::SeqCst), 3);
}

#[test]
fn switch_backend_evaluates_the_toggle_once_per_call() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let sw = SwitchBackend::new(
        Box::new(Arc::new(Counting::default())),
        Box::new(Arc::new(Counting { queue: true, ..Default::default() })),
        Arc::new(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            true
        }),
    );
    sw.spawn_styled("x", None, None, Emotion::Neutral).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    sw.enqueue_styled("x", None, None, Emotion::Neutral).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
