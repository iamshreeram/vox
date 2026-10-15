// M3 S3a tests: CancelJob local intent (D6, R2-10) and default invariance.
// Included from converse_local.rs as a child module, so `super::*` is the local module.

use super::*;

const CANCEL_PHRASES: [&str; 6] = [
    "cancel the job",
    "stop the job",
    "cancel the coding job",
    "stop the coding job",
    "cancel job",
    "stop job",
];

// ---------------- CancelJob phrases when enabled ----------------

#[test]
fn each_cancel_phrase_returns_cancel_job_when_enabled() {
    for phrase in CANCEL_PHRASES {
        assert_eq!(
            classify_local_with(phrase, true),
            LocalIntent::CancelJob,
            "phrase: {phrase:?}"
        );
    }
}

#[test]
fn cancel_phrases_ignore_case_punctuation_and_spacing() {
    for text in [
        "Cancel the job.",
        "CANCEL THE JOB!!",
        "Stop the Job",
        "cancel, the job",
        "  stop   the job  ",
        "Stop the coding job.",
        "cancel job!",
        "STOP JOB",
    ] {
        assert_eq!(
            classify_local_with(text, true),
            LocalIntent::CancelJob,
            "text: {text:?}"
        );
    }
}

#[test]
fn cancel_phrase_inside_a_longer_request_is_not_cancel_job() {
    for text in [
        "stop the job now",
        "please stop the job",
        "cancel the job please",
        "stop the job and the music",
        "cancel the coding job for me",
    ] {
        assert_ne!(
            classify_local_with(text, true),
            LocalIntent::CancelJob,
            "text: {text:?}"
        );
        assert_eq!(
            classify_local_with(text, true),
            LocalIntent::None,
            "text: {text:?}"
        );
    }
}

#[test]
fn cancel_phrase_after_a_dictation_prefix_stays_dictation() {
    assert_eq!(
        classify_local_with("dictate stop the job", true),
        LocalIntent::Dictate("stop the job".to_string())
    );
}

#[test]
fn lookalike_unicode_letters_do_not_match_cancel_phrases() {
    // Cyrillic small a (U+0430) in place of the ASCII letter in "cancel".
    assert_eq!(
        classify_local_with("c\u{430}ncel the job", true),
        LocalIntent::None
    );
}

#[test]
fn empty_and_whitespace_only_are_none_when_enabled() {
    assert_eq!(classify_local_with("", true), LocalIntent::None);
    assert_eq!(classify_local_with(" \t\n ", true), LocalIntent::None);
}

// ---------------- disabled: legacy result ----------------

#[test]
fn cancel_phrases_with_cancel_job_disabled_return_the_legacy_result() {
    for phrase in CANCEL_PHRASES {
        assert_eq!(
            classify_local_with(phrase, false),
            classify_local(phrase),
            "phrase: {phrase:?}"
        );
        assert_ne!(
            classify_local_with(phrase, false),
            LocalIntent::CancelJob,
            "phrase: {phrase:?}"
        );
    }
}

#[test]
fn stop_job_with_cancel_disabled_is_none_as_before() {
    // Default invariance: an exact configured phrase keeps the legacy routing.
    assert_eq!(classify_local_with("stop job", false), LocalIntent::None);
    assert_eq!(classify_local("stop job"), LocalIntent::None);
}

#[test]
fn classify_local_outputs_are_unchanged_for_existing_inputs() {
    assert_eq!(classify_local("stop"), LocalIntent::End);
    assert_eq!(classify_local("bye"), LocalIntent::End);
    assert_eq!(classify_local("goodbye"), LocalIntent::End);
    assert_eq!(classify_local("that's all"), LocalIntent::End);
    assert_eq!(classify_local("exit"), LocalIntent::End);
    assert_eq!(
        classify_local("dictate hello world"),
        LocalIntent::Dictate("hello world".to_string())
    );
    assert_eq!(classify_local("paste"), LocalIntent::None);
    assert_eq!(classify_local("open Safari"), LocalIntent::None);
    assert_eq!(classify_local(""), LocalIntent::None);
}

#[test]
fn classify_local_with_false_equals_classify_local_for_a_corpus() {
    let corpus = [
        "stop",
        "Stop.",
        "stop listening",
        "bye",
        "goodbye",
        "that's all",
        "we're done",
        "never mind",
        "exit",
        "stop the music",
        "bye bye now",
        "dictate hello world",
        "Type this: Hello there.",
        "write this down, buy milk",
        "paste",
        "type of music is jazz",
        "pasted text",
        "open Safari",
        "",
        "   ",
        "cancel the job",
        "stop job",
    ];
    for text in corpus {
        assert_eq!(
            classify_local_with(text, false),
            classify_local(text),
            "text: {text:?}"
        );
    }
}

// ---------------- END phrases still behave as before when enabled ----------------

#[test]
fn plain_end_phrases_are_unchanged_when_cancel_is_enabled() {
    for text in [
        "stop",
        "Stop.",
        "bye",
        "goodbye",
        "that's all",
        "exit",
        "never mind",
    ] {
        assert_eq!(
            classify_local_with(text, true),
            LocalIntent::End,
            "text: {text:?}"
        );
    }
}

#[test]
fn enabled_result_matches_legacy_for_non_cancel_inputs() {
    for text in [
        "stop the music",
        "bye bye now",
        "dictate hello world",
        "open Safari",
        "pastel colors",
        "",
    ] {
        assert_eq!(
            classify_local_with(text, true),
            classify_local(text),
            "text: {text:?}"
        );
    }
}

#[test]
fn cancel_job_variant_is_distinct_from_the_other_intents() {
    assert_ne!(LocalIntent::CancelJob, LocalIntent::End);
    assert_ne!(LocalIntent::CancelJob, LocalIntent::None);
    assert_ne!(LocalIntent::CancelJob, LocalIntent::Dictate(String::new()));
}
