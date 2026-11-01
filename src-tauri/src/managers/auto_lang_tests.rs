//! Tests for auto_lang (written first; see DESIGN.md Revision 3).
//!
//! Against the stub every case fails with `not yet implemented: auto_lang::...`.

use super::auto_lang::*;
use std::cell::Cell;

// ---------------------------------------------------------------------------
// Shared fixtures and helpers
// ---------------------------------------------------------------------------

const TAMIL_TEXT: &str = "வணக்கம் நண்பரே";
const DEVANAGARI_TEXT: &str = "नमस्ते दोस्त";
const LATIN_TEXT: &str = "Hello there my friend";
const FLAGGED_ACTIVE: &str = "Vanakam, Indravanila Yepadi Iukirade.";
const FLAGGED_TOKENS: usize = 21;

const THREE_SECS: usize = 48_000;
const POINT_FOUR_SECS: usize = 6_400;
const SECS_29_0: usize = 464_000;
const SECS_29_01: usize = 464_160;
const SECS_40: usize = 640_000;
const LID_CAP_SAMPLES: usize = 480_000;

const TOLERANCE: f32 = 1e-4;

struct FakeLid {
    outcome: LidOutcome,
    calls: Cell<usize>,
    last_len: Cell<usize>,
}

impl LidIdentify for FakeLid {
    fn identify(&self, samples: &[f32]) -> LidOutcome {
        self.calls.set(self.calls.get() + 1);
        self.last_len.set(samples.len());
        self.outcome.clone()
    }
}

struct FakeTamil {
    reply: Option<String>,
    calls: Cell<usize>,
    last_len: Cell<usize>,
}

impl TamilPackRoute for FakeTamil {
    fn transcribe(&self, samples: &[f32]) -> Option<String> {
        self.calls.set(self.calls.get() + 1);
        self.last_len.set(samples.len());
        self.reply.clone()
    }
}

fn fake_lid(outcome: LidOutcome) -> FakeLid {
    FakeLid {
        outcome,
        calls: Cell::new(0),
        last_len: Cell::new(0),
    }
}

fn fake_tamil(reply: Option<&str>) -> FakeTamil {
    FakeTamil {
        reply: reply.map(str::to_string),
        calls: Cell::new(0),
        last_len: Cell::new(0),
    }
}

fn done(lang: Option<&str>, text: &str) -> LidOutcome {
    LidOutcome::Done(lid_result(lang, text))
}

fn lid_result(lang: Option<&str>, text: &str) -> LidResult {
    LidResult {
        lang: lang.map(str::to_string),
        text: text.to_string(),
    }
}

fn silence(samples: usize) -> Vec<f32> {
    vec![0.0f32; samples]
}

fn run_refine(
    active: &str,
    tokens: usize,
    audio: &[f32],
    pack_ready: bool,
    lid: &FakeLid,
    tamil: &FakeTamil,
) -> AutoRefined {
    let input = AutoInput {
        active_text: active,
        token_count: tokens,
        audio,
        tamil_pack_ready: pack_ready,
    };
    auto_refine(&input, lid, tamil)
}

fn approx_eq(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() < TOLERANCE
}

fn cand(id: &str, bytes: u64, multilingual_whisper: bool, is_active: bool) -> LidCandidate {
    LidCandidate {
        id: id.to_string(),
        bytes,
        multilingual_whisper,
        is_active,
    }
}

// Independent reference implementation of the letter rules, used by the matrix test.

fn ref_is_tamil_letter(c: char) -> bool {
    ('\u{0B80}'..='\u{0BFF}').contains(&c) && !('\u{0BE6}'..='\u{0BEF}').contains(&c)
}

fn ref_is_letter(c: char) -> bool {
    c.is_alphabetic() || ref_is_tamil_letter(c)
}

fn ref_is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || (('\u{00C0}'..='\u{024F}').contains(&c) && c != '\u{00D7}' && c != '\u{00F7}')
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

fn ref_letters(text: &str) -> usize {
    text.chars().filter(|&c| ref_is_letter(c)).count()
}

fn ref_tamil_fraction(text: &str) -> f32 {
    let letters = ref_letters(text);
    if letters == 0 {
        return 0.0;
    }
    text.chars().filter(|&c| ref_is_tamil_letter(c)).count() as f32 / letters as f32
}

fn ref_non_latin_fraction(text: &str) -> f32 {
    let letters = ref_letters(text);
    if letters == 0 {
        return 0.0;
    }
    text.chars()
        .filter(|&c| ref_is_letter(c) && !ref_is_latin_letter(c))
        .count() as f32
        / letters as f32
}

/// Reference route for 3.0 s audio (the matrix test uses only that length).
fn reference_route(lang: Option<&str>, pack_ready: bool, text: &str) -> AutoRoute {
    let lang = match lang {
        None => return AutoRoute::KeepActive,
        Some(code) => code.to_ascii_lowercase(),
    };
    if lang.is_empty() || lang == "en" {
        return AutoRoute::KeepActive;
    }
    if ref_letters(text) < 3 {
        return AutoRoute::KeepActive;
    }
    if lang == "ta" {
        let tamil = ref_tamil_fraction(text);
        if pack_ready && tamil >= 0.5 {
            return AutoRoute::UseTamilPack;
        }
        if tamil >= 0.5 {
            return AutoRoute::UseLidText;
        }
        return AutoRoute::KeepActive;
    }
    if ref_non_latin_fraction(text) >= 0.5 {
        AutoRoute::UseLidText
    } else {
        AutoRoute::KeepActive
    }
}

// ---------------------------------------------------------------------------
// 1. tokens_per_word
// ---------------------------------------------------------------------------

#[test]
fn tokens_per_word_divides_tokens_by_words_for_twenty_four_tokens_six_words() {
    assert_eq!(
        tokens_per_word("Nalai Kalai Unbadu Manika Kutam Mirakir", 24),
        Some(4.0)
    );
}

#[test]
fn tokens_per_word_divides_tokens_by_words_for_twelve_tokens_six_words() {
    assert_eq!(
        tokens_per_word("What is the weather like today?", 12),
        Some(2.0)
    );
}

#[test]
fn tokens_per_word_is_none_for_empty_text() {
    assert_eq!(tokens_per_word("", 0), None);
}

#[test]
fn tokens_per_word_is_none_for_whitespace_only_text() {
    assert_eq!(tokens_per_word("   \t\n ", 5), None);
}

// ---------------------------------------------------------------------------
// 2. needs_language_check
// ---------------------------------------------------------------------------

#[test]
fn needs_check_is_true_for_romanised_tamil_with_high_tokens_per_word() {
    assert!(needs_language_check(
        "Nalai Kalai Unbadu Manika Kutam Mirakir",
        24,
        3.0
    ));
}

#[test]
fn needs_check_is_true_for_four_word_romanised_tamil_sentence() {
    assert!(needs_language_check(FLAGGED_ACTIVE, FLAGGED_TOKENS, 3.0));
}

#[test]
fn needs_check_is_true_for_three_word_text_at_four_point_six_seven_tokens_per_word() {
    assert!(needs_language_check("Yenaku Urukaptenir Vendum.", 14, 3.0));
}

#[test]
fn needs_check_is_true_for_namaste_sentence_at_four_point_three_three_tokens_per_word() {
    assert!(needs_language_check("Namaste, Ajmosam Kesah", 13, 3.0));
}

#[test]
fn needs_check_is_false_for_normal_english_sentence_at_three_tokens_per_word() {
    assert!(!needs_language_check(
        "Please summarize my last meeting.",
        15,
        3.0
    ));
}

#[test]
fn needs_check_is_false_for_normal_english_question() {
    assert!(!needs_language_check(
        "What is the weather like today?",
        12,
        3.0
    ));
}

#[test]
fn needs_check_is_false_for_normal_english_command() {
    assert!(!needs_language_check(
        "Set a reminder for 9 in the morning.",
        14,
        3.0
    ));
}

#[test]
fn needs_check_is_false_for_single_word_even_with_many_tokens() {
    assert!(!needs_language_check("Yes.", 2, 3.0));
}

#[test]
fn needs_check_is_false_for_two_words_at_two_point_five_tokens_per_word() {
    assert!(!needs_language_check("When dump.", 5, 3.0));
}

#[test]
fn needs_check_is_false_for_empty_text_with_zero_tokens() {
    assert!(!needs_language_check("", 0, 1.2));
}

#[test]
fn needs_check_is_false_for_whitespace_only_text_even_with_tokens() {
    assert!(!needs_language_check("   ", 4, 2.0));
}

#[test]
fn needs_check_is_false_when_tokens_per_word_is_exactly_three_point_five() {
    assert_eq!(tokens_per_word("two words", 7), Some(3.5));
    assert!(!needs_language_check("two words", 7, 3.0));
}

#[test]
fn needs_check_is_true_when_tokens_per_word_is_just_above_three_point_five() {
    // 20 words, 71 tokens: 3.55 tokens per word.
    let text = "Vanakam Indravanila Yepadi Iukirade Nalai Kalai Unbadu Manika Kutam Mirakir \
                Yenaku Urukaptenir Vendum Namaste Ajmosam Kesah Oru Naal Nanri Vaa";
    assert_eq!(text.split_whitespace().count(), 20);
    assert!(needs_language_check(text, 71, 3.0));
}

#[test]
fn needs_check_is_false_for_high_tokens_per_word_when_audio_is_below_half_second() {
    assert!(!needs_language_check(FLAGGED_ACTIVE, FLAGGED_TOKENS, 0.4));
}

#[test]
fn needs_check_is_true_for_high_tokens_per_word_when_audio_is_exactly_half_second() {
    assert!(needs_language_check(FLAGGED_ACTIVE, FLAGGED_TOKENS, 0.5));
}

// ---------------------------------------------------------------------------
// 3. Letter fractions and letter_count
// ---------------------------------------------------------------------------

#[test]
fn tamil_fraction_is_one_for_pure_tamil_word() {
    assert_eq!(tamil_letter_fraction("ஆமாம்"), 1.0);
}

#[test]
fn tamil_fraction_is_zero_for_latin_word() {
    assert_eq!(tamil_letter_fraction("hello"), 0.0);
}

#[test]
fn tamil_fraction_for_mixed_tamil_and_latin_is_strictly_between_thirty_and_seventy_percent() {
    let fraction = tamil_letter_fraction("வணக்கம் hello");
    assert!(fraction > 0.3 && fraction < 0.7, "got {fraction}");
}

#[test]
fn tamil_fraction_for_mixed_text_equals_tamil_letters_over_all_letters() {
    let tamil_letters = "வணக்கம்".chars().count() as f32;
    let expected = tamil_letters / (tamil_letters + 5.0);
    assert!(approx_eq(tamil_letter_fraction("வணக்கம் hello"), expected));
}

#[test]
fn tamil_fraction_is_zero_for_digits_only() {
    assert_eq!(tamil_letter_fraction("12345"), 0.0);
}

#[test]
fn tamil_fraction_is_zero_for_empty_text() {
    assert_eq!(tamil_letter_fraction(""), 0.0);
}

#[test]
fn tamil_fraction_is_zero_for_whitespace_only_text() {
    assert_eq!(tamil_letter_fraction("   \t "), 0.0);
}

#[test]
fn tamil_fraction_ignores_ascii_digits_and_punctuation() {
    assert_eq!(tamil_letter_fraction("வணக்கம் 123!"), 1.0);
}

#[test]
fn non_latin_fraction_is_one_for_devanagari_word() {
    let fraction = non_latin_letter_fraction("नमस्ते");
    assert!(fraction >= 0.99, "got {fraction}");
}

#[test]
fn non_latin_fraction_is_zero_for_accented_latin_word() {
    assert_eq!(non_latin_letter_fraction("café"), 0.0);
}

#[test]
fn non_latin_fraction_is_zero_for_plain_ascii_word() {
    assert_eq!(non_latin_letter_fraction("hello"), 0.0);
}

#[test]
fn non_latin_fraction_for_mixed_latin_and_devanagari_is_strictly_between_zero_and_one() {
    let fraction = non_latin_letter_fraction("hello नमस्ते");
    assert!(fraction > 0.0 && fraction < 1.0, "got {fraction}");
}

#[test]
fn non_latin_fraction_counts_tamil_as_non_latin() {
    assert_eq!(non_latin_letter_fraction("வணக்கம்"), 1.0);
}

#[test]
fn non_latin_fraction_is_zero_for_empty_text() {
    assert_eq!(non_latin_letter_fraction(""), 0.0);
}

#[test]
fn letter_count_ignores_digits_and_punctuation() {
    assert_eq!(letter_count("a1 b!"), 2);
}

#[test]
fn letter_count_counts_every_tamil_letter_in_a_tamil_word() {
    assert_eq!(letter_count("வணக்கம்"), "வணக்கம்".chars().count());
}

#[test]
fn letter_count_excludes_tamil_digits() {
    assert_eq!(letter_count("௧௨௩"), 0);
}

// ---------------------------------------------------------------------------
// 4. choose_route
// ---------------------------------------------------------------------------

#[test]
fn choose_route_keeps_active_when_lid_reports_no_language() {
    let lid = lid_result(None, TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_when_lid_language_is_empty_string() {
    let lid = lid_result(Some(""), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_when_lid_says_english() {
    let lid = lid_result(Some("en"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_when_lid_says_english_in_uppercase() {
    let lid = lid_result(Some("EN"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_uses_tamil_pack_when_ready_and_tamil_text() {
    let lid = lid_result(Some("ta"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::UseTamilPack);
}

#[test]
fn choose_route_uses_tamil_pack_for_uppercase_tamil_code() {
    let lid = lid_result(Some("TA"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::UseTamilPack);
}

#[test]
fn choose_route_uses_tamil_pack_for_forty_second_clip_when_ready() {
    let lid = lid_result(Some("ta"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, true, 40.0), AutoRoute::UseTamilPack);
}

#[test]
fn choose_route_uses_lid_text_for_tamil_when_pack_not_ready() {
    let lid = lid_result(Some("ta"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, false, 3.0), AutoRoute::UseLidText);
}

#[test]
fn choose_route_uses_lid_text_for_tamil_at_exactly_29_seconds_when_pack_not_ready() {
    let lid = lid_result(Some("ta"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, false, 29.0), AutoRoute::UseLidText);
}

#[test]
fn choose_route_keeps_active_for_tamil_over_29_seconds_when_pack_not_ready() {
    let lid = lid_result(Some("ta"), TAMIL_TEXT);
    assert_eq!(choose_route(&lid, false, 40.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_for_tamil_lid_with_latin_text() {
    let lid = lid_result(Some("ta"), LATIN_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
    assert_eq!(choose_route(&lid, false, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_for_tamil_lid_with_two_letter_tamil_text() {
    let text = "ஆம";
    assert_eq!(letter_count(text), 2);
    let lid = lid_result(Some("ta"), text);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
    assert_eq!(choose_route(&lid, false, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_uses_lid_text_for_hindi_devanagari_text() {
    let lid = lid_result(Some("hi"), DEVANAGARI_TEXT);
    assert_eq!(choose_route(&lid, false, 3.0), AutoRoute::UseLidText);
}

#[test]
fn choose_route_uses_lid_text_for_hindi_at_exactly_29_seconds() {
    let lid = lid_result(Some("hi"), DEVANAGARI_TEXT);
    assert_eq!(choose_route(&lid, false, 29.0), AutoRoute::UseLidText);
}

#[test]
fn choose_route_keeps_active_for_hindi_devanagari_text_at_29_point_01_seconds() {
    let lid = lid_result(Some("hi"), DEVANAGARI_TEXT);
    assert_eq!(choose_route(&lid, false, 29.01), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_for_hindi_with_latin_text() {
    let lid = lid_result(Some("hi"), LATIN_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_for_spanish_with_latin_text() {
    let lid = lid_result(Some("es"), LATIN_TEXT);
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_keeps_active_for_non_english_lid_with_empty_text() {
    let lid = lid_result(Some("hi"), "");
    assert_eq!(choose_route(&lid, true, 3.0), AutoRoute::KeepActive);
}

#[test]
fn choose_route_matrix_matches_reference_for_every_language_pack_and_text() {
    let languages: [Option<&str>; 6] = [
        None,
        Some("en"),
        Some("ta"),
        Some("hi"),
        Some("te"),
        Some("es"),
    ];
    let texts = [TAMIL_TEXT, DEVANAGARI_TEXT, LATIN_TEXT, ""];
    for lang in languages {
        for pack_ready in [true, false] {
            for text in texts {
                let lid = lid_result(lang, text);
                let actual = choose_route(&lid, pack_ready, 3.0);
                let expected = reference_route(lang, pack_ready, text);
                assert_eq!(
                    actual, expected,
                    "lang={lang:?} pack_ready={pack_ready} text={text:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 5. pick_lid_model
// ---------------------------------------------------------------------------

#[test]
fn pick_lid_model_chooses_smallest_eligible_multilingual_whisper() {
    let cands = [
        cand("whisper-small", 500, true, false),
        cand("whisper-tiny", 200, true, false),
        cand("whisper-base", 300, true, false),
    ];
    assert_eq!(pick_lid_model(&cands), Some("whisper-tiny".to_string()));
}

#[test]
fn pick_lid_model_breaks_size_ties_by_smaller_id() {
    let cands = [
        cand("whisper-b", 200, true, false),
        cand("whisper-a", 200, true, false),
    ];
    assert_eq!(pick_lid_model(&cands), Some("whisper-a".to_string()));
}

#[test]
fn pick_lid_model_ignores_english_only_models_even_when_smallest() {
    let cands = [
        cand("tiny.en", 10, false, false),
        cand("whisper-base", 300, true, false),
    ];
    assert_eq!(pick_lid_model(&cands), Some("whisper-base".to_string()));
}

#[test]
fn pick_lid_model_skips_the_active_model_even_when_smallest() {
    let cands = [
        cand("whisper-tiny", 100, true, true),
        cand("whisper-base", 300, true, false),
    ];
    assert_eq!(pick_lid_model(&cands), Some("whisper-base".to_string()));
}

#[test]
fn pick_lid_model_is_none_for_empty_slice() {
    assert_eq!(pick_lid_model(&[]), None);
}

#[test]
fn pick_lid_model_is_none_when_only_english_only_models_exist() {
    let cands = [
        cand("tiny.en", 10, false, false),
        cand("base.en", 20, false, false),
    ];
    assert_eq!(pick_lid_model(&cands), None);
}

#[test]
fn pick_lid_model_is_none_when_only_candidate_is_active() {
    let cands = [cand("whisper-base", 300, true, true)];
    assert_eq!(pick_lid_model(&cands), None);
}

#[test]
fn pick_lid_model_is_none_when_only_active_and_non_whisper_remain() {
    let cands = [
        cand("whisper-base", 300, true, true),
        cand("parakeet-unified-en-0.6b", 50, false, false),
    ];
    assert_eq!(pick_lid_model(&cands), None);
}

// ---------------------------------------------------------------------------
// 6. auto_route_active
// ---------------------------------------------------------------------------

#[test]
fn auto_route_active_is_true_for_the_full_auto_configuration() {
    assert!(auto_route_active("auto", true, false, 3.0, true));
}

#[test]
fn auto_route_active_is_false_when_language_is_tamil() {
    assert!(!auto_route_active("ta", true, false, 3.0, true));
}

#[test]
fn auto_route_active_is_false_when_language_is_english() {
    assert!(!auto_route_active("en", true, false, 3.0, true));
}

#[test]
fn auto_route_active_is_false_when_language_is_empty() {
    assert!(!auto_route_active("", true, false, 3.0, true));
}

#[test]
fn auto_route_active_is_false_when_setting_is_disabled() {
    assert!(!auto_route_active("auto", false, false, 3.0, true));
}

#[test]
fn auto_route_active_is_false_when_translate_to_english_is_on() {
    assert!(!auto_route_active("auto", true, true, 3.0, true));
}

#[test]
fn auto_route_active_is_false_for_audio_below_half_second() {
    assert!(!auto_route_active("auto", true, false, 0.49, true));
}

#[test]
fn auto_route_active_is_true_for_audio_exactly_half_second() {
    assert!(auto_route_active("auto", true, false, 0.5, true));
}

#[test]
fn auto_route_active_is_false_when_engine_has_no_token_rows() {
    assert!(!auto_route_active("auto", true, false, 3.0, false));
}

// ---------------------------------------------------------------------------
// 7. should_upgrade_to_tamil_pack
// ---------------------------------------------------------------------------

#[test]
fn upgrade_is_true_for_model_detected_tamil_in_full_auto_configuration() {
    assert!(should_upgrade_to_tamil_pack(
        Some("ta"),
        true,
        true,
        "auto",
        false
    ));
}

#[test]
fn upgrade_is_true_for_uppercase_tamil_code() {
    assert!(should_upgrade_to_tamil_pack(
        Some("TA"),
        true,
        true,
        "auto",
        false
    ));
}

#[test]
fn upgrade_is_false_when_tamil_pack_is_not_ready() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("ta"),
        false,
        true,
        "auto",
        false
    ));
}

#[test]
fn upgrade_is_false_when_setting_is_disabled() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("ta"),
        true,
        false,
        "auto",
        false
    ));
}

#[test]
fn upgrade_is_false_when_selected_language_is_tamil() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("ta"),
        true,
        true,
        "ta",
        false
    ));
}

#[test]
fn upgrade_is_false_when_selected_language_is_english() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("ta"),
        true,
        true,
        "en",
        false
    ));
}

#[test]
fn upgrade_is_false_when_translate_to_english_is_on() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("ta"),
        true,
        true,
        "auto",
        true
    ));
}

#[test]
fn upgrade_is_false_when_model_detected_hindi() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("hi"),
        true,
        true,
        "auto",
        false
    ));
}

#[test]
fn upgrade_is_false_when_model_detected_nothing() {
    assert!(!should_upgrade_to_tamil_pack(
        None, true, true, "auto", false
    ));
}

#[test]
fn upgrade_is_false_when_model_detected_english() {
    assert!(!should_upgrade_to_tamil_pack(
        Some("en"),
        true,
        true,
        "auto",
        false
    ));
}

// ---------------------------------------------------------------------------
// 8. auto_refine
// ---------------------------------------------------------------------------

#[test]
fn auto_refine_keeps_clean_active_text_unchanged_without_any_model_calls() {
    let active = "Please summarize my last meeting. ";
    let lid = fake_lid(done(Some("hi"), DEVANAGARI_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(active, 15, &audio, true, &lid, &tamil);
    assert_eq!(result.text, active);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_keeps_active_when_flagged_and_lid_says_english() {
    let lid = fake_lid(done(Some("en"), "English words"));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_uses_tamil_pack_text_when_lid_is_tamil_and_pack_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some("  வணக்கம் நண்பரே  "));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, "வணக்கம் நண்பரே");
    assert_eq!(result.source, Source::TamilPack);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 1);
}

#[test]
fn auto_refine_passes_whole_audio_to_tamil_pack_and_capped_audio_to_lid() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(SECS_40);
    let _ = run_refine(FLAGGED_ACTIVE, 60, &audio, true, &lid, &tamil);
    assert_eq!(lid.last_len.get(), LID_CAP_SAMPLES);
    assert_eq!(tamil.last_len.get(), SECS_40);
}

#[test]
fn auto_refine_falls_back_to_lid_tamil_text_when_tamil_pack_returns_none() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, TAMIL_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
    assert_eq!(tamil.calls.get(), 1);
}

#[test]
fn auto_refine_uses_lid_tamil_text_without_calling_tamil_pack_when_pack_not_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, false, &lid, &tamil);
    assert_eq!(result.text, TAMIL_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_keeps_active_when_lid_is_tamil_but_text_is_latin() {
    let lid = fake_lid(done(Some("ta"), LATIN_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_uses_lid_hindi_text_for_devanagari_whisper_output() {
    let lid = fake_lid(done(Some("hi"), DEVANAGARI_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, DEVANAGARI_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "hi".to_string()
        }
    );
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_keeps_active_when_lid_is_hindi_but_text_is_latin() {
    let lid = fake_lid(done(Some("hi"), LATIN_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
}

#[test]
fn auto_refine_keeps_active_when_lid_returns_empty_text_for_hindi() {
    let lid = fake_lid(done(Some("hi"), ""));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
}

#[test]
fn auto_refine_keeps_active_when_identifier_is_busy() {
    let lid = fake_lid(LidOutcome::Busy);
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 1);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_keeps_active_when_identifier_is_unavailable() {
    let lid = fake_lid(LidOutcome::Unavailable("x".to_string()));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_never_calls_identifier_for_empty_active_text() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine("", 0, &audio, true, &lid, &tamil);
    assert_eq!(result.text, "");
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_never_calls_identifier_for_whitespace_only_active_text() {
    let lid = fake_lid(done(Some("hi"), DEVANAGARI_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(THREE_SECS);
    let result = run_refine("   ", 4, &audio, true, &lid, &tamil);
    assert_eq!(result.text, "   ");
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_keeps_correct_english_active_text_when_lid_wrongly_says_hindi_with_latin_text() {
    let active = "Kubernetes Anjaneyulu Venkataraghavan deployed";
    let lid = fake_lid(done(
        Some("hi"),
        "Kubernetes Anjaneyulu Venkataraghavan deployed",
    ));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(active, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, active);
    assert_eq!(result.source, Source::KeepActive);
}

#[test]
fn auto_refine_documents_the_residual_risk_when_whisper_writes_non_latin_for_correct_english() {
    let active = "Kubernetes Anjaneyulu Venkataraghavan deployed";
    let lid = fake_lid(done(Some("hi"), DEVANAGARI_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(THREE_SECS);
    let result = run_refine(active, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, DEVANAGARI_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "hi".to_string()
        }
    );
}

#[test]
fn auto_refine_keeps_active_for_forty_second_clip_with_tamil_lid_when_pack_not_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(SECS_40);
    let active = "Vanakam, Indravanila Yepadi Iukirade. (rest of the forty second clip)";
    let result = run_refine(active, 60, &audio, false, &lid, &tamil);
    assert_eq!(result.text, active);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.last_len.get(), LID_CAP_SAMPLES);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_uses_tamil_pack_text_for_forty_second_clip_when_pack_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some("முழு ஒலிப்பதிவு"));
    let audio = silence(SECS_40);
    let result = run_refine(FLAGGED_ACTIVE, 60, &audio, true, &lid, &tamil);
    assert_eq!(result.text, "முழு ஒலிப்பதிவு");
    assert_eq!(result.source, Source::TamilPack);
    assert_eq!(tamil.last_len.get(), SECS_40);
}

#[test]
fn auto_refine_keeps_active_for_forty_second_clip_when_pack_returns_none_even_with_tamil_lid_text()
{
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(SECS_40);
    let result = run_refine(FLAGGED_ACTIVE, 60, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(tamil.calls.get(), 1);
}

#[test]
fn auto_refine_uses_lid_text_at_exactly_29_seconds_when_pack_not_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(SECS_29_0);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, false, &lid, &tamil);
    assert_eq!(result.text, TAMIL_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
}

#[test]
fn auto_refine_keeps_active_at_29_point_01_seconds_when_pack_not_ready() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(None);
    let audio = silence(SECS_29_01);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, false, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
}

#[test]
fn auto_refine_makes_zero_calls_for_flagged_text_in_audio_under_half_second() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some(TAMIL_TEXT));
    let audio = silence(POINT_FOUR_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, FLAGGED_ACTIVE);
    assert_eq!(result.source, Source::KeepActive);
    assert_eq!(lid.calls.get(), 0);
    assert_eq!(tamil.calls.get(), 0);
}

#[test]
fn auto_refine_falls_through_to_lid_text_when_tamil_pack_returns_whitespace_only() {
    let lid = fake_lid(done(Some("ta"), TAMIL_TEXT));
    let tamil = fake_tamil(Some("   \n\t  "));
    let audio = silence(THREE_SECS);
    let result = run_refine(FLAGGED_ACTIVE, FLAGGED_TOKENS, &audio, true, &lid, &tamil);
    assert_eq!(result.text, TAMIL_TEXT);
    assert_eq!(
        result.source,
        Source::LidText {
            lang: "ta".to_string()
        }
    );
    assert_eq!(tamil.calls.get(), 1);
}
