//! Portable tests for `parse_say_voice_list` and the non-macOS `list_installed_voices`.
//! Covers the corner cases in DESIGN.md "Acceptance checks (Rust corner cases, parse)".
//! Non-ASCII characters are written as escapes so this source file stays ASCII.

use super::*;
use crate::managers::tts::sanitize_voice;

/// Real-shape `say -v '?'` lines (columns separated by spaces). The accented
/// names are skipped by the parser.
const FIXTURE: &str = concat!(
    "Albert              en_US    # Hello! My name is Albert.\n",
    "Alice               it_IT    # Ciao! Mi chiamo Alice.\n",
    "Ava (Premium)       en_US    # Hello! My name is Ava.\n",
    "Bad News            en_US    # Hello! My name is Bad News.\n",
    "Eddy (English (US)) en_US    # Hello! My name is Eddy.\n",
    "Grandma (Spanish (Mexico)) es_MX    # Hola! Me llamo Grandma.\n",
    "Reed (English (UK)) en_GB    # Hello! My name is Reed.\n",
    "Paulina             es_419   # Hola! Me llamo Paulina.\n",
    "Am\u{e9}lie              fr_CA    # Bonjour! Je m'appelle Am\u{e9}lie.\n",
    "T\u{fc}nde               hu_HU    # Szia! T\u{fc}nde vagyok.\n",
);

fn voice(name: &str, locale: &str) -> TtsVoice {
    TtsVoice {
        name: name.to_string(),
        locale: locale.to_string(),
    }
}

#[test]
fn fixture_parses_ascii_voices_sorted_by_locale_then_name() {
    let got = parse_say_voice_list(FIXTURE);
    let expected = vec![
        voice("Reed (English (UK))", "en_GB"),
        voice("Albert", "en_US"),
        voice("Ava (Premium)", "en_US"),
        voice("Bad News", "en_US"),
        voice("Eddy (English (US))", "en_US"),
        voice("Paulina", "es_419"),
        voice("Grandma (Spanish (Mexico))", "es_MX"),
        voice("Alice", "it_IT"),
    ];
    assert_eq!(got, expected);
}

#[test]
fn fixture_accented_names_are_absent() {
    let got = parse_say_voice_list(FIXTURE);
    assert!(
        got.iter()
            .all(|v| !v.name.contains("Am\u{e9}lie") && !v.name.contains("T\u{fc}nde")),
        "{got:?}"
    );
}

#[test]
fn every_parsed_name_passes_sanitize_voice_unchanged() {
    for v in parse_say_voice_list(FIXTURE) {
        assert_eq!(
            sanitize_voice(&v.name).as_deref(),
            Some(v.name.as_str()),
            "{v:?}"
        );
    }
}

#[test]
fn non_ascii_names_are_skipped() {
    let input = "Am\u{e9}lie fr_CA # x\nT\u{fc}nde hu_HU # x\nvoz\u{e9} es_ES # x\n";
    assert_eq!(parse_say_voice_list(input), Vec::<TtsVoice>::new());
}

#[test]
fn empty_string_yields_no_voices() {
    assert_eq!(parse_say_voice_list(""), Vec::<TtsVoice>::new());
}

#[test]
fn only_blank_lines_yield_no_voices() {
    assert_eq!(parse_say_voice_list("\n\n\n"), Vec::<TtsVoice>::new());
}

#[test]
fn whitespace_only_yields_no_voices() {
    assert_eq!(
        parse_say_voice_list("   \t  \n \t \n"),
        Vec::<TtsVoice>::new()
    );
}

#[test]
fn line_without_hash_is_skipped() {
    let input = "Albert en_US\nAlice it_IT # Ciao\n";
    assert_eq!(parse_say_voice_list(input), vec![voice("Alice", "it_IT")]);
}

#[test]
fn line_without_locale_token_is_skipped() {
    let input = "# only a sample\nAlbert # sample\n";
    assert_eq!(parse_say_voice_list(input), Vec::<TtsVoice>::new());
}

#[test]
fn bad_locale_shapes_are_skipped() {
    let input = "Albert English # x\nAlbert EN_us # x\nAlbert en_ # x\n";
    assert_eq!(parse_say_voice_list(input), Vec::<TtsVoice>::new());
}

#[test]
fn sample_with_extra_hash_characters_still_parses() {
    let input = "Albert en_US # Number # 5\n";
    assert_eq!(parse_say_voice_list(input), vec![voice("Albert", "en_US")]);
}

#[test]
fn tab_separated_columns_parse() {
    let input = "Albert\ten_US\t# Hello\nAlice\t\tit_IT\t# Ciao\n";
    assert_eq!(
        parse_say_voice_list(input),
        vec![voice("Albert", "en_US"), voice("Alice", "it_IT")]
    );
}

#[test]
fn crlf_line_endings_parse() {
    let input = "Albert en_US # x\r\nAlice it_IT # y\r\n";
    assert_eq!(
        parse_say_voice_list(input),
        vec![voice("Albert", "en_US"), voice("Alice", "it_IT")]
    );
}

#[test]
fn nested_parentheses_in_name_are_kept() {
    let input = "Eddy (English (US)) en_US # x\n";
    assert_eq!(
        parse_say_voice_list(input),
        vec![voice("Eddy (English (US))", "en_US")]
    );
}

#[test]
fn leading_dash_name_is_skipped() {
    let input = "-evil en_US # x\n";
    assert_eq!(parse_say_voice_list(input), Vec::<TtsVoice>::new());
}

#[test]
fn name_longer_than_64_chars_is_skipped_and_64_kept() {
    let long = "a".repeat(65);
    let exact = "b".repeat(64);
    let input = format!("{long} en_US # x\n{exact} en_US # x\n");
    assert_eq!(parse_say_voice_list(&input), vec![voice(&exact, "en_US")]);
}

#[test]
fn control_characters_in_name_are_skipped() {
    let input = "Al\u{7}bert en_US # x\nAl\u{1b}ice en_US # x\n";
    assert_eq!(parse_say_voice_list(input), Vec::<TtsVoice>::new());
}

#[test]
fn duplicate_names_keep_the_first_entry() {
    let input = "Albert en_US # first\nAlbert fr_FR # second\n";
    assert_eq!(parse_say_voice_list(input), vec![voice("Albert", "en_US")]);
}

#[test]
fn more_than_300_distinct_voices_are_capped_at_300() {
    let mut input = String::new();
    for i in 0..400 {
        input.push_str(&format!("Voice{i:03} en_US # x\n"));
    }
    assert_eq!(parse_say_voice_list(&input).len(), 300);
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn list_installed_voices_is_empty_off_macos() {
    assert!(list_installed_voices().await.is_empty());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_say_lists_only_sanitized_voices() {
    let voices = list_installed_voices().await;
    assert!(!voices.is_empty(), "real `say -v ?` listed no voices");
    for v in &voices {
        assert_eq!(
            sanitize_voice(&v.name).as_deref(),
            Some(v.name.as_str()),
            "{v:?}"
        );
        let shape_ok = v.locale.split_once('_').is_some_and(|(lang, region)| {
            !lang.is_empty()
                && lang.chars().all(|c| c.is_ascii_alphabetic())
                && !region.is_empty()
                && region.chars().all(|c| c.is_ascii_alphanumeric())
        });
        assert!(shape_ok, "locale shape: {v:?}");
    }
}
