//! Tests for the empty-text path (DESIGN Revision 5 and 6), written first (step B3).
//!
//! Against the step-B3 stubs every case that calls into `auto_lang_empty` panics with
//! `step B4`; the pure `format_empty_log` test also fails the same way. Nothing here
//! is weakened to match the stub: these tests are the specification.

use super::auto_lang::*;
use super::auto_lang_empty::*;
use std::cell::Cell;

const TAMIL_TEXT: &str = "வணக்கம் இன்று எப்படி இருக்கிறீர்கள்";
const TAMIL_PACK_TEXT: &str = "தமிழ் உரை சோதனை";
const TAMIL_LATIN_TEXT: &str = "vanakkam nanbare inru";
const TAMIL_ONE_WORD: &str = "வணக்கமிருக்கிறீர்கள்";
const HINDI_TEXT: &str = "नमस्ते दुनिया यह एक परीक्षण है";
const JA_HALLUCINATION: &str = "ご視聴ありがとうございました";
const ENGLISH_TEXT: &str = "hello there my friend";
const LATIN_FRENCH: &str = "merci beaucoup monsieur";

/// Returns fixed speech seconds and counts how often it was consulted.
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

/// Returns a fixed outcome, counts calls and records the length of the audio it received.
struct FakeLid {
    outcome: LidOutcome,
    calls: Cell<usize>,
    last_len: Cell<usize>,
}

impl FakeLid {
    fn heard(lang: Option<&str>, text: &str) -> Self {
        FakeLid {
            outcome: LidOutcome::Done(LidResult {
                lang: lang.map(str::to_string),
                text: text.to_string(),
            }),
            calls: Cell::new(0),
            last_len: Cell::new(0),
        }
    }

    fn with_outcome(outcome: LidOutcome) -> Self {
        FakeLid {
            outcome,
            calls: Cell::new(0),
            last_len: Cell::new(0),
        }
    }
}

impl LidIdentify for FakeLid {
    fn identify(&self, samples: &[f32]) -> LidOutcome {
        self.calls.set(self.calls.get() + 1);
        self.last_len.set(samples.len());
        self.outcome.clone()
    }
}

/// Returns a fixed Tamil pack answer, counts calls and records the audio length it received.
struct FakeTamil {
    reply: Option<String>,
    calls: Cell<usize>,
    last_len: Cell<usize>,
}

impl FakeTamil {
    fn returns(reply: Option<&str>) -> Self {
        FakeTamil {
            reply: reply.map(str::to_string),
            calls: Cell::new(0),
            last_len: Cell::new(0),
        }
    }
}

impl TamilPackRoute for FakeTamil {
    fn transcribe(&self, samples: &[f32]) -> Option<String> {
        self.calls.set(self.calls.get() + 1);
        self.last_len.set(samples.len());
        self.reply.clone()
    }
}

/// Audio of `secs` seconds at 16 kHz, filled with a constant non-zero value.
fn audio_of_secs(secs: f32) -> Vec<f32> {
    vec![0.1f32; (secs * SAMPLE_RATE_HZ) as usize]
}

// ---------------------------------------------------------------------------
// empty_text_check_allowed
// ---------------------------------------------------------------------------

#[test]
fn empty_text_blank_with_full_speech_is_allowed() {
    let meter = FakeMeter::reading(1.0);
    let audio = audio_of_secs(5.0);
    assert_eq!(empty_text_check_allowed("", &audio, &meter), (true, 1.0));
}

#[test]
fn empty_text_whitespace_only_with_full_speech_is_allowed() {
    let meter = FakeMeter::reading(1.0);
    let audio = audio_of_secs(5.0);
    assert_eq!(
        empty_text_check_allowed("  \n\t", &audio, &meter),
        (true, 1.0)
    );
}

#[test]
fn empty_text_speech_just_below_one_second_is_refused() {
    let meter = FakeMeter::reading(0.99);
    let audio = audio_of_secs(5.0);
    assert_eq!(empty_text_check_allowed("", &audio, &meter), (false, 0.99));
}

#[test]
fn empty_text_speech_exactly_one_second_is_allowed() {
    let meter = FakeMeter::reading(1.0);
    let audio = audio_of_secs(5.0);
    let (allowed, speech) = empty_text_check_allowed("", &audio, &meter);
    assert!(allowed, "exactly 1.0 s of speech must be allowed");
    assert_eq!(speech, 1.0);
}

#[test]
fn empty_text_non_blank_text_is_refused_without_consulting_meter() {
    let meter = FakeMeter::reading(5.0);
    let audio = audio_of_secs(5.0);
    assert_eq!(
        empty_text_check_allowed("hello", &audio, &meter),
        (false, 0.0)
    );
    assert_eq!(
        meter.calls.get(),
        0,
        "meter must not run for non-blank text"
    );
}

#[test]
fn empty_text_audio_just_below_one_second_is_refused_without_consulting_meter() {
    let meter = FakeMeter::reading(5.0);
    let audio = vec![0.1f32; 15_840];
    assert_eq!(empty_text_check_allowed("", &audio, &meter), (false, 0.0));
    assert_eq!(
        meter.calls.get(),
        0,
        "meter must not run for audio under 1.0 s"
    );
}

#[test]
fn empty_text_audio_exactly_one_second_is_allowed_with_full_speech() {
    let meter = FakeMeter::reading(1.0);
    let audio = vec![0.1f32; 16_000];
    assert_eq!(empty_text_check_allowed("", &audio, &meter), (true, 1.0));
}

#[test]
fn empty_text_empty_audio_is_refused_without_consulting_meter() {
    let meter = FakeMeter::reading(5.0);
    assert_eq!(empty_text_check_allowed("", &[], &meter), (false, 0.0));
    assert_eq!(meter.calls.get(), 0);
}

// ---------------------------------------------------------------------------
// accept_empty_lid
// ---------------------------------------------------------------------------

#[test]
fn accept_rejects_missing_language() {
    let lid = LidResult {
        lang: None,
        text: TAMIL_TEXT.to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_english_language() {
    let lid = LidResult {
        lang: Some("en".to_string()),
        text: ENGLISH_TEXT.to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_english_with_case_and_space_noise() {
    let lid = LidResult {
        lang: Some("EN ".to_string()),
        text: ENGLISH_TEXT.to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_allows_tamil_script_multi_word_text() {
    // Four words of Tamil script: well above the 6-letter and 2-word minimums.
    let lid = LidResult {
        lang: Some("ta".to_string()),
        text: TAMIL_TEXT.to_string(),
    };
    assert!(accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_tamil_language_with_latin_script_text() {
    let lid = LidResult {
        lang: Some("ta".to_string()),
        text: TAMIL_LATIN_TEXT.to_string(),
    };
    assert!(
        !accept_empty_lid(&lid),
        "Tamil label with Latin text must fail the script rule"
    );
}

#[test]
fn accept_rejects_tamil_single_word_even_with_enough_letters() {
    // One long Tamil word (6+ letters): the letter minimum passes, the word minimum does not.
    let lid = LidResult {
        lang: Some("ta".to_string()),
        text: TAMIL_ONE_WORD.to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_allows_hindi_multi_word_text() {
    let lid = LidResult {
        lang: Some("hi".to_string()),
        text: HINDI_TEXT.to_string(),
    };
    assert!(accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_japanese_stock_phrase_as_one_word() {
    let lid = LidResult {
        lang: Some("ja".to_string()),
        text: JA_HALLUCINATION.to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_allows_japanese_two_words_with_eight_letters() {
    // "ご視聴" = 3 letters, "ありがとう" = 5 letters: 8 letters, 2 words, all non-Latin.
    let lid = LidResult {
        lang: Some("ja".to_string()),
        text: "ご視聴 ありがとう".to_string(),
    };
    assert!(accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_two_words_with_only_five_letters() {
    // "नम" (2) + "तयर" (3) = 5 Devanagari letters over 2 words: below the 6-letter minimum.
    let lid = LidResult {
        lang: Some("hi".to_string()),
        text: "नम तयर".to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_allows_cyrillic_two_words() {
    // "спасибо" (7) + "вам" (3) = 10 Cyrillic letters over 2 words.
    let lid = LidResult {
        lang: Some("ru".to_string()),
        text: "спасибо вам".to_string(),
    };
    assert!(accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_french_label_with_latin_text() {
    let lid = LidResult {
        lang: Some("fr".to_string()),
        text: LATIN_FRENCH.to_string(),
    };
    assert!(
        !accept_empty_lid(&lid),
        "Latin-script text is never accepted for a non-English label"
    );
}

#[test]
fn accept_rejects_blank_text() {
    let lid = LidResult {
        lang: Some("hi".to_string()),
        text: "   ".to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

#[test]
fn accept_rejects_punctuation_only_words() {
    // Zero letters: fails the letter minimum even though there are two words.
    let lid = LidResult {
        lang: Some("hi".to_string()),
        text: "... ---".to_string(),
    };
    assert!(!accept_empty_lid(&lid));
}

// ---------------------------------------------------------------------------
// auto_refine_empty
// ---------------------------------------------------------------------------

#[test]
fn refine_silence_never_asks_identifier_or_pack() {
    let meter = FakeMeter::reading(0.5);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty("", &audio_of_secs(5.0), true, &lid, &tamil, &meter);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
    assert!(!out.admitted);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert_eq!(out.refined.text, "");
    assert_eq!(out.speech_s, 0.5);
}

#[test]
fn refine_admitted_english_identifier_keeps_active_and_is_admitted() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("en"), ENGLISH_TEXT);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), true, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert!(out.admitted);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn refine_busy_identifier_keeps_active_and_is_admitted() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::with_outcome(LidOutcome::Busy);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), false, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert!(out.admitted);
}

#[test]
fn refine_unavailable_identifier_keeps_active_and_is_admitted() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::with_outcome(LidOutcome::Unavailable("no model".to_string()));
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), false, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert!(out.admitted);
}

#[test]
fn refine_tamil_with_ready_pack_uses_pack_text_once_each() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty("", &audio_of_secs(5.0), true, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::TamilPack);
    assert_eq!(out.refined.text, TAMIL_PACK_TEXT);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 1);
    assert!(out.admitted);
}

#[test]
fn refine_tamil_pack_returning_none_falls_back_to_identifier_text() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), true, &lid, &tamil, &meter);
    assert_eq!(
        out.refined.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
    assert_eq!(out.refined.text, TAMIL_TEXT);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 1);
}

#[test]
fn refine_tamil_without_ready_pack_uses_identifier_text_without_pack_call() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty("", &audio_of_secs(5.0), false, &lid, &tamil, &meter);
    assert_eq!(
        out.refined.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
    assert_eq!(out.refined.text, TAMIL_TEXT);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn refine_tamil_over_replacement_cap_without_pack_keeps_active_but_is_admitted() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty("", &audio_of_secs(40.0), false, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert_eq!(out.refined.text, "");
    assert!(out.admitted);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn refine_tamil_over_replacement_cap_with_ready_pack_uses_pack() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty("", &audio_of_secs(40.0), true, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::TamilPack);
    assert_eq!(out.refined.text, TAMIL_PACK_TEXT);
}

#[test]
fn refine_hindi_identifier_text_under_cap_replaces_text() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("hi"), HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), false, &lid, &tamil, &meter);
    assert_eq!(
        out.refined.source,
        Source::LidText {
            lang: "hi".to_string()
        }
    );
    assert_eq!(out.refined.text, HINDI_TEXT);
}

#[test]
fn refine_hindi_identifier_text_over_replacement_cap_keeps_active() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("hi"), HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(40.0), false, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert!(out.admitted);
}

#[test]
fn refine_japanese_hallucination_is_rejected_after_one_identifier_call() {
    let meter = FakeMeter::reading(3.0);
    let lid = FakeLid::heard(Some("ja"), JA_HALLUCINATION);
    let tamil = FakeTamil::returns(None);
    let out = auto_refine_empty("", &audio_of_secs(5.0), false, &lid, &tamil, &meter);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 1);
    assert!(out.admitted);
}

#[test]
fn refine_non_blank_active_text_never_asks_identifier() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("hi"), HINDI_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let out = auto_refine_empty(
        "hello world",
        &audio_of_secs(5.0),
        true,
        &lid,
        &tamil,
        &meter,
    );
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
    assert!(!out.admitted);
    assert_eq!(out.refined.source, Source::KeepActive);
    assert_eq!(out.refined.text, "hello world");
}

#[test]
fn refine_identifier_receives_at_most_lid_max_seconds() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("hi"), HINDI_TEXT);
    let tamil = FakeTamil::returns(None);
    let _ = auto_refine_empty("", &audio_of_secs(40.0), false, &lid, &tamil, &meter);
    assert_eq!(lid.calls.get(), 1);
    assert!(
        lid.last_len.get() <= (LID_MAX_SECS * SAMPLE_RATE_HZ) as usize,
        "identifier received {} samples",
        lid.last_len.get()
    );
}

#[test]
fn refine_tamil_pack_receives_the_whole_audio() {
    let meter = FakeMeter::reading(5.0);
    let lid = FakeLid::heard(Some("ta"), TAMIL_TEXT);
    let tamil = FakeTamil::returns(Some(TAMIL_PACK_TEXT));
    let _ = auto_refine_empty("", &audio_of_secs(40.0), true, &lid, &tamil, &meter);
    assert_eq!(tamil.calls.get(), 1);
    assert_eq!(tamil.last_len.get(), 640_000, "pack must get all 40 s");
}

// ---------------------------------------------------------------------------
// format_empty_log
// ---------------------------------------------------------------------------

#[test]
fn log_line_for_admitted_tamil_lid_text_has_exact_fields() {
    let facts = AutoEmptyLogFacts {
        speech_s: 2.349,
        admitted: true,
        source: "lid_text:ta".to_string(),
    };
    assert_eq!(
        format_empty_log(&facts, 812),
        "auto language: empty_path=true admitted=true speech_s=2.3 source=lid_text:ta ms=812"
    );
}

#[test]
fn log_line_for_refused_clip_has_exact_fields() {
    let facts = AutoEmptyLogFacts {
        speech_s: 0.0,
        admitted: false,
        source: "keep".to_string(),
    };
    assert_eq!(
        format_empty_log(&facts, 0),
        "auto language: empty_path=true admitted=false speech_s=0.0 source=keep ms=0"
    );
}

#[test]
fn log_line_never_contains_recognised_text() {
    let facts = AutoEmptyLogFacts {
        speech_s: 3.0,
        admitted: true,
        source: "tamil_pack".to_string(),
    };
    let line = format_empty_log(&facts, 5);
    assert!(
        !line.contains(TAMIL_TEXT),
        "log line must not carry transcript text"
    );
    assert!(
        !line.contains(TAMIL_PACK_TEXT),
        "log line must not carry transcript text"
    );
    assert!(
        !line.contains(HINDI_TEXT),
        "log line must not carry transcript text"
    );
}

// ---------------------------------------------------------------------------
// EarshotMeter (real detector)
// ---------------------------------------------------------------------------

#[test]
fn earshot_silence_five_seconds_is_zero() {
    let audio = vec![0.0f32; 80_000];
    assert_eq!(EarshotMeter.speech_seconds(&audio), 0.0);
}

#[test]
fn earshot_empty_slice_is_zero() {
    assert_eq!(EarshotMeter.speech_seconds(&[]), 0.0);
}

#[test]
fn earshot_slice_shorter_than_one_frame_is_zero() {
    let audio = vec![0.5f32; 255];
    assert_eq!(EarshotMeter.speech_seconds(&audio), 0.0);
}

#[test]
fn earshot_nan_frames_are_zero_and_do_not_panic() {
    let audio = vec![f32::NAN; 80_000];
    assert_eq!(EarshotMeter.speech_seconds(&audio), 0.0);
}

#[test]
fn earshot_infinite_samples_are_zero_and_do_not_panic() {
    let positive = vec![f32::INFINITY; 80_000];
    let negative = vec![f32::NEG_INFINITY; 80_000];
    assert_eq!(EarshotMeter.speech_seconds(&positive), 0.0);
    assert_eq!(EarshotMeter.speech_seconds(&negative), 0.0);
}

#[test]
fn earshot_constant_dc_five_seconds_is_zero_by_design() {
    // Expected by design (DESIGN R6-1): a steady DC offset is not voice. If the real detector
    // disagrees, that is reported as a finding, not fixed by weakening this test.
    let audio = vec![0.1f32; 80_000];
    assert_eq!(EarshotMeter.speech_seconds(&audio), 0.0);
}

#[test]
fn earshot_trailing_partial_frame_is_ignored() {
    // 62 whole 256-sample frames, with and without 100 extra samples.
    let whole = vec![0.0f32; 62 * 256];
    let with_partial = vec![0.0f32; 62 * 256 + 100];
    assert_eq!(
        EarshotMeter.speech_seconds(&whole),
        EarshotMeter.speech_seconds(&with_partial)
    );
}

#[test]
fn earshot_result_is_a_multiple_of_sixteen_milliseconds() {
    for audio in [vec![0.0f32; 16_000], vec![0.1f32; 80_000]] {
        let seconds = EarshotMeter.speech_seconds(&audio);
        let frames = seconds / 0.016;
        assert!(
            (frames - frames.round()).abs() * 0.016 < 1e-4,
            "result {seconds} is not a whole number of 16 ms frames"
        );
    }
}
