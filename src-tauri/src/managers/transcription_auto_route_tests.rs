//! Tests for the auto language decision as one pure function, `route_auto`, with every
//! environment fact passed in: fake language identifier, fake Tamil pack, and counting closures
//! for pack readiness and pack use. No model file and no app state are needed.

use super::*;
use crate::managers::auto_lang::{LidIdentify, LidOutcome, LidResult, TamilPackRoute};
use std::cell::Cell;

/// Parakeet-like output: 6 words, 24 token rows (tokens per word 4.0), gate true.
const GIBBERISH_TEXT: &str = "Nalai Kalai Unbadu Manika Kutam Mirakir";
/// Ordinary English: 6 words, 12 token rows (tokens per word 2.0), gate false.
const ENGLISH_TEXT: &str = "What is the weather like today?";
const HINDI_TEXT: &str = "नमस्ते दुनिया यह एक परीक्षण है";
const TAMIL_TEXT: &str = "வணக்கம் இன்று எப்படி இருக்கிறீர்கள்";

struct FakeLid {
    reply: LidOutcome,
    calls: Cell<usize>,
}

impl FakeLid {
    fn heard(lang: &str, text: &str) -> Self {
        FakeLid {
            reply: LidOutcome::Done(LidResult {
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
        self.reply.clone()
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

/// Counts the readiness closure and the pack-use callback.
#[derive(Default)]
struct Probe {
    ready: bool,
    ready_calls: Cell<usize>,
    use_calls: Cell<usize>,
}

impl Probe {
    fn with_pack(ready: bool) -> Self {
        Probe {
            ready,
            ..Probe::default()
        }
    }
}

/// One set of environment facts. `gated()` is the default: opted in, language auto, a
/// gate-true utterance, Tamil enabled, active model without language detection.
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
    fn gated() -> Self {
        Case {
            enabled: true,
            selected: "auto",
            translate: false,
            detects: false,
            audio_secs: 2.0,
            text: GIBBERISH_TEXT,
            tokens: Some(24),
            detected: None,
            tamil_enabled: true,
        }
    }
}

fn run(
    case: &Case,
    probe: &Probe,
    lid: &FakeLid,
    tamil: &FakeTamil,
) -> (AutoApplied, Option<AutoLogFacts>) {
    let audio = vec![0.0f32; (case.audio_secs * SAMPLE_RATE_HZ) as usize];
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

fn assert_no_calls(probe: &Probe, lid: &FakeLid, tamil: &FakeTamil) {
    assert_eq!(lid.calls.get(), 0, "language check must not run");
    assert_eq!(tamil.calls.get(), 0, "Tamil route must not run");
    assert_eq!(
        probe.ready_calls.get(),
        0,
        "pack readiness must not be checked"
    );
    assert_eq!(probe.use_calls.get(), 0, "pack must not be marked as used");
}

#[test]
fn c3_default_settings_with_ready_pack_and_wrong_hindi_never_call_anything() {
    let mut case = Case::gated();
    case.enabled = false;
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, facts) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(facts, None);
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn explicit_language_never_calls_anything() {
    let mut case = Case::gated();
    case.selected = "hi";
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn translate_to_english_never_calls_anything() {
    let mut case = Case::gated();
    case.translate = true;
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn active_model_with_detection_and_no_detected_language_keeps_active() {
    let mut case = Case::gated();
    case.detects = true;
    case.detected = None;
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, facts) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(facts, None);
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn ordinary_english_below_the_gate_never_checks_pack_or_language() {
    let mut case = Case::gated();
    case.text = ENGLISH_TEXT;
    case.tokens = Some(12);
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, facts) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(
        facts,
        Some(AutoLogFacts {
            gate: false,
            source: "keep".to_string(),
            tokens_per_word: Some(2.0),
        })
    );
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn gate_true_and_wrong_language_with_replacement_text_is_replaced() {
    let probe = Probe::with_pack(false);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let (applied, _) = run(&Case::gated(), &probe, &lid, &tamil);
    assert_eq!(
        applied,
        AutoApplied::Replaced {
            text: HINDI_TEXT.to_string(),
            lang: "hi".to_string(),
        }
    );
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn gate_true_and_language_check_says_english_keeps_active() {
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("en", "Hello there my friend");
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, facts) = run(&Case::gated(), &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 0);
    assert_eq!(
        facts,
        Some(AutoLogFacts {
            gate: true,
            source: "keep".to_string(),
            tokens_per_word: Some(4.0),
        })
    );
}

#[test]
fn detected_tamil_with_ready_pack_uses_the_pack_once() {
    let mut case = Case::gated();
    case.detects = true;
    case.detected = Some("ta");
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::Tamil(TAMIL_TEXT.to_string()));
    assert_eq!(tamil.calls.get(), 1);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(probe.use_calls.get(), 1);
}

#[test]
fn detected_tamil_when_pack_returns_none_keeps_active() {
    let mut case = Case::gated();
    case.detects = true;
    case.detected = Some("ta");
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(None);
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(tamil.calls.get(), 1);
    assert_eq!(lid.calls.get(), 0);
}

#[test]
fn detected_tamil_with_pack_not_ready_keeps_active_without_pack_calls() {
    let mut case = Case::gated();
    case.detects = true;
    case.detected = Some("ta");
    let probe = Probe::with_pack(false);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(tamil.calls.get(), 0);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(probe.use_calls.get(), 0);
}

#[test]
fn detected_tamil_with_tamil_disabled_checks_nothing() {
    let mut case = Case::gated();
    case.detects = true;
    case.detected = Some("ta");
    case.tamil_enabled = false;
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("ta", TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(tamil.calls.get(), 0);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(probe.ready_calls.get(), 0);
    assert_eq!(probe.use_calls.get(), 0);
}

#[test]
fn audio_over_29_seconds_is_never_replaced() {
    // Design Revision 4 (C1): the language check still runs on the first 30 s, but its text
    // may only replace audio of 29 s or less. The brief asked for zero language-check calls
    // here; that conflicts with the design and the existing adapter test, so it is not asserted.
    let mut case = Case::gated();
    case.audio_secs = 40.0;
    let probe = Probe::with_pack(false);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let (applied, _) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
}

#[test]
fn missing_token_rows_never_call_anything() {
    let mut case = Case::gated();
    case.tokens = None;
    let probe = Probe::with_pack(true);
    let lid = FakeLid::heard("hi", HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_TEXT));
    let (applied, facts) = run(&case, &probe, &lid, &tamil);
    assert_eq!(applied, AutoApplied::KeepActive);
    assert_eq!(facts, None);
    assert_no_calls(&probe, &lid, &tamil);
}

#[test]
fn lid_tick_policy_enabled_never_keeps_the_timeout() {
    let policy = lid_tick_policy(true, ModelUnloadTimeout::Never);
    assert!(!matches!(policy, UnloadPolicy::Immediately));
    assert_eq!(policy, UnloadPolicy::Never);
}

#[test]
fn lid_tick_policy_enabled_immediately_is_immediately() {
    let policy = lid_tick_policy(true, ModelUnloadTimeout::Immediately);
    assert_eq!(policy, UnloadPolicy::Immediately);
}

#[test]
fn lid_tick_policy_disabled_never_unloads_immediately() {
    let policy = lid_tick_policy(false, ModelUnloadTimeout::Never);
    assert_eq!(policy, UnloadPolicy::Immediately);
}

#[test]
fn lid_tick_policy_disabled_min5_unloads_immediately() {
    let policy = lid_tick_policy(false, ModelUnloadTimeout::Min5);
    assert_eq!(policy, UnloadPolicy::Immediately);
}
