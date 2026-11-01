//! Route-level tests for the empty-text path: `route_auto_empty` with every environment fact
//! passed in (fake identifier, fake Tamil pack, fake speech meter, counting closures). Written
//! first (step B3); against the stub every case that reaches `route_auto_empty` panics with
//! `step B4`. `route_auto` itself is unchanged and is checked once for blank text.

use super::*;
use crate::managers::auto_lang::{LidIdentify, LidOutcome, LidResult, TamilPackRoute};
use crate::managers::auto_lang_empty::{format_empty_log, AutoEmptyLogFacts, SpeechMeter};
use std::cell::Cell;

const TAMIL_TEXT: &str = "வணக்கம் இன்று எப்படி இருக்கிறீர்கள்";
const TAMIL_PACK_TEXT: &str = "தமிழ் உரை சோதனை";
const HINDI_TEXT: &str = "नमस्ते दुनिया यह एक परीक्षण है";
const JA_HALLUCINATION: &str = "ご視聴ありがとうございました";
const GIBBERISH_TEXT: &str = "Nalai Kalai Unbadu Manika Kutam Mirakir";

struct FakeLid {
    outcome: LidOutcome,
    calls: Cell<usize>,
}

impl FakeLid {
    fn heard(lang: &str, text: &str) -> Self {
        FakeLid {
            outcome: LidOutcome::Done(LidResult {
                lang: Some(lang.to_string()),
                text: text.to_string(),
            }),
            calls: Cell::new(0),
        }
    }
}

impl LidIdentify for FakeLid {
    fn identify(&self, _samples: &[f32]) -> LidOutcome {
        self.calls.set(self.calls.get() + 1);
        self.outcome.clone()
    }
}

struct FakeTamil {
    reply: Option<String>,
    calls: Cell<usize>,
}

impl FakeTamil {
    fn returns(reply: Option<&str>) -> Self {
        FakeTamil {
            reply: reply.map(str::to_string),
            calls: Cell::new(0),
        }
    }
}

impl TamilPackRoute for FakeTamil {
    fn transcribe(&self, _samples: &[f32]) -> Option<String> {
        self.calls.set(self.calls.get() + 1);
        self.reply.clone()
    }
}

/// Fixed speech seconds, counting calls.
struct FakeMeter {
    seconds: f32,
    calls: Cell<usize>,
}

impl FakeMeter {
    fn reading(seconds: f32) -> Self {
        FakeMeter {
            seconds,
            calls: Cell::new(0),
        }
    }
}

impl SpeechMeter for FakeMeter {
    fn speech_seconds(&self, _audio: &[f32]) -> f32 {
        self.calls.set(self.calls.get() + 1);
        self.seconds
    }
}

/// Counts the readiness closure and the pack-use callback.
struct Probe {
    ready: bool,
    ready_calls: Cell<usize>,
    use_calls: Cell<usize>,
}

impl Probe {
    fn with_pack(ready: bool) -> Self {
        Probe {
            ready,
            ready_calls: Cell::new(0),
            use_calls: Cell::new(0),
        }
    }
}

/// One set of environment facts. `blank()` is the default: opted in, language auto, a
/// two-second clip whose active text is empty and whose token count is zero.
struct Case {
    enabled: bool,
    selected: &'static str,
    translate: bool,
    detects: bool,
    audio_secs: f32,
    text: &'static str,
    tokens: Option<usize>,
    detected: Option<&'static str>,
    tamil_enabled: bool,
}

impl Case {
    fn blank() -> Self {
        Case {
            enabled: true,
            selected: "auto",
            translate: false,
            detects: false,
            audio_secs: 2.0,
            text: "",
            tokens: Some(0),
            detected: None,
            tamil_enabled: true,
        }
    }
}

fn run_empty(
    case: &Case,
    probe: &Probe,
    meter: &FakeMeter,
    lid: &FakeLid,
    tamil: &FakeTamil,
) -> Option<(AutoApplied, AutoEmptyLogFacts)> {
    let audio = vec![0.1f32; (case.audio_secs * SAMPLE_RATE_HZ) as usize];
    let ready = || {
        probe.ready_calls.set(probe.ready_calls.get() + 1);
        probe.ready
    };
    let used = || probe.use_calls.set(probe.use_calls.get() + 1);
    let ctx = AutoRouteCtx {
        settings_enabled: case.enabled,
        selected_language: case.selected,
        translate_to_english: case.translate,
        active_detects_language: case.detects,
        audio: &audio,
        active_text: case.text,
        token_count: case.tokens,
        detected_language: case.detected,
        tamil_enabled: case.tamil_enabled,
        tamil_pack_ready: &ready,
        on_pack_use: &used,
    };
    route_auto_empty(&ctx, lid, tamil, meter)
}

fn run_ordinary(
    case: &Case,
    probe: &Probe,
    lid: &FakeLid,
    tamil: &FakeTamil,
) -> (AutoApplied, Option<AutoLogFacts>) {
    let audio = vec![0.1f32; (case.audio_secs * SAMPLE_RATE_HZ) as usize];
    let ready = || {
        probe.ready_calls.set(probe.ready_calls.get() + 1);
        probe.ready
    };
    let used = || probe.use_calls.set(probe.use_calls.get() + 1);
    let ctx = AutoRouteCtx {
        settings_enabled: case.enabled,
        selected_language: case.selected,
        translate_to_english: case.translate,
        active_detects_language: case.detects,
        audio: &audio,
        active_text: case.text,
        token_count: case.tokens,
        detected_language: case.detected,
        tamil_enabled: case.tamil_enabled,
        tamil_pack_ready: &ready,
        on_pack_use: &used,
    };
    route_auto(&ctx, lid, tamil)
}

fn assert_nothing_ran(lid: &FakeLid, tamil: &FakeTamil, meter: &FakeMeter, probe: &Probe) {
    assert_eq!(lid.calls.get(), 0, "identifier must not run");
    assert_eq!(tamil.calls.get(), 0, "Tamil pack must not run");
    assert_eq!(meter.calls.get(), 0, "speech meter must not run");
    assert_eq!(probe.ready_calls.get(), 0, "readiness closure must not run");
    assert_eq!(probe.use_calls.get(), 0, "pack must not be marked used");
}

// ---------------------------------------------------------------------------
// Conditions that send the decision elsewhere (all return None, all calls zero)
// ---------------------------------------------------------------------------

#[test]
fn setting_off_returns_none_and_calls_nothing() {
    let mut case = Case::blank();
    case.enabled = false;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn explicit_tamil_language_returns_none() {
    let mut case = Case::blank();
    case.selected = "ta";
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn explicit_hindi_language_returns_none() {
    let mut case = Case::blank();
    case.selected = "hi";
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn translate_to_english_returns_none() {
    let mut case = Case::blank();
    case.translate = true;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn active_model_that_detects_language_returns_none() {
    let mut case = Case::blank();
    case.detects = true;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn missing_token_rows_return_none() {
    let mut case = Case::blank();
    case.tokens = None;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn clip_under_half_a_second_returns_none() {
    let mut case = Case::blank();
    case.audio_secs = 0.4;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

#[test]
fn non_blank_active_text_returns_none_even_with_high_token_ratio() {
    // Gibberish with 24 token rows over 6 words: the ordinary gate's case, not the empty path.
    let mut case = Case::blank();
    case.text = GIBBERISH_TEXT;
    case.tokens = Some(24);
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    assert_eq!(run_empty(&case, &probe, &meter, &lid, &tamil), None);
    assert_nothing_ran(&lid, &tamil, &meter, &probe);
}

// ---------------------------------------------------------------------------
// The empty path itself
// ---------------------------------------------------------------------------

#[test]
fn blank_text_without_speech_is_refused_with_keep_and_no_identifier_calls() {
    let case = Case::blank();
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(0.2);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    assert_eq!(
        result,
        Some((
            AutoApplied::KeepActive,
            AutoEmptyLogFacts {
                speech_s: 0.2,
                admitted: false,
                source: "keep".to_string(),
            }
        ))
    );
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
    assert_eq!(probe.ready_calls.get(), 0);
    assert_eq!(probe.use_calls.get(), 0);
    assert_eq!(meter.calls.get(), 1);
}

#[test]
fn whitespace_only_text_behaves_as_blank_without_speech() {
    let mut case = Case::blank();
    case.text = "  \n\t";
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(0.2);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    assert_eq!(
        result,
        Some((
            AutoApplied::KeepActive,
            AutoEmptyLogFacts {
                speech_s: 0.2,
                admitted: false,
                source: "keep".to_string(),
            }
        ))
    );
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn blank_text_with_speech_and_ready_pack_uses_tamil_pack_once() {
    let case = Case::blank();
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    let (applied, facts) = result.expect("empty path must decide");
    assert_eq!(applied, AutoApplied::Tamil(TAMIL_PACK_TEXT.to_string()));
    assert_eq!(facts.source, "tamil_pack");
    assert!(facts.admitted);
    assert_eq!(probe.ready_calls.get(), 1);
    assert_eq!(probe.use_calls.get(), 1);
    assert_eq!(tamil.calls.get(), 1);
}

#[test]
fn blank_text_with_speech_and_hindi_without_pack_replaces_with_lid_text() {
    let case = Case::blank();
    let probe = Probe::with_pack(false);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    let (applied, facts) = result.expect("empty path must decide");
    assert_eq!(
        applied,
        AutoApplied::Replaced {
            text: HINDI_TEXT.to_string(),
            lang: "hi".to_string(),
        }
    );
    assert_eq!(facts.source, "lid_text:hi");
    assert!(facts.admitted);
    assert_eq!(probe.use_calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn blank_text_with_speech_and_english_identifier_keeps_active_and_is_admitted() {
    let case = Case::blank();
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("en", "hello there my friend");
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    let (applied, facts) = result.expect("empty path must decide");
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(facts.source, "keep");
    assert!(facts.admitted);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn tamil_disabled_never_checks_readiness_and_uses_identifier_text() {
    let mut case = Case::blank();
    case.tamil_enabled = false;
    let probe = Probe::with_pack(true);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    let (applied, facts) = result.expect("empty path must decide");
    assert_eq!(
        applied,
        AutoApplied::Replaced {
            text: TAMIL_TEXT.to_string(),
            lang: "ta".to_string(),
        }
    );
    assert_eq!(facts.source, "lid_text:ta");
    assert_eq!(
        probe.ready_calls.get(),
        0,
        "readiness must not be checked when Tamil is disabled"
    );
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn japanese_hallucination_is_refused_but_identifier_was_asked() {
    let case = Case::blank();
    let probe = Probe::with_pack(false);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("ja", JA_HALLUCINATION);
    let tamil = FakeTamil::returns(None);
    let result = run_empty(&case, &probe, &meter, &lid, &tamil);
    let (applied, facts) = result.expect("empty path must decide");
    assert_eq!(applied, AutoApplied::KeepActive);
    assert!(facts.admitted);
    assert_eq!(lid.calls.get(), 1);
}

#[test]
fn facts_carry_the_measured_speech_seconds() {
    let case = Case::blank();
    let probe = Probe::with_pack(false);
    let meter = FakeMeter::reading(2.75);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let (_, facts) =
        run_empty(&case, &probe, &meter, &lid, &tamil).expect("empty path must decide");
    assert_eq!(facts.speech_s, 2.75);
}

#[test]
fn logged_line_for_empty_path_contains_no_transcript_text() {
    let case = Case::blank();
    let probe = Probe::with_pack(false);
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let (_, facts) =
        run_empty(&case, &probe, &meter, &lid, &tamil).expect("empty path must decide");
    let line = format_empty_log(&facts, 5);
    assert!(
        !line.contains(HINDI_TEXT),
        "log line must not carry transcript text"
    );
    assert!(
        !line.contains(TAMIL_TEXT),
        "log line must not carry transcript text"
    );
    assert!(
        !line.contains(TAMIL_PACK_TEXT),
        "log line must not carry transcript text"
    );
}

// ---------------------------------------------------------------------------
// route_auto is unchanged: the empty path is a separate function
// ---------------------------------------------------------------------------

#[test]
fn ordinary_route_still_keeps_blank_text_without_calling_identifier() {
    let case = Case::blank();
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let (applied, facts) = run_ordinary(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(
        facts,
        Some(AutoLogFacts {
            gate: false,
            source: "keep".to_string(),
            tokens_per_word: None,
        })
    );
    assert_eq!(lid.calls.get(), 0);
}
