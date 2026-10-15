//! M4 S1b RED tests for design D1 (coding agent settings validation).
//!
//! These tests define the contract for the agent allowlist, the agent flag, the ask switch and
//! the settings defaults and compatibility. They fail until the S1b stubs are implemented.

use super::*;
use crate::settings::{
    clean_agent_names, effective_agent_flag, get_default_settings, is_valid_agent_flag, AppSettings,
};

fn s(text: &str) -> String {
    text.to_string()
}

fn settings_with_prompt_flag(prompt_flag: &str) -> AppSettings {
    let mut settings = get_default_settings();
    settings.agent_bridge_prompt_flag = prompt_flag.to_string();
    settings
}

// ---- clean_agent_names: valid names ------------------------------------------------------------

#[test]
fn clean_agent_names_keeps_valid_names_in_order() {
    let input = vec![s("coder-one"), s("a"), s("A1._-x")];
    assert_eq!(
        clean_agent_names(input),
        vec![s("coder-one"), s("a"), s("A1._-x")]
    );
}

#[test]
fn clean_agent_names_keeps_a_64_character_name() {
    let name = format!("a{}", "b".repeat(63));
    assert_eq!(name.len(), 64);
    assert_eq!(clean_agent_names(vec![name.clone()]), vec![name]);
}

#[test]
fn clean_agent_names_drops_a_65_character_name() {
    let name = format!("a{}", "b".repeat(64));
    assert_eq!(name.len(), 65);
    assert!(clean_agent_names(vec![name]).is_empty());
}

// ---- clean_agent_names: invalid entries are dropped --------------------------------------------

#[test]
fn clean_agent_names_drops_empty_string() {
    assert!(clean_agent_names(vec![s("")]).is_empty());
}

#[test]
fn clean_agent_names_drops_single_space() {
    assert!(clean_agent_names(vec![s(" ")]).is_empty());
}

#[test]
fn clean_agent_names_drops_leading_hyphen_name() {
    assert!(clean_agent_names(vec![s("-x")]).is_empty());
}

#[test]
fn clean_agent_names_drops_flag_shaped_name() {
    assert!(clean_agent_names(vec![s("--agent")]).is_empty());
}

#[test]
fn clean_agent_names_drops_leading_dot_name() {
    assert!(clean_agent_names(vec![s(".x")]).is_empty());
}

#[test]
fn clean_agent_names_drops_leading_underscore_name() {
    assert!(clean_agent_names(vec![s("_x")]).is_empty());
}

#[test]
fn clean_agent_names_drops_name_with_space() {
    assert!(clean_agent_names(vec![s("a b")]).is_empty());
}

#[test]
fn clean_agent_names_drops_name_with_tab() {
    assert!(clean_agent_names(vec![s("a\tb")]).is_empty());
}

#[test]
fn clean_agent_names_drops_name_with_newline() {
    assert!(clean_agent_names(vec![s("a\nb")]).is_empty());
}

#[test]
fn clean_agent_names_drops_path_traversal_name() {
    assert!(clean_agent_names(vec![s("../x")]).is_empty());
}

#[test]
fn clean_agent_names_drops_slash_name() {
    assert!(clean_agent_names(vec![s("a/b")]).is_empty());
}

#[test]
fn clean_agent_names_drops_shell_separator_name() {
    assert!(clean_agent_names(vec![s("a;b")]).is_empty());
}

#[test]
fn clean_agent_names_drops_accented_latin_name() {
    assert!(clean_agent_names(vec![s("\u{e9}")]).is_empty());
}

#[test]
fn clean_agent_names_drops_fullwidth_latin_name() {
    assert!(clean_agent_names(vec![s("\u{ff41}")]).is_empty());
}

#[test]
fn clean_agent_names_drops_name_with_nul() {
    assert!(clean_agent_names(vec![s("a\0b")]).is_empty());
}

#[test]
fn clean_agent_names_does_not_trim_leading_space() {
    assert!(clean_agent_names(vec![s(" ram")]).is_empty());
}

#[test]
fn clean_agent_names_does_not_trim_trailing_space() {
    assert!(clean_agent_names(vec![s("ram ")]).is_empty());
}

#[test]
fn clean_agent_names_keeps_valid_names_next_to_invalid_ones() {
    let input = vec![s("bad name"), s("ram"), s("-x"), s("ai")];
    assert_eq!(clean_agent_names(input), vec![s("ram"), s("ai")]);
}

// ---- clean_agent_names: duplicates, cap, empty, volume -----------------------------------------

#[test]
fn clean_agent_names_drops_case_insensitive_duplicates_keeping_first_spelling() {
    let input = vec![s("Ram"), s("ram"), s("RAM")];
    assert_eq!(clean_agent_names(input), vec![s("Ram")]);
}

#[test]
fn clean_agent_names_keeps_first_spelling_when_lowercase_comes_first() {
    let input = vec![s("ram"), s("Ram")];
    assert_eq!(clean_agent_names(input), vec![s("ram")]);
}

#[test]
fn clean_agent_names_keeps_only_the_first_20_valid_entries() {
    let input: Vec<String> = (0..25).map(|i| format!("agent{i}")).collect();
    let expected: Vec<String> = (0..20).map(|i| format!("agent{i}")).collect();
    assert_eq!(clean_agent_names(input), expected);
}

#[test]
fn clean_agent_names_cap_counts_only_valid_entries() {
    let mut input = vec![s("-bad"), s("")];
    input.extend((0..21).map(|i| format!("agent{i}")));
    let cleaned = clean_agent_names(input);
    assert_eq!(cleaned.len(), 20);
    assert_eq!(cleaned.first(), Some(&s("agent0")));
    assert_eq!(cleaned.last(), Some(&s("agent19")));
}

#[test]
fn clean_agent_names_empty_input_gives_empty_output() {
    assert!(clean_agent_names(Vec::new()).is_empty());
}

#[test]
fn clean_agent_names_thousand_invalid_entries_return_empty_without_panic() {
    let input = vec![s("-x"); 1000];
    assert!(clean_agent_names(input).is_empty());
}

// ---- is_valid_agent_flag -----------------------------------------------------------------------

#[test]
fn agent_flag_accepts_long_default_flag() {
    assert!(is_valid_agent_flag("--agent", "-p"));
}

#[test]
fn agent_flag_accepts_short_flag() {
    assert!(is_valid_agent_flag("-a", "-p"));
}

#[test]
fn agent_flag_accepts_hyphenated_long_flag() {
    assert!(is_valid_agent_flag("--agent-name", "-p"));
}

#[test]
fn agent_flag_accepts_mixed_case_with_digit() {
    assert!(is_valid_agent_flag("--A1", "-p"));
}

#[test]
fn agent_flag_rejects_empty_string() {
    assert!(!is_valid_agent_flag("", "-p"));
}

#[test]
fn agent_flag_rejects_bare_word_without_dashes() {
    assert!(!is_valid_agent_flag("agent", "-p"));
}

#[test]
fn agent_flag_rejects_single_hyphen_only() {
    assert!(!is_valid_agent_flag("-", "-p"));
}

#[test]
fn agent_flag_rejects_double_hyphen_only() {
    assert!(!is_valid_agent_flag("--", "-p"));
}

#[test]
fn agent_flag_rejects_triple_hyphen_prefix() {
    assert!(!is_valid_agent_flag("---x", "-p"));
}

#[test]
fn agent_flag_rejects_space_inside() {
    assert!(!is_valid_agent_flag("--a b", "-p"));
}

#[test]
fn agent_flag_rejects_trailing_newline() {
    assert!(!is_valid_agent_flag("--a\n", "-p"));
}

#[test]
fn agent_flag_rejects_digit_after_dashes() {
    assert!(!is_valid_agent_flag("--1x", "-p"));
}

#[test]
fn agent_flag_accepts_exactly_32_characters_after_double_dash() {
    let flag = format!("--{}", "a".repeat(32));
    assert!(is_valid_agent_flag(&flag, "-p"));
}

#[test]
fn agent_flag_rejects_33_characters_after_double_dash() {
    let flag = format!("--{}", "a".repeat(33));
    assert!(!is_valid_agent_flag(&flag, "-p"));
}

#[test]
fn agent_flag_accepts_exactly_32_characters_after_single_dash() {
    let flag = format!("-{}", "a".repeat(32));
    assert!(is_valid_agent_flag(&flag, "-p"));
}

#[test]
fn agent_flag_rejects_33_characters_after_single_dash() {
    let flag = format!("-{}", "a".repeat(33));
    assert!(!is_valid_agent_flag(&flag, "-p"));
}

#[test]
fn agent_flag_rejects_flag_equal_to_prompt_flag() {
    assert!(!is_valid_agent_flag("-p", "-p"));
}

#[test]
fn agent_flag_rejects_long_flag_equal_to_custom_prompt_flag() {
    assert!(!is_valid_agent_flag("--agent", "--agent"));
}

#[test]
fn agent_flag_rejects_unicode_letter() {
    assert!(!is_valid_agent_flag("--\u{e9}", "-p"));
}

#[test]
fn agent_flag_rejects_fullwidth_letter() {
    assert!(!is_valid_agent_flag("--\u{ff41}", "-p"));
}

#[test]
fn agent_flag_rejects_nul() {
    assert!(!is_valid_agent_flag("--a\0", "-p"));
}

// ---- effective_agent_flag ----------------------------------------------------------------------

#[test]
fn effective_agent_flag_defaults_to_long_agent_flag() {
    assert_eq!(effective_agent_flag(&get_default_settings()), "--agent");
}

#[test]
fn effective_agent_flag_falls_back_for_invalid_stored_value() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agent_flag = s("agent");
    assert_eq!(effective_agent_flag(&settings), "--agent");
}

#[test]
fn effective_agent_flag_falls_back_for_stored_value_with_space() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agent_flag = s("--a b");
    assert_eq!(effective_agent_flag(&settings), "--agent");
}

#[test]
fn effective_agent_flag_returns_valid_stored_value() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agent_flag = s("-a");
    assert_eq!(effective_agent_flag(&settings), "-a");
}

#[test]
fn effective_agent_flag_falls_back_when_stored_flag_equals_prompt_flag() {
    let mut settings = settings_with_prompt_flag("-a");
    settings.agent_jobs_agent_flag = s("-a");
    assert_eq!(effective_agent_flag(&settings), "--agent");
}

#[test]
fn effective_agent_flag_falls_back_when_stored_flag_equals_default_prompt_flag() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agent_flag = s("-p");
    assert_eq!(effective_agent_flag(&settings), "--agent");
}

// ---- apply_agents_change -----------------------------------------------------------------------

#[test]
fn apply_agents_change_stores_and_returns_cleaned_list() {
    let mut settings = get_default_settings();
    let returned = apply_agents_change(&mut settings, vec![s("ram"), s("-bad"), s("RAM")]);
    assert_eq!(returned, vec![s("ram")]);
    assert_eq!(settings.agent_jobs_agents, vec![s("ram")]);
}

#[test]
fn apply_agents_change_second_call_replaces_the_list() {
    let mut settings = get_default_settings();
    apply_agents_change(&mut settings, vec![s("first")]);
    let returned = apply_agents_change(&mut settings, vec![s("second"), s("third")]);
    assert_eq!(returned, vec![s("second"), s("third")]);
    assert_eq!(settings.agent_jobs_agents, vec![s("second"), s("third")]);
}

#[test]
fn apply_agents_change_empty_input_clears_the_list() {
    let mut settings = get_default_settings();
    apply_agents_change(&mut settings, vec![s("first")]);
    let returned = apply_agents_change(&mut settings, Vec::new());
    assert!(returned.is_empty());
    assert!(settings.agent_jobs_agents.is_empty());
}

// ---- apply_flag_change -------------------------------------------------------------------------

#[test]
fn apply_flag_change_stores_and_returns_valid_flag() {
    let mut settings = get_default_settings();
    let result = apply_flag_change(&mut settings, s("--agent-name"));
    assert_eq!(result, Ok(s("--agent-name")));
    assert_eq!(settings.agent_jobs_agent_flag, "--agent-name");
}

#[test]
fn apply_flag_change_rejects_invalid_flag_and_keeps_previous_value() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agent_flag = s("-a");
    let result = apply_flag_change(&mut settings, s("agent"));
    assert_eq!(result, Err(s("Invalid agent flag.")));
    assert_eq!(settings.agent_jobs_agent_flag, "-a");
}

#[test]
fn apply_flag_change_rejects_empty_flag() {
    let mut settings = get_default_settings();
    let result = apply_flag_change(&mut settings, String::new());
    assert_eq!(result, Err(s("Invalid agent flag.")));
    assert_eq!(settings.agent_jobs_agent_flag, "--agent");
}

#[test]
fn apply_flag_change_rejects_flag_equal_to_prompt_flag_and_keeps_previous_value() {
    let mut settings = get_default_settings();
    let result = apply_flag_change(&mut settings, s("-p"));
    assert_eq!(result, Err(s("Invalid agent flag.")));
    assert_eq!(settings.agent_jobs_agent_flag, "--agent");
}

#[test]
fn apply_flag_change_rejects_flag_equal_to_custom_prompt_flag() {
    let mut settings = settings_with_prompt_flag("--agent");
    let result = apply_flag_change(&mut settings, s("--agent"));
    assert_eq!(result, Err(s("Invalid agent flag.")));
    assert_eq!(settings.agent_jobs_agent_flag, "--agent");
}

// ---- apply_ask_change --------------------------------------------------------------------------

#[test]
fn apply_ask_change_sets_true_then_false() {
    let mut settings = get_default_settings();
    apply_ask_change(&mut settings, true);
    assert!(settings.agent_jobs_ask_enabled);
    apply_ask_change(&mut settings, false);
    assert!(!settings.agent_jobs_ask_enabled);
}

// ---- defaults and M3 compatibility -------------------------------------------------------------

#[test]
fn m4_defaults_are_empty_list_long_flag_and_ask_off() {
    let settings = get_default_settings();
    assert!(settings.agent_jobs_agents.is_empty());
    assert_eq!(settings.agent_jobs_agent_flag, "--agent");
    assert!(!settings.agent_jobs_ask_enabled);
}

#[test]
fn m4_settings_missing_from_m3_json_deserialize_to_defaults() {
    let mut settings = get_default_settings();
    settings.agent_jobs_enabled = true;
    settings.agent_jobs_timeout_minutes = 75;
    let mut value = serde_json::to_value(&settings).unwrap();
    let obj = value.as_object_mut().unwrap();
    for key in [
        "agent_jobs_agents",
        "agent_jobs_agent_flag",
        "agent_jobs_ask_enabled",
    ] {
        assert!(obj.remove(key).is_some(), "{key} must be serialized");
    }
    let loaded: AppSettings =
        serde_json::from_value(value).expect("M3-era settings JSON must deserialize");
    assert!(loaded.agent_jobs_agents.is_empty());
    assert_eq!(loaded.agent_jobs_agent_flag, "--agent");
    assert!(!loaded.agent_jobs_ask_enabled);
}

#[test]
fn m4_settings_missing_from_m3_json_keep_m3_job_fields_unchanged() {
    let mut settings = get_default_settings();
    settings.agent_jobs_enabled = true;
    settings.agent_jobs_timeout_minutes = 75;
    let mut value = serde_json::to_value(&settings).unwrap();
    let obj = value.as_object_mut().unwrap();
    for key in [
        "agent_jobs_agents",
        "agent_jobs_agent_flag",
        "agent_jobs_ask_enabled",
    ] {
        obj.remove(key);
    }
    let loaded: AppSettings = serde_json::from_value(value).unwrap();
    assert!(loaded.agent_jobs_enabled);
    assert_eq!(loaded.agent_jobs_timeout_minutes, 75);
}

#[test]
fn m4_settings_round_trip_keeps_agent_fields() {
    let mut settings = get_default_settings();
    settings.agent_jobs_agents = vec![s("ram"), s("ai")];
    settings.agent_jobs_agent_flag = s("-a");
    settings.agent_jobs_ask_enabled = true;
    let json = serde_json::to_string(&settings).unwrap();
    let loaded: AppSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.agent_jobs_agents, vec![s("ram"), s("ai")]);
    assert_eq!(loaded.agent_jobs_agent_flag, "-a");
    assert!(loaded.agent_jobs_ask_enabled);
}
