//! Spec for `natural_chunk` (step S1). Authoritative: do not weaken an assertion; if a test is
//! wrong, report it instead of editing it.
use crate::managers::natural_chunk::*;
use std::cell::Cell;

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn prose(sentences: usize) -> String {
    (0..sentences)
        .map(|i| format!("Sentence number {i} talks about the weather today and nothing else."))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn empty_and_whitespace_yield_no_chunks() {
    assert!(split_for_synthesis("", NATURAL_MAX_CHARS).is_empty());
    assert!(split_for_synthesis("   \n\t  ", NATURAL_MAX_CHARS).is_empty());
}

#[test]
fn punctuation_and_emoji_only_yield_no_chunks() {
    assert!(split_for_synthesis("...!!!", NATURAL_MAX_CHARS).is_empty());
    assert!(split_for_synthesis("\u{2014} \u{2026}", NATURAL_MAX_CHARS).is_empty());
    assert!(split_for_synthesis("\u{1F642}\u{1F642}", NATURAL_MAX_CHARS).is_empty());
}

#[test]
fn short_sentence_is_one_chunk() {
    assert_eq!(
        split_for_synthesis("  Hello there.  ", NATURAL_MAX_CHARS),
        vec!["Hello there.".to_string()]
    );
}

#[test]
fn caps_hold_first_chunk_is_short_and_no_text_is_lost() {
    let text = prose(12);
    let chunks = split_for_synthesis(&text, NATURAL_MAX_CHARS);
    assert!(chunks.len() > 5, "expected many chunks, got {}", chunks.len());
    assert!(chunks[0].chars().count() <= NATURAL_FIRST_CHUNK_CHARS);
    for c in &chunks {
        assert!(!c.trim().is_empty());
        assert_eq!(c.as_str(), c.trim());
        assert!(c.chars().count() <= NATURAL_MAX_CHARS, "too long: {c:?}");
        assert!(c.chars().any(|ch| ch.is_alphanumeric()));
    }
    assert_eq!(squash(&chunks.concat()), squash(&text));
}

#[test]
fn prefers_sentence_boundaries_over_arbitrary_cuts() {
    let s1 = "The quick fox jumps over the dog.";
    let s2 = "A second sentence follows right here.";
    let chunks = split_for_synthesis(&format!("{s1} {s2}"), NATURAL_MAX_CHARS);
    // s1 fits the first-chunk cap, so the cut must fall exactly on the sentence boundary.
    assert_eq!(chunks, vec![s1.to_string(), s2.to_string()]);
}

#[test]
fn splits_a_long_sentence_at_a_space_not_inside_a_word() {
    let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike \
                november oscar papa quebec romeo sierra tango uniform victor whiskey xray yankee zulu";
    let chunks = split_for_synthesis(text, 40);
    assert!(chunks.len() > 2);
    let original: Vec<&str> = text.split_whitespace().collect();
    let rejoined: Vec<String> = chunks
        .iter()
        .flat_map(|c| c.split_whitespace().map(String::from))
        .collect();
    assert_eq!(rejoined, original, "words must stay whole and in order");
    assert!(chunks.iter().all(|c| c.chars().count() <= 40));
}

#[test]
fn ten_thousand_chars_all_chunks_within_cap() {
    let text = prose(160); // > 10k chars
    assert!(text.chars().count() > 10_000);
    let chunks = split_for_synthesis(&text, NATURAL_MAX_CHARS);
    assert!(chunks.len() >= 125);
    assert!(chunks.iter().all(|c| c.chars().count() <= NATURAL_MAX_CHARS));
    assert_eq!(squash(&chunks.concat()), squash(&text));
}

#[test]
fn unbroken_token_is_hard_split() {
    let text = "a".repeat(400);
    let chunks = split_for_synthesis(&text, NATURAL_MAX_CHARS);
    assert!(chunks.len() >= 5);
    assert!(chunks[0].chars().count() <= NATURAL_FIRST_CHUNK_CHARS);
    assert!(chunks.iter().all(|c| c.chars().count() <= NATURAL_MAX_CHARS));
    assert_eq!(chunks.concat(), text);
}

#[test]
fn multibyte_chars_are_counted_as_chars_not_bytes() {
    let text = "\u{e9}".repeat(400);
    let chunks = split_for_synthesis(&text, NATURAL_MAX_CHARS);
    assert!(chunks.iter().all(|c| c.chars().count() <= NATURAL_MAX_CHARS));
    assert_eq!(chunks.concat(), text);
}

#[test]
fn markup_and_control_characters_are_removed() {
    let chunks = split_for_synthesis("[[slnc 500]]hello\u{7} wor\nld \t again", NATURAL_MAX_CHARS);
    let all = chunks.concat();
    assert!(!all.contains("[[") && !all.contains("]]"));
    assert!(all.chars().all(|c| !c.is_control()), "control char left in {all:?}");
    assert!(squash(&all).contains("hello"));
    assert!(squash(&all).contains("world"));
}

#[test]
fn a_small_custom_cap_is_honoured_including_the_first_chunk() {
    let chunks = split_for_synthesis("one two three four five six seven eight nine ten", 10);
    assert!(chunks.iter().all(|c| c.chars().count() <= 10), "{chunks:?}");
    assert_eq!(
        squash(&chunks.concat()),
        "onetwothreefourfivesixseveneightnineten"
    );
}

#[test]
fn unicode_text_survives_intact() {
    let chunks = split_for_synthesis("H\u{e9}llo w\u{f6}rld. \u{c7}a va?", NATURAL_MAX_CHARS);
    assert_eq!(squash(&chunks.concat()), squash("H\u{e9}llo w\u{f6}rld. \u{c7}a va?"));
}

#[test]
fn latin_gate_matrix() {
    assert!(is_mostly_latin("Hello world"));
    assert!(is_mostly_latin("Caf\u{e9} d\u{e9}j\u{e0} vu"));
    assert!(is_mostly_latin("Hello world \u{4f60}")); // 10 of 11 alphabetic chars are Latin
    assert!(!is_mostly_latin("Hello \u{4f60}\u{597d}")); // 5 of 7
    assert!(!is_mostly_latin("\u{4f60}\u{597d}\u{4e16}\u{754c}"));
    assert!(!is_mostly_latin("\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{43c}\u{438}\u{440}"));
    assert!(!is_mostly_latin(""));
    assert!(!is_mostly_latin("12345 !!!"));
}

// ---- overflow retry --------------------------------------------------------------------

const SR: u32 = 24_000;

fn alnum(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

fn good_audio(text: &str) -> Vec<f32> {
    vec![0.1; (alnum(text) as f32 * 0.09 * SR as f32) as usize]
}

fn long_text() -> String {
    // 24 words of 4 letters separated by spaces: 119 chars.
    (0..24).map(|_| "word").collect::<Vec<_>>().join(" ")
}

#[test]
fn retry_passes_through_when_the_engine_is_fine() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        Ok(good_audio(t))
    };
    let text = "hello there my friend";
    let out = synth_with_overflow_retry(&mut synth, text, SR, &|| true);
    assert_eq!(calls.get(), 1);
    assert_eq!(
        out,
        RetryOutcome::Audio {
            samples: good_audio(text),
            dropped_pieces: 0
        }
    );
}

#[test]
fn retry_halves_when_the_engine_errors_on_long_text() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        if t.chars().count() > 70 {
            Err("token overflow".to_string())
        } else {
            Ok(good_audio(t))
        }
    };
    let text = long_text();
    let out = synth_with_overflow_retry(&mut synth, &text, SR, &|| true);
    assert_eq!(calls.get(), 3, "1 failing call + 2 halves");
    match out {
        RetryOutcome::Audio {
            samples,
            dropped_pieces,
        } => {
            assert_eq!(dropped_pieces, 0);
            let expected = good_audio(&text).len() as i64;
            assert!((samples.len() as i64 - expected).abs() <= 3);
        }
        other => panic!("expected audio, got {other:?}"),
    }
}

#[test]
fn retry_treats_too_short_audio_as_truncation() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        if t.chars().count() > 70 {
            Ok(vec![0.1; 10]) // silently truncated
        } else {
            Ok(good_audio(t))
        }
    };
    let out = synth_with_overflow_retry(&mut synth, &long_text(), SR, &|| true);
    assert_eq!(calls.get(), 3);
    assert!(matches!(out, RetryOutcome::Audio { dropped_pieces: 0, .. }));
}

#[test]
fn retry_always_failing_engine_terminates_with_dropped() {
    let calls = Cell::new(0);
    let mut synth = |_t: &str| {
        calls.set(calls.get() + 1);
        Err::<Vec<f32>, String>("boom".into())
    };
    let out = synth_with_overflow_retry(&mut synth, &long_text(), SR, &|| true);
    assert_eq!(out, RetryOutcome::Dropped);
    assert!(calls.get() <= 7, "unbounded retries: {}", calls.get());
    assert!(calls.get() >= 3);
}

#[test]
fn retry_always_truncating_engine_terminates_with_dropped() {
    let calls = Cell::new(0);
    let mut synth = |_t: &str| {
        calls.set(calls.get() + 1);
        Ok(vec![0.1f32; 5])
    };
    let out = synth_with_overflow_retry(&mut synth, &long_text(), SR, &|| true);
    assert_eq!(out, RetryOutcome::Dropped);
    assert!(calls.get() <= 7);
}

#[test]
fn retry_keeps_good_pieces_and_counts_the_dropped_one() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        if t.contains("BAD") {
            Err("bad".to_string())
        } else {
            Ok(good_audio(t))
        }
    };
    let text = "aaaa bbbb cccc dddd eeee ffff gggg BAD hhhh iiii jjjj kkkk";
    let out = synth_with_overflow_retry(&mut synth, text, SR, &|| true);
    assert_eq!(calls.get(), 5, "1 + 2 halves + 2 quarters of the failing half");
    match out {
        RetryOutcome::Audio {
            samples,
            dropped_pieces,
        } => {
            assert_eq!(dropped_pieces, 1);
            assert!(!samples.is_empty());
        }
        other => panic!("expected partial audio, got {other:?}"),
    }
}

#[test]
fn retry_cancelled_after_first_call_makes_no_recursive_calls() {
    // C11: a stopped chunk must not start further engine runs.
    let calls = Cell::new(0);
    let mut synth = |_t: &str| {
        calls.set(calls.get() + 1);
        Err::<Vec<f32>, String>("boom".into())
    };
    let is_current = || calls.get() == 0;
    let out = synth_with_overflow_retry(&mut synth, &long_text(), SR, &is_current);
    assert_eq!(out, RetryOutcome::Cancelled);
    assert_eq!(calls.get(), 1);
}

#[test]
fn retry_cancelled_before_the_first_call_never_calls_the_engine() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        Ok(good_audio(t))
    };
    let out = synth_with_overflow_retry(&mut synth, "hello world", SR, &|| false);
    assert_eq!(out, RetryOutcome::Cancelled);
    assert_eq!(calls.get(), 0);
}

#[test]
fn retry_discards_good_audio_when_cancelled_during_the_call() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        Ok(good_audio(t))
    };
    let is_current = || calls.get() == 0;
    let out = synth_with_overflow_retry(&mut synth, "hello world", SR, &is_current);
    assert_eq!(out, RetryOutcome::Cancelled);
    assert_eq!(calls.get(), 1);
}

#[test]
fn retry_text_without_alphanumerics_is_dropped_without_a_call() {
    let calls = Cell::new(0);
    let mut synth = |t: &str| {
        calls.set(calls.get() + 1);
        Ok(good_audio(t))
    };
    assert_eq!(
        synth_with_overflow_retry(&mut synth, "", SR, &|| true),
        RetryOutcome::Dropped
    );
    assert_eq!(
        synth_with_overflow_retry(&mut synth, "... !!!", SR, &|| true),
        RetryOutcome::Dropped
    );
    assert_eq!(calls.get(), 0);
}
