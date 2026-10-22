//! TRACK S step S-B: tests for ambient wake name and context window settings.
//! Written test-first; the items below are defined by the production step in
//! `managers/ambient/engagement.rs` (`name_is_matchable`) and `settings.rs`
//! (`AMBIENT_*` constants, `validate_ambient_*`, `effective_ambient_*`).

use crate::managers::ambient::engagement::name_is_matchable;
use crate::managers::ambient::{Engagement, EngagementJudge, TranscriptSegment};
use crate::settings::{
    effective_ambient_wake_names, effective_ambient_window_ms, get_default_settings,
    validate_ambient_wake_name, validate_ambient_window_ms, AMBIENT_WAKE_NAME_MAX_CHARS,
    AMBIENT_WINDOW_MAX_MS, AMBIENT_WINDOW_MIN_MS,
};

fn seg(text: &str) -> TranscriptSegment {
    TranscriptSegment {
        segment_id: "0".to_string(),
        text: text.to_string(),
        start_time_ms: 0,
        end_time_ms: 0,
        confidence: 1.0,
        during_tts: false,
    }
}

// ---- name_is_matchable: the real ADDRESS_PATTERN decides ----

#[test]
fn matchable_accepts_single_word_tokens() {
    assert!(name_is_matchable("Vox"));
    assert!(name_is_matchable("Jarvis"));
    assert!(name_is_matchable("Vox_2"));
    assert!(name_is_matchable("vox"));
    assert!(name_is_matchable("JARVIS"));
}

#[test]
fn matchable_accepts_unicode_letters_because_pattern_uses_unicode_word_class() {
    // ADDRESS_PATTERN uses Rust regex `\w`, which is Unicode-aware by default,
    // so "Café" is captured as a single token. Asserted as the real behaviour.
    assert!(name_is_matchable("Café"));
}

#[test]
fn matchable_rejects_multi_word_punctuated_or_empty_names() {
    assert!(!name_is_matchable("My Assistant"));
    assert!(!name_is_matchable("Vox-bot"));
    assert!(!name_is_matchable("a b"));
    assert!(!name_is_matchable("Hey!"));
    assert!(!name_is_matchable(""));
    assert!(!name_is_matchable("  "));
    assert!(!name_is_matchable("Vox."));
    assert!(!name_is_matchable("vox,bot"));
}

// ---- validate_ambient_wake_name ----

#[test]
fn wake_name_accepts_valid_names_and_returns_trimmed_original_casing() {
    assert_eq!(validate_ambient_wake_name("Vox"), Ok("Vox".to_string()));
    assert_eq!(
        validate_ambient_wake_name("Jarvis"),
        Ok("Jarvis".to_string())
    );
    assert_eq!(validate_ambient_wake_name("Vox_2"), Ok("Vox_2".to_string()));
    assert_eq!(
        validate_ambient_wake_name("  Jarvis "),
        Ok("Jarvis".to_string())
    );
}

#[test]
fn wake_name_accepts_exactly_max_chars_and_rejects_one_more() {
    assert_eq!(AMBIENT_WAKE_NAME_MAX_CHARS, 32);
    let max_name = "a".repeat(32);
    assert_eq!(validate_ambient_wake_name(&max_name), Ok(max_name.clone()));
    let too_long = "a".repeat(33);
    assert!(validate_ambient_wake_name(&too_long).is_err());
}

#[test]
fn wake_name_counts_chars_not_bytes() {
    // 32 chars, 64 bytes: must be accepted.
    let multibyte = "é".repeat(32);
    assert_eq!(multibyte.chars().count(), 32);
    assert!(multibyte.len() > 32);
    assert_eq!(
        validate_ambient_wake_name(&multibyte),
        Ok(multibyte.clone())
    );
    // 33 chars: must be rejected.
    let multibyte_too_long = "é".repeat(33);
    assert!(validate_ambient_wake_name(&multibyte_too_long).is_err());
}

#[test]
fn wake_name_rejects_multi_word_punctuated_and_blank_names() {
    assert!(validate_ambient_wake_name("My Assistant").is_err());
    assert!(validate_ambient_wake_name("Vox-bot").is_err());
    assert!(validate_ambient_wake_name("").is_err());
    assert!(validate_ambient_wake_name("   ").is_err());
}

#[test]
fn wake_name_rejects_control_characters() {
    assert!(validate_ambient_wake_name("bad\u{7}name").is_err());
    assert!(validate_ambient_wake_name("tab\tname").is_err());
}

#[test]
fn wake_name_errors_are_human_readable_and_non_empty() {
    for bad in ["My Assistant", "", "   ", "bad\u{7}name", "Vox-bot"] {
        let err = validate_ambient_wake_name(bad).expect_err("should be rejected");
        assert!(!err.trim().is_empty(), "empty error for {bad:?}");
    }
}

// ---- validate_ambient_window_ms ----

#[test]
fn window_bounds_constants_match_contract() {
    assert_eq!(AMBIENT_WINDOW_MIN_MS, 10_000);
    assert_eq!(AMBIENT_WINDOW_MAX_MS, 600_000);
}

#[test]
fn window_accepts_inclusive_bounds_and_default() {
    assert_eq!(validate_ambient_window_ms(10_000), Ok(10_000));
    assert_eq!(validate_ambient_window_ms(90_000), Ok(90_000));
    assert_eq!(validate_ambient_window_ms(600_000), Ok(600_000));
}

#[test]
fn window_rejects_out_of_range_values() {
    assert!(validate_ambient_window_ms(9_999).is_err());
    assert!(validate_ambient_window_ms(0).is_err());
    assert!(validate_ambient_window_ms(600_001).is_err());
    assert!(validate_ambient_window_ms(u64::MAX).is_err());
}

#[test]
fn window_error_message_is_non_empty() {
    let err = validate_ambient_window_ms(0).expect_err("should be rejected");
    assert!(!err.trim().is_empty());
}

// ---- defaults ----

#[test]
fn defaults_validate_under_the_new_rules() {
    let settings = get_default_settings();
    assert_eq!(settings.ambient_wake_name, "Vox");
    assert_eq!(settings.ambient_window_ms, 90_000);
    assert_eq!(
        validate_ambient_wake_name(&settings.ambient_wake_name),
        Ok("Vox".to_string())
    );
    assert_eq!(
        validate_ambient_window_ms(settings.ambient_window_ms),
        Ok(90_000)
    );
}

// ---- effective_* startup helpers ----

#[test]
fn effective_values_for_default_settings() {
    let settings = get_default_settings();
    assert_eq!(
        effective_ambient_wake_names(&settings),
        vec!["vox".to_string()]
    );
    assert_eq!(effective_ambient_window_ms(&settings), 90_000);
}

#[test]
fn effective_wake_names_use_valid_stored_name_lowercased() {
    let mut settings = get_default_settings();
    settings.ambient_wake_name = "Jarvis".to_string();
    assert_eq!(
        effective_ambient_wake_names(&settings),
        vec!["jarvis".to_string()]
    );
    settings.ambient_wake_name = "JARVIS".to_string();
    assert_eq!(
        effective_ambient_wake_names(&settings),
        vec!["jarvis".to_string()]
    );
    settings.ambient_wake_name = "Vox_2".to_string();
    assert_eq!(
        effective_ambient_wake_names(&settings),
        vec!["vox_2".to_string()]
    );
}

#[test]
fn effective_wake_names_fall_back_to_default_for_invalid_stored_name() {
    let mut settings = get_default_settings();
    for bad in ["My Assistant", "Vox-bot", "", "   ", "bad\u{7}name"] {
        settings.ambient_wake_name = bad.to_string();
        assert_eq!(
            effective_ambient_wake_names(&settings),
            vec!["vox".to_string()],
            "invalid stored name {bad:?} must fall back to default"
        );
    }
}

#[test]
fn effective_window_uses_valid_stored_value() {
    let mut settings = get_default_settings();
    settings.ambient_window_ms = 120_000;
    assert_eq!(effective_ambient_window_ms(&settings), 120_000);
    settings.ambient_window_ms = 10_000;
    assert_eq!(effective_ambient_window_ms(&settings), 10_000);
    settings.ambient_window_ms = 600_000;
    assert_eq!(effective_ambient_window_ms(&settings), 600_000);
}

#[test]
fn effective_window_falls_back_to_default_for_invalid_stored_value() {
    let mut settings = get_default_settings();
    for bad in [0_u64, 9_999, 600_001, u64::MAX] {
        settings.ambient_window_ms = bad;
        assert_eq!(
            effective_ambient_window_ms(&settings),
            90_000,
            "invalid stored window {bad} must fall back to default"
        );
    }
}

// ---- end-to-end startup wiring: the judge built from the settings ----

#[test]
fn startup_wiring_judge_from_configured_name_recognises_that_name() {
    let mut settings = get_default_settings();
    settings.ambient_wake_name = "Jarvis".to_string();
    let judge = EngagementJudge::new(effective_ambient_wake_names(&settings));

    assert_eq!(
        judge.evaluate(&[seg("Hey Jarvis, tell me the time")]),
        Engagement::Addressed {
            extracted_request: "tell me the time".to_string()
        }
    );
    assert_eq!(
        judge.evaluate(&[seg("Hey Vox, tell me the time")]),
        Engagement::NotAddressed
    );
}

#[test]
fn startup_wiring_default_settings_judge_still_recognises_vox() {
    let settings = get_default_settings();
    let judge = EngagementJudge::new(effective_ambient_wake_names(&settings));
    assert_eq!(
        judge.evaluate(&[seg("Hey Vox, tell me the time")]),
        Engagement::Addressed {
            extracted_request: "tell me the time".to_string()
        }
    );
}

// ---- Unicode case folding: validation and matching agree ----

#[test]
fn accented_name_judge_recognises_uppercase_and_lowercase_spellings() {
    let mut settings = get_default_settings();
    settings.ambient_wake_name = "Café".to_string();
    let judge = EngagementJudge::new(effective_ambient_wake_names(&settings));

    assert_eq!(
        judge.evaluate(&[seg("Hey CAFÉ, tell me the time")]),
        Engagement::Addressed {
            extracted_request: "tell me the time".to_string()
        }
    );
    assert_eq!(
        judge.evaluate(&[seg("Hey café, tell me the time")]),
        Engagement::Addressed {
            extracted_request: "tell me the time".to_string()
        }
    );
}

#[test]
fn uppercase_accented_name_is_matchable_and_matched_by_lowercase_speech() {
    assert!(name_is_matchable("CAFÉ"));
    let judge = EngagementJudge::new(vec!["CAFÉ".to_lowercase()]);
    assert_eq!(
        judge.evaluate(&[seg("Hey café, tell me the time")]),
        Engagement::Addressed {
            extracted_request: "tell me the time".to_string()
        }
    );
}
