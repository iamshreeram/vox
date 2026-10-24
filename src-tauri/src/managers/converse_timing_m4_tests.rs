//! M4 S5 tests: latency timing lines (observability only). Included from converse.rs.
//! Do not edit to make code pass.
//!
//! Unavailable values print as `-`. A timing line never carries transcript or reply text.
//! Timing must not change the mouth or reporter call sequence.

use super::*;
use crate::managers::converse_plan::{PlanKind, TurnPlan};
use crate::managers::converse_platform::transcribe_line;
use std::collections::VecDeque;
use tokio::sync::watch;

type Shared = Arc<Mutex<Vec<String>>>;

// ---- fakes -------------------------------------------------------------------------------

struct TQueue {
    log: Shared,
}
impl SpeechQueue for TQueue {
    fn push(&self, text: &str, _emotion: Emotion) -> bool {
        self.log.lock().unwrap().push(format!("push:{text}"));
        true
    }
    fn abort(&self) {
        self.log.lock().unwrap().push("abort".into());
    }
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        self.log.lock().unwrap().push("finish".into());
        Box::pin(async { SayOutcome::Done })
    }
}

struct TMouth {
    log: Shared,
    /// Offers a speech queue (streaming path) when true; otherwise only `say` exists.
    offer: bool,
}
impl Mouth for TMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.lock().unwrap().push(format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {
        self.log.lock().unwrap().push("stop".into());
    }
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        if !self.offer {
            return None;
        }
        self.log.lock().unwrap().push("open".into());
        let queue: Arc<dyn SpeechQueue> = Arc::new(TQueue {
            log: self.log.clone(),
        });
        Some(queue)
    }
}

struct TEars {
    log: Shared,
    script: Mutex<VecDeque<String>>,
}
impl Ears for TEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.log.lock().unwrap().push("listen".into());
        let next = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(utterance) => ListenOutcome::Utterance(utterance),
                None => ListenOutcome::Aborted,
            }
        })
    }
}

struct THands {
    log: Shared,
}
impl Hands for THands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("paste:{text}"));
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.log.lock().unwrap().push(format!("model:{phrase}"));
        Box::pin(async { PhraseOutcome::Done("ok".into()) })
    }
}

/// One scripted turn: an optional streamed delta (raw brain JSON) and the final plan.
struct TBrain {
    reply: Mutex<Option<Result<TurnPlan, PlanError>>>,
    delta: Option<String>,
}
impl TBrain {
    fn take_reply(&self) -> Result<TurnPlan, PlanError> {
        let next = self.reply.lock().unwrap().take();
        next.unwrap_or(Err(PlanError::Backend))
    }
}
impl Brain for TBrain {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let reply = self.take_reply();
        Box::pin(async move { reply })
    }
    fn plan_streaming<'a>(
        &'a self,
        _input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let reply = self.take_reply();
        let delta = self.delta.clone();
        Box::pin(async move {
            if let Some(delta) = delta {
                on_text(&delta);
            }
            reply
        })
    }
}

#[derive(Default)]
struct TReporter {
    states: Mutex<Vec<SessionState>>,
    timing: Mutex<Vec<String>>,
}
impl Reporter for TReporter {
    fn state(&self, state: SessionState) {
        self.states.lock().unwrap().push(state);
    }
    fn timing_line(&self, line: &str) {
        self.timing.lock().unwrap().push(line.to_string());
    }
}

/// Everything observable about one session run.
#[derive(Debug)]
struct Outcome {
    log: Vec<String>,
    states: Vec<SessionState>,
    timing: Vec<String>,
    end: SessionEnd,
}

/// Runs one session with a single scripted utterance (the ears then abort, ending the session).
/// `queue` selects the streaming path (mouth offers a queue and early speech is on).
/// `cancelled` cancels the session before it starts.
async fn run_turn(
    queue: bool,
    timing: bool,
    cancelled: bool,
    reply: Result<TurnPlan, PlanError>,
    delta: Option<String>,
) -> Outcome {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let ears = TEars {
        log: log.clone(),
        script: Mutex::new(VecDeque::from(vec!["tell me about the zebra".to_string()])),
    };
    let mouth = TMouth {
        log: log.clone(),
        offer: queue,
    };
    let hands = THands { log: log.clone() };
    let brain = TBrain {
        reply: Mutex::new(Some(reply)),
        delta,
    };
    let reporter = TReporter::default();
    let (tx, rx) = watch::channel(false);
    if cancelled {
        tx.send_replace(true);
    }
    let cfg = SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: queue,
        ack_after: None,
        timing_log: timing,
        ..SessionConfig::default()
    };
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &reporter,
    };
    let end = tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg, rx))
        .await
        .expect("run_session hung");
    drop(tx);
    let log = log.lock().unwrap().clone();
    let states = reporter.states.lock().unwrap().clone();
    let timing = reporter.timing.lock().unwrap().clone();
    Outcome {
        log,
        states,
        timing,
        end,
    }
}

fn answer(speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        role: None,
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    }
}

/// Raw brain JSON for one answer, streamed as a single delta.
fn answer_json(speech: &str) -> String {
    format!(r#"{{"kind":"answer","speech":"{speech}"}}"#)
}

/// The value of `key` in a `key=value` timing line, if present.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(key)?.strip_prefix('='))
}

fn is_number(value: &str) -> bool {
    value.parse::<u64>().is_ok()
}

const REPLY: &str = "The quokka sentence here.";

// ---- format_timing (test 1) ----------------------------------------------------------------

#[test]
fn format_timing_prints_all_four_values_from_synthetic_instants() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let line = format_timing(base, Some(at(120)), Some(at(340)), at(410), at(900));
    assert_eq!(
        line,
        "converse timing: first_text_ms=120 first_speech_ms=340 plan_ms=410 total_ms=900"
    );
}

#[test]
fn format_timing_prints_dash_when_first_text_is_missing() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let line = format_timing(base, None, Some(at(340)), at(410), at(900));
    assert_eq!(
        line,
        "converse timing: first_text_ms=- first_speech_ms=340 plan_ms=410 total_ms=900"
    );
}

#[test]
fn format_timing_prints_dash_when_first_speech_is_missing() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let line = format_timing(base, Some(at(120)), None, at(410), at(900));
    assert_eq!(
        line,
        "converse timing: first_text_ms=120 first_speech_ms=- plan_ms=410 total_ms=900"
    );
}

#[test]
fn format_timing_prints_dash_for_both_optional_values_when_both_missing() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let line = format_timing(base, None, None, at(410), at(900));
    assert_eq!(
        line,
        "converse timing: first_text_ms=- first_speech_ms=- plan_ms=410 total_ms=900"
    );
}

#[test]
fn format_timing_zero_offsets_print_zero() {
    let base = Instant::now();
    let line = format_timing(base, Some(base), Some(base), base, base);
    assert_eq!(
        line,
        "converse timing: first_text_ms=0 first_speech_ms=0 plan_ms=0 total_ms=0"
    );
}

#[test]
fn format_timing_instant_before_tier_start_saturates_to_zero_without_panic() {
    let base = Instant::now();
    let at = |ms: u64| base + Duration::from_millis(ms);
    let line = format_timing(
        base + Duration::from_millis(100),
        Some(at(40)),
        Some(at(50)),
        at(100),
        at(100),
    );
    assert_eq!(
        line,
        "converse timing: first_text_ms=0 first_speech_ms=0 plan_ms=0 total_ms=0"
    );
}

// ---- format_transcribe (test 2) -----------------------------------------------------------

#[test]
fn format_transcribe_zero_values_exact_string() {
    assert_eq!(
        format_transcribe(0, 0),
        "converse transcribe: ms=0 audio_ms=0"
    );
}

#[test]
fn format_transcribe_typical_values_exact_string() {
    assert_eq!(
        format_transcribe(250, 3200),
        "converse transcribe: ms=250 audio_ms=3200"
    );
}

#[test]
fn format_transcribe_huge_values_do_not_panic() {
    assert_eq!(
        format_transcribe(u64::MAX, u64::MAX),
        format!("converse transcribe: ms={} audio_ms={}", u64::MAX, u64::MAX)
    );
}

// ---- transcribe_line (test 3) -------------------------------------------------------------

#[test]
fn transcribe_line_is_none_when_timing_is_disabled() {
    assert_eq!(transcribe_line(false, 180, 16_000, 16_000), None);
}

#[test]
fn transcribe_line_reports_one_second_clip_at_16khz() {
    assert_eq!(
        transcribe_line(true, 180, 16_000, 16_000).as_deref(),
        Some("converse transcribe: ms=180 audio_ms=1000")
    );
}

#[test]
fn transcribe_line_zero_sample_rate_gives_zero_audio_ms_without_divide_by_zero() {
    assert_eq!(
        transcribe_line(true, 5, 16_000, 0).as_deref(),
        Some("converse transcribe: ms=5 audio_ms=0")
    );
}

#[test]
fn transcribe_line_zero_samples_gives_zero_audio_ms() {
    assert_eq!(
        transcribe_line(true, 7, 0, 16_000).as_deref(),
        Some("converse transcribe: ms=7 audio_ms=0")
    );
}

#[test]
fn transcribe_line_huge_sample_count_does_not_overflow() {
    // usize::MAX samples at 1 Hz: the audio length must not panic or overflow; only the
    // prefix and a numeric audio_ms are asserted (the saturation value is not specified).
    let line = transcribe_line(true, 5, usize::MAX, 1);
    let line = line.unwrap_or_default();
    assert!(
        line.starts_with("converse transcribe: ms=5 audio_ms="),
        "{line}"
    );
    let audio = line.rsplit('=').next().unwrap_or_default();
    assert!(audio.parse::<u64>().is_ok(), "{line}");
}

#[test]
fn transcribe_line_rounds_audio_length_down() {
    // 100 samples at 44100 Hz is about 2.27 ms of audio; floor gives 2.
    assert_eq!(
        transcribe_line(true, 9, 100, 44_100).as_deref(),
        Some("converse transcribe: ms=9 audio_ms=2")
    );
}

// ---- streaming path (tests 4, 6, 7) -------------------------------------------------------

#[tokio::test]
async fn streaming_turn_records_exactly_one_timing_line_with_a_numeric_first_speech() {
    let r = run_turn(
        true,
        true,
        false,
        Ok(answer(REPLY)),
        Some(answer_json(REPLY)),
    )
    .await;
    assert_eq!(r.timing.len(), 1, "one line per turn: {:?}", r.timing);
    let line = &r.timing[0];
    assert!(line.starts_with("converse timing:"), "{line}");
    assert!(
        is_number(field(line, "first_speech_ms").unwrap_or("-")),
        "{line}"
    );
    assert!(
        is_number(field(line, "first_text_ms").unwrap_or("-")),
        "{line}"
    );
    assert!(is_number(field(line, "plan_ms").unwrap_or("-")), "{line}");
    assert!(is_number(field(line, "total_ms").unwrap_or("-")), "{line}");
}

#[tokio::test]
async fn streaming_turn_that_speaks_nothing_prints_dash_for_first_speech() {
    let r = run_turn(true, true, false, Ok(answer("")), None).await;
    assert_eq!(r.timing.len(), 1, "{:?}", r.timing);
    assert_eq!(
        field(&r.timing[0], "first_speech_ms"),
        Some("-"),
        "{:?}",
        r.timing
    );
}

#[tokio::test]
async fn streaming_timing_line_never_contains_transcript_or_reply_words() {
    let r = run_turn(
        true,
        true,
        false,
        Ok(answer(REPLY)),
        Some(answer_json(REPLY)),
    )
    .await;
    assert_eq!(r.timing.len(), 1, "{:?}", r.timing);
    let line = &r.timing[0];
    for word in ["zebra", "quokka", "tell", "sentence"] {
        assert!(!line.contains(word), "timing line leaks {word:?}: {line}");
    }
}

#[tokio::test]
async fn cancelled_before_planning_records_no_timing_line_on_the_streaming_path() {
    let r = run_turn(true, true, true, Ok(answer(REPLY)), None).await;
    assert_eq!(r.end, SessionEnd::Cancelled);
    assert!(r.timing.is_empty(), "{:?}", r.timing);
}

#[tokio::test]
async fn timing_on_or_off_leaves_streaming_mouth_and_reporter_calls_identical() {
    let on = run_turn(
        true,
        true,
        false,
        Ok(answer(REPLY)),
        Some(answer_json(REPLY)),
    )
    .await;
    let off = run_turn(
        true,
        false,
        false,
        Ok(answer(REPLY)),
        Some(answer_json(REPLY)),
    )
    .await;
    assert_eq!(on.log, off.log, "mouth/hands calls must not change");
    assert_eq!(on.states, off.states, "reporter states must not change");
    assert_eq!(on.end, off.end);
    assert_eq!(
        on.timing.len(),
        1,
        "timing on records one line: {:?}",
        on.timing
    );
    assert!(
        off.timing.is_empty(),
        "timing off records nothing: {:?}",
        off.timing
    );
}

// ---- non-streaming path (tests 5, 6, 7) ---------------------------------------------------

#[tokio::test]
async fn non_streaming_turn_records_one_timing_line_with_dashes_for_text_values() {
    let r = run_turn(false, true, false, Ok(answer(REPLY)), None).await;
    assert_eq!(r.timing.len(), 1, "one line per turn: {:?}", r.timing);
    let line = &r.timing[0];
    assert!(line.starts_with("converse timing:"), "{line}");
    assert_eq!(field(line, "first_text_ms"), Some("-"), "{line}");
    assert!(
        is_number(field(line, "first_speech_ms").unwrap_or("-")),
        "{line}"
    );
    let plan_ms = field(line, "plan_ms").and_then(|v| v.parse::<u64>().ok());
    let total_ms = field(line, "total_ms").and_then(|v| v.parse::<u64>().ok());
    assert!(plan_ms.is_some(), "{line}");
    assert!(total_ms.is_some(), "{line}");
    assert!(
        total_ms >= plan_ms,
        "total_ms must not be below plan_ms: {line}"
    );
}

#[tokio::test]
async fn non_streaming_turn_that_speaks_nothing_prints_dash_for_first_speech() {
    let r = run_turn(false, true, false, Ok(answer("")), None).await;
    assert_eq!(r.timing.len(), 1, "{:?}", r.timing);
    let line = &r.timing[0];
    assert_eq!(field(line, "first_speech_ms"), Some("-"), "{line}");
    assert_eq!(field(line, "first_text_ms"), Some("-"), "{line}");
    assert!(is_number(field(line, "plan_ms").unwrap_or("-")), "{line}");
    assert!(is_number(field(line, "total_ms").unwrap_or("-")), "{line}");
}

#[tokio::test]
async fn non_streaming_timing_line_never_contains_transcript_or_reply_words() {
    let r = run_turn(false, true, false, Ok(answer(REPLY)), None).await;
    assert_eq!(r.timing.len(), 1, "{:?}", r.timing);
    let line = &r.timing[0];
    for word in ["zebra", "quokka", "tell", "sentence"] {
        assert!(!line.contains(word), "timing line leaks {word:?}: {line}");
    }
}

#[tokio::test]
async fn timing_disabled_records_no_line_on_the_non_streaming_path() {
    let r = run_turn(false, false, false, Ok(answer(REPLY)), None).await;
    assert!(r.timing.is_empty(), "{:?}", r.timing);
    assert!(has_say(&r.log, REPLY), "{:?}", r.log);
}

#[tokio::test]
async fn cancelled_before_planning_records_no_timing_line_on_the_non_streaming_path() {
    let r = run_turn(false, true, true, Ok(answer(REPLY)), None).await;
    assert_eq!(r.end, SessionEnd::Cancelled);
    assert!(r.timing.is_empty(), "{:?}", r.timing);
}

#[tokio::test]
async fn timing_on_or_off_leaves_non_streaming_mouth_and_reporter_calls_identical() {
    let on = run_turn(false, true, false, Ok(answer(REPLY)), None).await;
    let off = run_turn(false, false, false, Ok(answer(REPLY)), None).await;
    assert_eq!(on.log, off.log, "mouth/hands calls must not change");
    assert_eq!(on.states, off.states, "reporter states must not change");
    assert_eq!(on.end, off.end);
    assert_eq!(
        on.timing.len(),
        1,
        "timing on records one line: {:?}",
        on.timing
    );
    assert!(
        off.timing.is_empty(),
        "timing off records nothing: {:?}",
        off.timing
    );
}

fn has_say(log: &[String], text: &str) -> bool {
    log.iter().any(|entry| entry == &format!("say:{text}"))
}
