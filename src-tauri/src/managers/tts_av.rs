//! Resident macOS speech synthesizer (`AVSpeechSynthesizer`) behind the queue core.
//!
//! Measured: a resident synthesizer plays queued utterances gaplessly and first audio starts
//! ~0.1 s after `speakUtterance:`. Known limit: after an immediate stop, a resident
//! synthesizer does not start a new utterance unless the run loop is pumped, while a freshly
//! created one starts at once. So `stop_immediate` replaces the synthesizer after a stop
//! instead of pumping the run loop. The framework is used through objc2 dynamic messaging so
//! no typed AVFoundation crate is needed.
//!
//! The whole module is compiled only on macOS (see the `cfg` gate in `managers/mod.rs`).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;

use crate::managers::tts::{default_backend, SayBackend, SpeechBackend};
use crate::managers::tts_queue::{
    QueueBackend, SharedCore, SpeakerCore, StopOutcome, SwitchBackend, Synth,
};

#[link(name = "AVFoundation", kind = "framework")]
extern "C" {}

/// `AVSpeechBoundaryImmediate`.
const AV_BOUNDARY_IMMEDIATE: isize = 0;
/// Total bound for a stop to take effect (measured: S2 quiets a start-window stop in 58-70 ms).
// Measured: re-issued stops quiet the synthesizer in 58-212 ms (cold first stop is the slow case);
// 400 ms keeps a safe margin before reporting `Failed` (a false "stuck" is worse than a longer wait).
const STOP_CONFIRM: Duration = Duration::from_millis(400);
/// How often the stop is re-issued while the synthesizer is still starting or speaking.
const STOP_REISSUE: Duration = Duration::from_millis(25);
/// How often `isSpeaking` is checked between stop re-issues.
const STOP_CHECK: Duration = Duration::from_millis(5);
/// Extra quiet confirmation after `isSpeaking` first reads false.
const STOP_SETTLE: Duration = Duration::from_millis(30);
const DEFAULT_LANGUAGE: &str = "en-US";

/// Maps a words-per-minute rate onto the `AVSpeechUtterance` rate scale.
/// `None` is the framework default (0.5); otherwise scales 180 wpm to 0.5, clamped to
/// `0.2..=0.75`.
pub fn av_rate(wpm: Option<u32>) -> f32 {
    match wpm {
        None => 0.5,
        Some(w) => (0.5 * w as f32 / 180.0).clamp(0.2, 0.75),
    }
}

/// `respondsToSelector:` guard so a missing selector is never sent (sending one raises an
/// Objective-C exception, which must not reach Rust).
fn responds(obj: &AnyObject, selector: Sel) -> bool {
    // SAFETY: `respondsToSelector:` is defined on NSObject, which every object here inherits.
    unsafe { msg_send![obj, respondsToSelector: selector] }
}

/// Creates a new `AVSpeechSynthesizer` after checking every selector this module sends to it
/// and to the utterance and voice classes. `None` if anything is missing.
fn create_synth() -> Option<Retained<AnyObject>> {
    let synth_cls = AnyClass::get(c"AVSpeechSynthesizer")?;
    let utt_cls = AnyClass::get(c"AVSpeechUtterance")?;
    let voice_cls = AnyClass::get(c"AVSpeechSynthesisVoice")?;
    // Class methods live on the metaclass, so check there.
    if !(synth_cls.metaclass().responds_to(sel!(new))
        && utt_cls
            .metaclass()
            .responds_to(sel!(speechUtteranceWithString:))
        && voice_cls.metaclass().responds_to(sel!(speechVoices))
        && voice_cls.metaclass().responds_to(sel!(voiceWithLanguage:)))
    {
        return None;
    }
    // SAFETY: `new` on the synthesizer class returns an owned, initialized object.
    let synth: Retained<AnyObject> = unsafe { msg_send![synth_cls, new] };
    if !(responds(&synth, sel!(speakUtterance:))
        && responds(&synth, sel!(stopSpeakingAtBoundary:))
        && responds(&synth, sel!(isSpeaking)))
    {
        return None;
    }
    Some(synth)
}

/// A resident `AVSpeechSynthesizer` driven through dynamic messaging.
pub struct AvSynth {
    /// The current burst's synthesizer; `None` when idle (dropped after a stop or reset).
    synth: Option<Retained<AnyObject>>,
    volume: f32,
}

// SAFETY: the Objective-C objects are only ever touched through `Synth` methods, and every
// access is serialized by the `SharedCore` mutex that owns this value, so no two threads
// message the synthesizer at the same time.
unsafe impl Send for AvSynth {}

impl AvSynth {
    /// A synthesizer at full volume. `None` if the framework/classes are unavailable.
    pub fn new() -> Option<Self> {
        Self::with_volume(1.0)
    }

    /// A synthesizer at `volume` (clamped to `0.0..=1.0`). `None` if the classes or the
    /// selectors this module uses are missing, or the volume is NaN.
    pub fn with_volume(volume: f32) -> Option<Self> {
        if volume.is_nan() {
            return None;
        }
        let volume = volume.clamp(0.0, 1.0);
        // Probe availability once; the first burst creates its own synthesizer lazily.
        let probe = create_synth()?;
        drop(probe);
        Some(Self {
            synth: None,
            volume,
        })
    }

    fn speaking(&self) -> bool {
        match self.synth.as_deref() {
            // SAFETY: `isSpeaking` is a read-only BOOL property (selector presence checked in
            // `create_synth`) on a live synthesizer owned by this value.
            Some(synth) => unsafe { msg_send![synth, isSpeaking] },
            None => false,
        }
    }
}

/// Finds an installed voice whose `name` equals `wanted` (case-insensitive).
fn find_voice_named(wanted: &str) -> Option<Retained<AnyObject>> {
    let voice_cls = AnyClass::get(c"AVSpeechSynthesisVoice")?;
    // SAFETY: `speechVoices` is a class method returning an autoreleased NSArray.
    let voices: Retained<AnyObject> = unsafe { msg_send![voice_cls, speechVoices] };
    // SAFETY: `count` on an NSArray returns NSUInteger.
    let count: usize = unsafe { msg_send![&*voices, count] };
    for index in 0..count {
        // SAFETY: `objectAtIndex:` with an index below `count` returns the element or nil.
        let voice: Option<Retained<AnyObject>> =
            unsafe { msg_send![&*voices, objectAtIndex: index] };
        let Some(voice) = voice else { continue };
        // SAFETY: `name` on an AVSpeechSynthesisVoice returns an NSString or nil.
        let name: Option<Retained<NSString>> = unsafe { msg_send![&*voice, name] };
        if let Some(name) = name {
            if name.to_string().eq_ignore_ascii_case(wanted) {
                return Some(voice);
            }
        }
    }
    None
}

/// The voice for `name`: an installed voice with that name, else the en-US voice (also used
/// when `name` is `None`). `None` only if no voice at all can be created.
fn voice_for(name: Option<&str>) -> Option<Retained<AnyObject>> {
    if let Some(wanted) = name {
        if let Some(voice) = find_voice_named(wanted) {
            return Some(voice);
        }
    }
    let voice_cls = AnyClass::get(c"AVSpeechSynthesisVoice")?;
    let language = NSString::from_str(DEFAULT_LANGUAGE);
    // SAFETY: `voiceWithLanguage:` is a class method taking an NSString and returning an
    // autoreleased voice or nil.
    unsafe { msg_send![voice_cls, voiceWithLanguage: &*language] }
}

impl Synth for AvSynth {
    fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32) {
        // One fresh synthesizer per speaking burst: create it lazily here, never reuse a
        // stopped one (see `reset`/`stop_immediate`).
        if self.synth.is_none() {
            self.synth = create_synth();
        }
        let Some(synth) = self.synth.as_deref() else {
            return;
        };
        let Some(utt_cls) = AnyClass::get(c"AVSpeechUtterance") else {
            return;
        };
        let text = NSString::from_str(text);
        // SAFETY: `speechUtteranceWithString:` is a class method taking an NSString and
        // returning an autoreleased utterance or nil.
        let utterance: Option<Retained<AnyObject>> =
            unsafe { msg_send![utt_cls, speechUtteranceWithString: &*text] };
        let Some(utterance) = utterance else { return };
        if !(responds(&utterance, sel!(setRate:))
            && responds(&utterance, sel!(setPitchMultiplier:))
            && responds(&utterance, sel!(setVolume:))
            && responds(&utterance, sel!(setVoice:))
            && responds(synth, sel!(speakUtterance:)))
        {
            return;
        }
        let rate = av_rate(rate_wpm);
        let voice = voice_for(voice);
        // SAFETY: the setters are present (checked above) and take float/object arguments;
        // `speakUtterance:` queues the utterance on the resident synthesizer.
        unsafe {
            let _: () = msg_send![&*utterance, setRate: rate];
            let _: () = msg_send![&*utterance, setPitchMultiplier: pitch];
            let _: () = msg_send![&*utterance, setVolume: self.volume];
            if let Some(voice) = voice {
                let _: () = msg_send![&*utterance, setVoice: &*voice];
            }
            let _: () = msg_send![synth, speakUtterance: &*utterance];
        }
    }

    fn is_speaking(&mut self) -> bool {
        self.speaking()
    }

    fn stop_immediate(&mut self) -> StopOutcome {
        let Some(synth) = self.synth.as_deref() else {
            return StopOutcome::AlreadyIdle;
        };
        // Speaking before the stop counts too, so read it first.
        // SAFETY: `isSpeaking` on the live synthesizer held in `self.synth` (see above).
        let mut seen_speaking: bool = unsafe { msg_send![synth, isSpeaking] };
        // Why re-issue: a single stop issued before the engine has begun is ignored, so the
        // synth keeps speaking (measured: one stop never went quiet, 10/10 rounds). Re-issuing
        // the stop every 25 ms quiets it in 58-70 ms (10/10 rounds). Speech may have been
        // queued and not yet started (the start window), so the stop is always sent.
        let start = Instant::now();
        let mut last_stop: Option<Instant> = None;
        loop {
            if last_stop.is_none_or(|t| t.elapsed() >= STOP_REISSUE) {
                // SAFETY: `stopSpeakingAtBoundary:` (presence checked in `create_synth`) takes
                // an NSInteger boundary; the immediate boundary cancels all queued speech.
                let _stopped: bool =
                    unsafe { msg_send![synth, stopSpeakingAtBoundary: AV_BOUNDARY_IMMEDIATE] };
                last_stop = Some(Instant::now());
            }
            // SAFETY: `isSpeaking` on the live synthesizer held in `self.synth` (see above).
            let speaking: bool = unsafe { msg_send![synth, isSpeaking] };
            if speaking {
                seen_speaking = true;
            } else {
                // Quiet: confirm it stays quiet for one settle window before accepting it.
                thread::sleep(STOP_SETTLE);
                // SAFETY: as above.
                let again: bool = unsafe { msg_send![synth, isSpeaking] };
                if !again {
                    break;
                }
                seen_speaking = true;
            }
            if start.elapsed() >= STOP_CONFIRM {
                // Still speaking after the bound: keep this synthesizer and report the failure.
                return StopOutcome::Failed;
            }
            thread::sleep(STOP_CHECK);
        }
        // Quiet and confirmed: drop this burst's synthesizer. Reusing a stopped one does not
        // restart reliably, so the next burst gets a fresh one.
        self.synth = None;
        if seen_speaking {
            StopOutcome::Stopped
        } else {
            StopOutcome::AlreadyIdle
        }
    }

    fn reset(&mut self) {
        self.synth = None;
    }
}

/// The fast backend: a resident synthesizer behind the queue, or `None` if unavailable.
/// Build the speaker as a shared core so the backend and its handles share one queue.
fn fast_backend() -> Option<QueueBackend<AvSynth>> {
    let synth = AvSynth::new()?;
    let core: SharedCore<AvSynth> = Arc::new(Mutex::new(SpeakerCore::new(synth)));
    Some(QueueBackend::new(core))
}

/// Speech backend that routes to the resident synthesizer when `pick_fast()` is true
/// (evaluated per call) and to the `say` backend otherwise. Falls back to the plain default
/// backend when the synthesizer is unavailable.
pub fn default_backend_with(
    pick_fast: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Box<dyn SpeechBackend> {
    match fast_backend() {
        Some(fast) => Box::new(SwitchBackend::new(
            Box::new(SayBackend),
            Box::new(fast),
            pick_fast,
        )),
        None => default_backend(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn av_rate_maps_wpm_onto_the_utterance_scale_with_clamping() {
        assert!(av_rate(None) == 0.5);
        assert!(av_rate(Some(180)) == 0.5);
        assert!((av_rate(Some(90)) - 0.25).abs() < 1e-6);
        assert!(av_rate(Some(10)) == 0.2);
        assert!(av_rate(Some(1000)) == 0.75);
    }

    /// Test-only wrapper that records whether the real `isSpeaking` ever reported true.
    struct Observed {
        inner: AvSynth,
        seen: Arc<AtomicBool>,
    }

    impl Synth for Observed {
        fn speak(&mut self, text: &str, voice: Option<&str>, rate_wpm: Option<u32>, pitch: f32) {
            self.inner.speak(text, voice, rate_wpm, pitch);
        }
        fn is_speaking(&mut self) -> bool {
            let speaking = self.inner.is_speaking();
            if speaking {
                self.seen.store(true, Ordering::SeqCst);
            }
            speaking
        }
        fn stop_immediate(&mut self) -> StopOutcome {
            self.inner.stop_immediate()
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
    }

    /// Polls the core until the real synthesizer has been observed speaking, or `within` passes.
    fn wait_seen(core: &mut SpeakerCore<Observed>, seen: &AtomicBool, within: Duration) -> bool {
        let end = Instant::now() + within;
        while !seen.load(Ordering::SeqCst) {
            if Instant::now() >= end {
                return false;
            }
            core.poll(Instant::now());
            thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Polls until the core goes idle by itself, failing after `within`.
    fn run_to_completion(core: &mut SpeakerCore<Observed>, within: Duration) {
        let end = Instant::now() + within;
        while core.poll(Instant::now()) {
            assert!(Instant::now() < end, "speech did not finish");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Checks that the real synthesizer stays silent for `dur`: the core must report idle and
    /// the wrapper must never see `isSpeaking` true.
    fn assert_quiet_for(core: &mut SpeakerCore<Observed>, seen: &AtomicBool, dur: Duration) {
        let end = Instant::now() + dur;
        while Instant::now() < end {
            assert!(!core.poll(Instant::now()), "core busy while idle");
            assert!(
                !seen.load(Ordering::SeqCst),
                "synth was speaking while idle"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Manual live check (needs macOS audio hardware; volume 0 keeps it silent):
    /// `cargo test --lib -- --ignored live_resident --nocapture`
    #[test]
    #[ignore]
    fn live_resident_synth_speaks_queues_and_stops_truthfully() {
        let worker = thread::spawn(|| {
            let seen = Arc::new(AtomicBool::new(false));
            let synth = Observed {
                inner: AvSynth::with_volume(0.0).expect("AVSpeechSynthesizer available"),
                seen: Arc::clone(&seen),
            };
            let core: SharedCore<Observed> = Arc::new(Mutex::new(SpeakerCore::new(synth)));
            let mut core = core.lock().expect("core lock");

            core.enqueue(
                "Hello there, this is the first sentence.",
                None,
                None,
                1.0,
                Instant::now(),
            )
            .expect("first enqueue");
            core.enqueue(
                "And this is the second one.",
                None,
                None,
                1.0,
                Instant::now(),
            )
            .expect("second enqueue");
            assert!(core.poll(Instant::now()), "busy right after enqueue");

            // Wait until the real synthesizer reports speaking (not just the core's busy flag).
            let observe_deadline = Instant::now() + Duration::from_secs(1);
            while !seen.load(Ordering::SeqCst) {
                assert!(
                    Instant::now() < observe_deadline,
                    "speech never observed starting"
                );
                core.poll(Instant::now());
                thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(core.stop(Instant::now()), StopOutcome::Stopped);
            assert!(!core.poll(Instant::now()), "idle after stop");

            seen.store(false, Ordering::SeqCst);
            core.enqueue(
                "Once more, to let it finish.",
                None,
                None,
                1.0,
                Instant::now(),
            )
            .expect("third enqueue");
            let finish_deadline = Instant::now() + Duration::from_secs(15);
            while core.poll(Instant::now()) {
                assert!(Instant::now() < finish_deadline, "speech did not finish");
                thread::sleep(Duration::from_millis(10));
            }
            assert!(
                seen.load(Ordering::SeqCst),
                "third utterance was never observed"
            );
            assert_eq!(core.take_start_failures(), 0);
            assert_eq!(core.stop(Instant::now()), StopOutcome::AlreadyIdle);

            // (a) Stop inside the start window: nothing observed speaking afterwards.
            seen.store(false, Ordering::SeqCst);
            core.enqueue(
                "Quick one, cut off at once.",
                None,
                None,
                1.0,
                Instant::now(),
            )
            .expect("window enqueue");
            let outcome = core.stop(Instant::now());
            assert_ne!(outcome, StopOutcome::Failed, "start-window stop failed");
            assert!(!core.poll(Instant::now()), "idle after start-window stop");
            assert_quiet_for(&mut core, &seen, Duration::from_millis(500));

            // (b) Restart on the replaced synthesizer: must actually speak and finish.
            core.enqueue("Restarted after the stop.", None, None, 1.0, Instant::now())
                .expect("restart enqueue");
            assert!(
                wait_seen(&mut core, &seen, Duration::from_secs(1)),
                "restart never observed"
            );
            run_to_completion(&mut core, Duration::from_secs(15));
            assert_eq!(core.take_start_failures(), 0);

            // (c) Repeat stop + restart three times.
            for round in 0..3 {
                seen.store(false, Ordering::SeqCst);
                core.enqueue("Round sentence to cut.", None, None, 1.0, Instant::now())
                    .expect("round enqueue");
                assert!(
                    wait_seen(&mut core, &seen, Duration::from_secs(1)),
                    "round {round}: start not observed"
                );
                assert_eq!(
                    core.stop(Instant::now()),
                    StopOutcome::Stopped,
                    "round {round}: stop"
                );
                assert!(!core.poll(Instant::now()), "round {round}: idle after stop");
                // Only speech AFTER the stop counts as a leak: clear the flag set before it.
                seen.store(false, Ordering::SeqCst);
                assert_quiet_for(&mut core, &seen, Duration::from_millis(500));

                seen.store(false, Ordering::SeqCst);
                core.enqueue("Round restart.", None, None, 1.0, Instant::now())
                    .expect("round restart enqueue");
                assert!(
                    wait_seen(&mut core, &seen, Duration::from_secs(1)),
                    "round {round}: restart not observed"
                );
                run_to_completion(&mut core, Duration::from_secs(15));
            }
            assert_eq!(core.take_start_failures(), 0);
            assert_eq!(core.stop(Instant::now()), StopOutcome::AlreadyIdle);
        });
        worker.join().expect("live worker panicked");
    }
}
