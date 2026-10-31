//! Spec for the Tamil text mode of `natural_engine` (DESIGN R2-C, step S5b). CI-safe: fake engine
//! and fake sink only. The Latin behaviour is covered by `natural_engine_tests.rs`, which must
//! keep passing unchanged; the last test here pins the Latin path against regressions.
use crate::managers::natural_engine::*;
use crate::managers::natural_sink::ManagedSink;
use crate::managers::natural_test_support::{FakeDevice, FakeStreamFactory};
use crate::managers::natural_text::normalize_for_speech;
use crate::managers::tts_queue::{StopOutcome, Synth};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// Samples per text the fake engine returns when no fixed length is scripted: 0.04 s per
/// alphanumeric char at 24 kHz (well above the Latin truncation threshold).
fn samples_for(text: &str) -> usize {
    (alnum(text) as f32 * 0.04 * 24_000.0) as usize
}

#[derive(Default)]
struct Script {
    fail_all: bool,
    fail_word: Option<String>,
    /// A chunk containing this word gets `Ok(vec![])`.
    empty_word: Option<String>,
    /// Fixed sample count for every successful call (`None` = `samples_for`).
    fixed_samples: Option<usize>,
    delay: Duration,
}

/// Short audio for the Tamil tests so playback stays well inside the wait budgets.
fn quick() -> Script {
    Script {
        fixed_samples: Some(2_400),
        ..Script::default()
    }
}

struct Shared {
    calls: Mutex<Vec<String>>,
    script: Mutex<Script>,
}

struct FakeEngine {
    shared: Arc<Shared>,
}

impl SynthEngine for FakeEngine {
    fn synth(&mut self, text: &str, _voice: &str, _speed: f32) -> Result<Vec<f32>, String> {
        self.shared.calls.lock().unwrap().push(text.to_string());
        let (fail_all, fail_word, empty_word, fixed, delay) = {
            let s = self.shared.script.lock().unwrap();
            (
                s.fail_all,
                s.fail_word.clone(),
                s.empty_word.clone(),
                s.fixed_samples,
                s.delay,
            )
        };
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        if fail_all || fail_word.is_some_and(|w| text.contains(&w)) {
            return Err("scripted failure".into());
        }
        if empty_word.is_some_and(|w| text.contains(&w)) {
            return Ok(Vec::new());
        }
        Ok(vec![0.1; fixed.unwrap_or_else(|| samples_for(text))])
    }
}

struct FakeLoader {
    shared: Arc<Shared>,
}

impl EngineLoader for FakeLoader {
    fn load(&mut self) -> Result<Box<dyn SynthEngine>, String> {
        Ok(Box::new(FakeEngine {
            shared: self.shared.clone(),
        }))
    }
}

struct Rig {
    synth: NaturalSynth,
    shared: Arc<Shared>,
    device: FakeDevice,
    health: Arc<NaturalHealth>,
    events: Arc<Mutex<Vec<FailureReason>>>,
}

fn rig(script: Script, mode: TextMode) -> Rig {
    let shared = Arc::new(Shared {
        calls: Mutex::new(Vec::new()),
        script: Mutex::new(script),
    });
    let factory = FakeStreamFactory::new(24_000);
    let sink = Arc::new(ManagedSink::new(factory.clone(), Duration::from_secs(3)));
    let health = NaturalHealth::new(Duration::from_secs(300));
    let events = Arc::new(Mutex::new(Vec::new()));
    let ev = events.clone();
    health.set_event_sink(Box::new(move |r| ev.lock().unwrap().push(r)));
    let config = NaturalConfig {
        idle_unload: Duration::from_secs(60),
        tick: ms(20),
        text_mode: mode,
        ..NaturalConfig::default()
    };
    let synth = NaturalSynth::new(
        Box::new(FakeLoader {
            shared: shared.clone(),
        }),
        sink,
        health.clone(),
        config,
    );
    let device = FakeDevice::start(factory.clone(), 240, ms(10));
    Rig {
        synth,
        shared,
        device,
        health,
        events,
    }
}

impl Rig {
    fn say(&mut self, text: &str) {
        self.synth.speak(text, None, None, 0.0);
    }
    fn nonzero(&self) -> usize {
        self.device.nonzero()
    }
    fn texts(&self) -> Vec<String> {
        self.shared.calls.lock().unwrap().clone()
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

/// A Latin sentence of exactly `len` chars whose first word is `marker`.
fn sentence(marker: &str, len: usize) -> String {
    format!("{marker} {}.", "x".repeat(len - marker.len() - 2))
}

const TAMIL_LONG: &str = "ஒரு காலத்தில் ஒரு சிறிய கிராமத்தில் ஒரு அன்பான முயல் வாழ்ந்தது.";

/// True for Tamil combining marks (vowel signs, pulli, anusvara, visarga, AU length mark).
fn is_tamil_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0B82}' | '\u{0B83}' | '\u{0BBE}'..='\u{0BCD}' | '\u{0BD7}'
    )
}

// ---- Tamil mode -----------------------------------------------------------------------------------

#[test]
fn tamil_sentence_longer_than_48_chars_reaches_the_engine_as_one_chunk() {
    assert!(TAMIL_LONG.chars().count() > 48);
    let mut r = rig(quick(), TextMode::Tamil);
    r.say(TAMIL_LONG);
    assert!(r.wait_idle(5));
    assert_eq!(r.texts(), vec![TAMIL_LONG.to_string()]);
    assert_eq!(r.health.state(), HealthState::Ready);
}

#[test]
fn tamil_text_keeps_digits_and_pulli_and_is_never_spelled_out() {
    // normalize_for_speech would spell 2024 in English and may drop the pulli: Tamil mode must
    // hand the engine the text with both intact.
    let mut r = rig(quick(), TextMode::Tamil);
    r.say("இன்று 2024 வணக்கம்");
    assert!(r.wait_idle(5));
    assert_eq!(r.texts(), vec!["இன்று 2024 வணக்கம்".to_string()]);
}

#[test]
fn tamil_mode_english_only_text_is_a_no_op_and_the_worker_stays_healthy() {
    let mut r = rig(quick(), TextMode::Tamil);
    r.say("Hello there, this is English only.");
    assert!(r.wait_idle(3), "nothing submitted, so nothing is busy");
    assert!(r.texts().is_empty(), "the engine must never be called");
    assert_eq!(r.health.state(), HealthState::Ready);
    assert!(r.events().is_empty());
    r.say("இது ஒரு சோதனை.");
    assert!(r.wait_idle(5));
    assert_eq!(r.texts(), vec!["இது ஒரு சோதனை.".to_string()]);
    assert_eq!(r.health.state(), HealthState::Ready);
}

#[test]
fn tamil_engine_ok_empty_chunk_is_a_skip_and_the_next_chunk_is_still_synthesised() {
    let mut r = rig(
        Script {
            empty_word: Some("வெற்று".into()),
            ..quick()
        },
        TextMode::Tamil,
    );
    r.say("முதல் வாக்கியம். இரண்டாவது வெற்று. மூன்றாவது வாக்கியம்.");
    assert!(r.wait_idle(5));
    assert_eq!(
        r.texts(),
        vec![
            "முதல் வாக்கியம்.".to_string(),
            "இரண்டாவது வெற்று.".to_string(),
            "மூன்றாவது வாக்கியம்.".to_string(),
        ]
    );
    assert_eq!(r.health.state(), HealthState::Ready);
    assert!(r.events().is_empty());
}

#[test]
fn tamil_very_short_valid_audio_is_never_retried() {
    // 1440 samples is far below the Latin truncation threshold, which would split and retry.
    let mut r = rig(
        Script {
            fixed_samples: Some(1_440),
            ..Script::default()
        },
        TextMode::Tamil,
    );
    r.say("ஒரு வாக்கியம். இன்னொரு வாக்கியம்.");
    assert!(r.wait_idle(5));
    assert_eq!(
        r.texts(),
        vec!["ஒரு வாக்கியம்.".to_string(), "இன்னொரு வாக்கியம்.".to_string(),],
        "exactly one call per chunk"
    );
    assert_eq!(r.health.state(), HealthState::Ready);
}

#[test]
fn tamil_engine_error_marks_health_failed_once_and_later_calls_behave_as_latin() {
    let mut r = rig(
        Script {
            fail_all: true,
            ..quick()
        },
        TextMode::Tamil,
    );
    r.say("ஒரு வாக்கியம்.");
    assert!(r.wait_idle(3));
    assert_eq!(r.health.state(), HealthState::Failed(FailureReason::Synth));
    assert_eq!(r.events(), vec![FailureReason::Synth]);
    // Same as the Latin failure test: a later submission is still attempted, and the failure
    // is neither re-applied nor re-announced while the health stays failed.
    r.say("இன்னொரு வாக்கியம்.");
    assert!(r.wait_idle(3));
    assert_eq!(
        r.texts(),
        vec!["ஒரு வாக்கியம்.".to_string(), "இன்னொரு வாக்கியம்.".to_string(),]
    );
    assert_eq!(r.health.state(), HealthState::Failed(FailureReason::Synth));
    assert_eq!(r.events(), vec![FailureReason::Synth], "one event only");
}

#[test]
fn tamil_unspaced_700_chars_split_into_chunks_that_concatenate_to_the_input() {
    let input = "தமிழ்".repeat(140);
    assert_eq!(input.chars().count(), 700);
    let mut r = rig(quick(), TextMode::Tamil);
    r.say(&input);
    assert!(r.wait_idle(10));
    let chunks = r.texts();
    assert!(chunks.len() >= 2, "700 chars must be split: {chunks:?}");
    assert_eq!(chunks.concat(), input, "no char lost or duplicated");
    for chunk in &chunks {
        let first = chunk.chars().next();
        assert!(
            first.is_some_and(|c| !is_tamil_combining_mark(c)),
            "chunk starts with a combining mark: {chunk:?}"
        );
        assert!(chunk.chars().count() <= 600, "chunk over 600 chars");
    }
    assert_eq!(r.health.state(), HealthState::Ready);
}

#[test]
fn tamil_stop_in_the_middle_of_a_multi_chunk_utterance_stops_further_synthesis() {
    let text = ["ஒரு வாக்கியம்."; 5].join(" ");
    let mut r = rig(
        Script {
            delay: ms(300),
            fixed_samples: Some(24_000),
            ..Script::default()
        },
        TextMode::Tamil,
    );
    r.say(&text);
    assert!(wait_until(Duration::from_secs(5), || r.nonzero() > 0));
    assert_eq!(r.synth.stop_immediate(), StopOutcome::Stopped);
    assert!(!r.synth.is_speaking());
    let played = r.nonzero();
    let calls_at_stop = r.texts().len();
    std::thread::sleep(ms(400));
    assert_eq!(r.nonzero(), played, "no audio after a confirmed stop");
    assert!(
        r.texts().len() <= calls_at_stop + 1,
        "no new synthesis after stop"
    );
    assert!(
        r.texts().len() < 5,
        "the utterance was multi-chunk and stopped early"
    );
}

// ---- Latin mode regression ------------------------------------------------------------------------

#[test]
fn latin_engine_still_gets_normalized_text_and_the_48_char_first_chunk_cap() {
    let mut r = rig(Script::default(), TextMode::Latin);
    r.say("I have 3 apples and 50% off.");
    assert!(r.wait_idle(5));
    assert_eq!(
        r.texts(),
        vec![normalize_for_speech("I have 3 apples and 50% off.")]
    );
    assert_eq!(
        r.texts(),
        vec!["I have three apples and fifty percent off.".to_string()]
    );

    let text = format!("{} {}", sentence("alpha", 48), sentence("bravo", 66));
    let mut r = rig(Script::default(), TextMode::Latin);
    r.say(&text);
    assert!(r.wait_idle(10));
    let texts = r.texts();
    assert!(texts.len() >= 2, "Latin text is still split: {texts:?}");
    assert!(
        texts[0].chars().count() <= 48,
        "Latin first chunk is capped at 48 chars: {:?}",
        texts[0]
    );
}

#[test]
fn tamil_chunks_without_a_tamil_letter_are_dropped_one_by_one() {
    // "..." and "2024." carry no Tamil letter, so only the two Tamil sentences reach the engine.
    let mut r = rig(quick(), TextMode::Tamil);
    r.say("வணக்கம். ... 2024. நன்றி");
    assert!(r.wait_idle(5));
    assert_eq!(r.texts(), vec!["வணக்கம்.".to_string(), "நன்றி".to_string()]);
    assert_eq!(r.health.state(), HealthState::Ready);
}

#[test]
fn tamil_digits_only_or_dots_only_text_is_a_no_op() {
    for text in ["2024 2025.", "...", "— 10, 20!"] {
        let mut r = rig(quick(), TextMode::Tamil);
        r.say(text);
        assert!(r.wait_idle(3), "nothing submitted for {text:?}");
        std::thread::sleep(ms(50));
        assert!(
            r.texts().is_empty(),
            "engine must not be called for {text:?}"
        );
        assert_eq!(r.health.state(), HealthState::Ready);
        assert!(r.events().is_empty());
    }
}

#[test]
fn tamil_digits_inside_a_chunk_with_letters_stay_in_one_chunk() {
    let mut r = rig(quick(), TextMode::Tamil);
    r.say("ஆண்டு 2024 ஆகும்");
    assert!(r.wait_idle(5));
    assert_eq!(r.texts(), vec!["ஆண்டு 2024 ஆகும்".to_string()]);
}
