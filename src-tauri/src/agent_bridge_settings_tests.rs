//! Tests for Track S step S-C: custom filler word validation, agent bridge
//! timeout validation, and agent bridge prompt flag validation.
//!
//! These tests encode the contract for the production functions in
//! `settings.rs` and are expected to fail to compile until they exist.

use crate::audio_toolkit::{remove_filler_words, OutputLanguageEvidence};
use crate::settings::*;

fn settings_with(agent_flag: &str, prompt_flag: &str) -> AppSettings {
    let mut s = get_default_settings();
    s.agent_jobs_agent_flag = agent_flag.to_string();
    s.agent_bridge_prompt_flag = prompt_flag.to_string();
    s
}

// ---------------------------------------------------------------------------
// A) agent bridge timeout
// ---------------------------------------------------------------------------

#[test]
fn timeout_constants_match_contract() {
    assert_eq!(AGENT_BRIDGE_TIMEOUT_MIN_SECS, 5);
    assert_eq!(AGENT_BRIDGE_TIMEOUT_MAX_SECS, 600);
}

#[test]
fn timeout_boundaries_and_typical_values_are_accepted() {
    assert_eq!(validate_agent_bridge_timeout_secs(5), Ok(5));
    assert_eq!(validate_agent_bridge_timeout_secs(60), Ok(60));
    assert_eq!(validate_agent_bridge_timeout_secs(600), Ok(600));
}

#[test]
fn timeout_out_of_range_is_rejected() {
    assert!(validate_agent_bridge_timeout_secs(4).is_err());
    assert!(validate_agent_bridge_timeout_secs(0).is_err());
    assert!(validate_agent_bridge_timeout_secs(601).is_err());
    assert!(validate_agent_bridge_timeout_secs(u64::MAX).is_err());
}

#[test]
fn timeout_rejection_has_a_message() {
    let err = validate_agent_bridge_timeout_secs(0).unwrap_err();
    assert!(!err.trim().is_empty());
}

#[test]
fn default_timeout_validates() {
    let s = get_default_settings();
    assert!(validate_agent_bridge_timeout_secs(s.agent_bridge_timeout_secs).is_ok());
}

// ---------------------------------------------------------------------------
// B) custom filler words: validation
// ---------------------------------------------------------------------------

fn words(list: &[&str]) -> Option<Vec<String>> {
    Some(list.iter().map(|w| w.to_string()).collect())
}

#[test]
fn filler_constants_match_contract() {
    assert_eq!(CUSTOM_FILLER_MAX_WORDS, 100);
    assert_eq!(CUSTOM_FILLER_WORD_MAX_CHARS, 32);
}

#[test]
fn filler_none_stays_none_built_in_lists() {
    assert_eq!(validate_custom_filler_words(None), Ok(None));
}

#[test]
fn filler_empty_list_is_some_empty_and_distinct_from_none() {
    let result = validate_custom_filler_words(Some(vec![]));
    assert_eq!(result, Ok(Some(vec![])));
    // Filtering OFF must never collapse into the built-in-lists state.
    assert_ne!(result.unwrap(), None);
}

#[test]
fn filler_words_are_trimmed() {
    assert_eq!(
        validate_custom_filler_words(words(&["  Umm "])),
        Ok(Some(vec!["umm".to_string()]))
    );
}

#[test]
fn filler_words_are_lowercased() {
    assert_eq!(
        validate_custom_filler_words(words(&["UmM"])),
        Ok(Some(vec!["umm".to_string()]))
    );
}

#[test]
fn filler_duplicates_removed_keeping_first_occurrence() {
    assert_eq!(
        validate_custom_filler_words(words(&["Uh", "uh", " UH "])),
        Ok(Some(vec!["uh".to_string()]))
    );
    assert_eq!(
        validate_custom_filler_words(words(&["Uh", "Um", "uh"])),
        Ok(Some(vec!["uh".to_string(), "um".to_string()]))
    );
}

#[test]
fn filler_unicode_is_lowercased() {
    assert_eq!(
        validate_custom_filler_words(words(&["ÉTÉ"])),
        Ok(Some(vec!["été".to_string()]))
    );
}

#[test]
fn filler_100_entries_ok_101_entries_rejected() {
    let hundred: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
    assert!(validate_custom_filler_words(Some(hundred)).is_ok());

    let hundred_one: Vec<String> = (0..101).map(|i| format!("w{i}")).collect();
    assert!(validate_custom_filler_words(Some(hundred_one)).is_err());
}

#[test]
fn filler_raw_count_is_checked_before_dedupe() {
    // 101 raw entries that collapse to one unique word are still over the cap.
    let duplicated: Vec<String> = vec!["uh".to_string(); 101];
    assert!(validate_custom_filler_words(Some(duplicated)).is_err());
}

#[test]
fn filler_empty_word_rejected() {
    assert!(validate_custom_filler_words(words(&[""])).is_err());
}

#[test]
fn filler_whitespace_only_word_rejected() {
    assert!(validate_custom_filler_words(words(&["   "])).is_err());
}

#[test]
fn filler_33_char_word_rejected_32_char_word_accepted() {
    let ok = "a".repeat(32);
    let too_long = "a".repeat(33);
    assert_eq!(
        validate_custom_filler_words(Some(vec![ok.clone()])),
        Ok(Some(vec![ok]))
    );
    assert!(validate_custom_filler_words(Some(vec![too_long])).is_err());
}

#[test]
fn filler_length_counts_chars_not_bytes() {
    // 32 two-byte characters: 64 bytes, but 32 chars -> accepted.
    let multibyte_ok = "é".repeat(32);
    assert!(validate_custom_filler_words(Some(vec![multibyte_ok])).is_ok());

    // 33 two-byte characters -> rejected.
    let multibyte_long = "é".repeat(33);
    assert!(validate_custom_filler_words(Some(vec![multibyte_long])).is_err());
}

#[test]
fn filler_control_character_rejected() {
    assert!(validate_custom_filler_words(words(&["a\u{7}"])).is_err());
}

#[test]
fn filler_tab_inside_word_rejected() {
    assert!(validate_custom_filler_words(words(&["a\tb"])).is_err());
}

#[test]
fn filler_mixed_valid_and_invalid_rejects_whole_call() {
    // Nothing may be silently dropped: one bad entry fails the whole list.
    assert!(validate_custom_filler_words(words(&["umm", "", "uh"])).is_err());
    assert!(validate_custom_filler_words(words(&["umm", "a\u{7}"])).is_err());
}

#[test]
fn filler_serde_distinguishes_null_from_empty_list() {
    let mut s = get_default_settings();
    s.custom_filler_words = None;
    let value = serde_json::to_value(&s).expect("serialize None");
    assert_eq!(value["custom_filler_words"], serde_json::Value::Null);
    let back: AppSettings = serde_json::from_value(value).expect("deserialize null");
    assert_eq!(back.custom_filler_words, None);

    s.custom_filler_words = Some(vec![]);
    let value = serde_json::to_value(&s).expect("serialize Some([])");
    assert_eq!(value["custom_filler_words"], serde_json::json!([]));
    let back: AppSettings = serde_json::from_value(value).expect("deserialize []");
    assert_eq!(back.custom_filler_words, Some(vec![]));
}

// ---------------------------------------------------------------------------
// B) custom filler words: end-to-end through the real consumer
// ---------------------------------------------------------------------------

fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn filler_validated_custom_word_is_removed_by_consumer() {
    let validated = validate_custom_filler_words(words(&["umm"])).expect("valid list");
    let result = remove_filler_words(
        "well umm that works",
        &OutputLanguageEvidence::Unknown,
        &validated,
        true,
    );
    assert_eq!(collapse_spaces(&result), "well that works");
}

#[test]
fn filler_empty_custom_list_leaves_text_untouched_in_consumer() {
    let validated = validate_custom_filler_words(Some(vec![])).expect("valid list");
    let text = "uh that works";
    let result = remove_filler_words(text, &OutputLanguageEvidence::Unknown, &validated, true);
    assert_eq!(result, text);
}

#[test]
fn filler_none_keeps_built_in_removal_in_consumer() {
    let validated = validate_custom_filler_words(None).expect("valid list");
    let result = remove_filler_words(
        "uh that works",
        &OutputLanguageEvidence::Unknown,
        &validated,
        true,
    );
    assert_eq!(collapse_spaces(&result), "that works");
}

// ---------------------------------------------------------------------------
// C) agent flags: shape rule
// ---------------------------------------------------------------------------

#[test]
fn flag_shape_accepts_documented_forms() {
    // Real rule: optional leading "--" or "-", then 1..=32 chars where the first
    // is ASCII alphabetic and the rest are ASCII alphanumeric or '-'.
    assert!(is_valid_flag_shape("-p"));
    assert!(is_valid_flag_shape("--prompt"));
    assert!(is_valid_flag_shape("-q"));
    assert!(is_valid_flag_shape("--agent"));
    assert!(is_valid_flag_shape("--prompt-text"));
}

#[test]
fn flag_shape_requires_leading_dash() {
    // The real rule requires a leading '-' or '--'; a bare name is rejected.
    assert!(!is_valid_flag_shape("p"));
    assert!(!is_valid_flag_shape("prompt-text"));
}

#[test]
fn flag_shape_rejects_malformed_flags() {
    for bad in [
        "", "-", "--", "-1", "--1x", "-p x", " -p", "-p ", "-p;", "$(x)", "`x`",
        // The existing rule does not allow underscores.
        "--a_b",
    ] {
        assert!(!is_valid_flag_shape(bad), "should reject {bad:?}");
    }
}

#[test]
fn flag_shape_name_length_limit() {
    let name_32 = format!("-{}", "a".repeat(32));
    let name_33 = format!("-{}", "a".repeat(33));
    assert!(is_valid_flag_shape(&name_32));
    assert!(!is_valid_flag_shape(&name_33));
}

#[test]
fn extracted_rule_keeps_is_valid_agent_flag_behaviour() {
    assert!(is_valid_agent_flag("--agent", "-p"));
    assert!(!is_valid_agent_flag("-p", "-p"));
}

#[test]
fn effective_prompt_flag_uses_valid_stored_value() {
    let s = settings_with("--agent", "-q");
    assert_eq!(effective_prompt_flag(&s), "-q");
}

#[test]
fn effective_prompt_flag_falls_back_to_default_when_invalid() {
    assert_eq!(
        effective_prompt_flag(&settings_with("--agent", "; rm -rf /")),
        "-p"
    );
    assert_eq!(effective_prompt_flag(&settings_with("--agent", "")), "-p");
    assert_eq!(
        effective_prompt_flag(&settings_with("--agent", "-p x")),
        "-p"
    );
}

#[test]
fn effective_agent_flag_avoids_equality_with_effective_prompt_flag() {
    // Stored agent flag equals the stored prompt flag -> fallback to --agent.
    let s = settings_with("-p", "-p");
    assert_eq!(effective_agent_flag(&s), "--agent");
}

#[test]
fn effective_agent_flag_compares_against_effective_not_raw_prompt_flag() {
    // Raw prompt flag is invalid, so the effective prompt flag is "-p".
    // The stored agent flag "-p" equals that effective prompt flag -> --agent.
    let s = settings_with("-p", "; rm -rf /");
    assert_eq!(effective_agent_flag(&s), "--agent");
}

#[test]
fn validate_prompt_flag_accepts_valid_candidate_unchanged() {
    let s = settings_with("--agent", "-p");
    assert_eq!(
        validate_agent_bridge_prompt_flag(&s, "-q"),
        Ok("-q".to_string())
    );
}

#[test]
fn validate_prompt_flag_rejects_equal_to_stored_agent_flag() {
    let s = settings_with("--agent", "-p");
    let before_agent = s.agent_jobs_agent_flag.clone();
    let before_prompt = s.agent_bridge_prompt_flag.clone();
    let err = validate_agent_bridge_prompt_flag(&s, "--agent").unwrap_err();
    assert!(!err.trim().is_empty());
    // The settings themselves are untouched.
    assert_eq!(s.agent_jobs_agent_flag, before_agent);
    assert_eq!(s.agent_bridge_prompt_flag, before_prompt);
}

#[test]
fn validate_prompt_flag_rejects_equal_to_default_agent_fallback() {
    // Stored agent flag is invalid, so the effective agent flag is "--agent".
    let s = settings_with("bad flag", "-p");
    assert!(validate_agent_bridge_prompt_flag(&s, "--agent").is_err());
}

#[test]
fn validate_prompt_flag_rejects_malformed_candidates_without_trimming() {
    let s = settings_with("--agent", "-p");
    assert!(validate_agent_bridge_prompt_flag(&s, " -p").is_err());
    assert!(validate_agent_bridge_prompt_flag(&s, "; rm -rf /").is_err());
    assert!(validate_agent_bridge_prompt_flag(&s, "-p x").is_err());
    assert!(validate_agent_bridge_prompt_flag(&s, "").is_err());
}

#[test]
fn validate_prompt_flag_rejects_with_nonempty_message() {
    let s = settings_with("--agent", "-p");
    let err = validate_agent_bridge_prompt_flag(&s, "; rm -rf /").unwrap_err();
    assert!(!err.trim().is_empty());
}

#[test]
fn default_prompt_flag_validates_against_default_agent_flag() {
    let s = get_default_settings();
    assert!(validate_agent_bridge_prompt_flag(&s, &s.agent_bridge_prompt_flag).is_ok());
}

#[test]
fn new_valid_prompt_flag_makes_same_agent_flag_invalid() {
    // Symmetric rule: once "-q" is the prompt flag, "-q" cannot be the agent flag.
    let s = settings_with("--agent", "-q");
    assert!(!is_valid_agent_flag("-q", &effective_prompt_flag(&s)));
    assert!(!is_valid_agent_flag("-q", "-q"));
    assert_eq!(effective_agent_flag(&settings_with("-q", "-q")), "--agent");
}
