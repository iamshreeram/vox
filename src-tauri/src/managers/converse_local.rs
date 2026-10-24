//! Local (no model) classification of a spoken utterance during a
//! conversation session: explicit "end the session" phrases and explicit
//! dictation prefixes. Pure functions, no I/O.
//!
//! Contract: see DESIGN.md section 3.1 (Revision 4).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalIntent {
    /// The user wants to end the conversation.
    End,
    /// The user wants this exact text pasted into the active window.
    Dictate(String),
    /// M3: the user wants the running coding job cancelled (only from
    /// `classify_local_with` with `cancel_job_enabled == true`).
    #[allow(dead_code)] // M3: wired in S3b
    CancelJob,
    /// Stage 1 roles: the user ends the active role (only from `classify_local_in_role` while a
    /// role is active).
    ClearRole,
    /// Nothing local matched.
    None,
}

/// Whole-utterance end phrases (compared on word keys).
const END_PHRASES: &[&[&str]] = &[
    &["stop"],
    &["stop", "listening"],
    &["stop", "conversation"],
    &["end", "conversation"],
    &["goodbye"],
    &["good", "bye"],
    &["bye"],
    &["thats", "all"],
    &["that", "is", "all"],
    &["were", "done"],
    &["we", "are", "done"],
    &["im", "done"],
    &["i", "am", "done"],
    &["never", "mind"],
    &["nevermind"],
    &["exit"],
];

/// Dictation prefixes, ordered longest first so the longest match wins.
const DICTATE_PREFIXES: &[&[&str]] = &[
    &["write", "this", "down"],
    &["write", "down"],
    &["type", "this"],
    &["type", "out"],
    &["paste", "this"],
    &["dictate"],
    &["paste"],
];

/// One whitespace-delimited word with a non-empty key.
struct Word {
    /// Byte offset just past the end of this word in the original text.
    end: usize,
    key: String,
}

/// Word key: lowercase, with ASCII punctuation and apostrophes (incl. U+2019) removed.
fn word_key(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_ascii_punctuation() && *c != '\u{2019}')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Split on Unicode whitespace, keeping byte offsets. Words whose key is empty are skipped.
fn tokenize(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut push = |start: usize, end: usize| {
        if let Some(raw) = text.get(start..end) {
            let key = word_key(raw);
            if !key.is_empty() {
                words.push(Word { end, key });
            }
        }
    };
    let mut start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                push(s, i);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push(s, text.len());
    }
    words
}

/// If the leading keys equal `pattern`, return the end offset of the last matched word.
fn match_prefix(words: &[Word], pattern: &[&str]) -> Option<usize> {
    let mut end = None;
    let mut matched = 0usize;
    for (word, expected) in words.iter().zip(pattern) {
        if word.key != *expected {
            return None;
        }
        end = Some(word.end);
        matched += 1;
    }
    if !pattern.is_empty() && matched == pattern.len() {
        end
    } else {
        None
    }
}

/// M3: whole-utterance cancel phrases (compared on word keys).
#[allow(dead_code)] // M3: wired in S3b
const CANCEL_JOB_PHRASES: &[&[&str]] = &[
    &["cancel", "the", "job"],
    &["stop", "the", "job"],
    &["cancel", "the", "coding", "job"],
    &["stop", "the", "coding", "job"],
    &["cancel", "job"],
    &["stop", "job"],
];

/// M3: classify with the cancel-job intent optionally enabled. With
/// `cancel_job_enabled == false` the result equals `classify_local(text)`.
/// When enabled, a whole utterance equal to a cancel phrase is checked
/// before the END phrases.
#[allow(dead_code)] // M3: wired in S3b
pub fn classify_local_with(text: &str, cancel_job_enabled: bool) -> LocalIntent {
    if cancel_job_enabled && is_cancel_job_utterance(&tokenize(text)) {
        return LocalIntent::CancelJob;
    }
    classify_local(text)
}

/// True when the words, in full, equal one cancel phrase.
#[allow(dead_code)] // M3: wired in S3b
fn is_cancel_job_utterance(words: &[Word]) -> bool {
    CANCEL_JOB_PHRASES
        .iter()
        .any(|phrase| words.len() == phrase.len() && match_prefix(words, phrase).is_some())
}

/// Stage 1 roles: whole-utterance phrases that end an active role (compared on word keys).
const CLEAR_ROLE_PHRASES: &[&[&str]] = &[
    &["back", "to", "normal"],
    &["stop", "acting"],
    &["stop", "the", "role"],
    &["drop", "the", "role"],
    &["end", "the", "role"],
];

/// Stage 1 roles: `classify_local_with`, preceded by the role clear phrases when a role is active.
/// Without an active role the result is exactly `classify_local_with`.
pub fn classify_local_in_role(
    text: &str,
    cancel_job_enabled: bool,
    role_active: bool,
) -> LocalIntent {
    if role_active && is_clear_role_utterance(&tokenize(text)) {
        return LocalIntent::ClearRole;
    }
    classify_local_with(text, cancel_job_enabled)
}

/// True when the words, in full, equal one role clear phrase.
fn is_clear_role_utterance(words: &[Word]) -> bool {
    CLEAR_ROLE_PHRASES
        .iter()
        .any(|phrase| words.len() == phrase.len() && match_prefix(words, phrase).is_some())
}

/// Classify one utterance. See DESIGN.md 3.1 for the exact word model.
pub fn classify_local(text: &str) -> LocalIntent {
    let words = tokenize(text);

    if END_PHRASES
        .iter()
        .any(|phrase| words.len() == phrase.len() && match_prefix(&words, phrase).is_some())
    {
        return LocalIntent::End;
    }

    for pattern in DICTATE_PREFIXES {
        if let Some(end) = match_prefix(&words, pattern) {
            let rest = text.get(end..).unwrap_or("");
            let remainder = rest.trim_start_matches(|c: char| {
                c.is_whitespace() || matches!(c, ':' | ',' | '-' | '.')
            });
            return if remainder.is_empty() {
                LocalIntent::None
            } else {
                LocalIntent::Dictate(remainder.to_string())
            };
        }
    }

    LocalIntent::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(s: &str) -> LocalIntent {
        LocalIntent::Dictate(s.to_string())
    }

    #[test]
    fn end_phrases_match_whole_utterance() {
        for p in [
            "stop",
            "Stop.",
            "STOP LISTENING",
            "stop conversation",
            "End conversation!",
            "goodbye",
            "Good bye.",
            "bye",
            "that's all",
            "That\u{2019}s all.",
            "thats all",
            "that is all",
            "we're done",
            "we are done",
            "I'm done",
            "i am done",
            "never mind",
            "Nevermind.",
            "exit",
            "  stop  ",
        ] {
            assert_eq!(classify_local(p), LocalIntent::End, "phrase: {p:?}");
        }
    }

    #[test]
    fn end_phrase_inside_a_longer_request_is_not_end() {
        for p in [
            "stop the music",
            "please stop",
            "bye bye now",
            "stop listening to jazz",
            "exit the building plan",
            "never mind the weather",
        ] {
            assert_eq!(classify_local(p), LocalIntent::None, "phrase: {p:?}");
        }
    }

    #[test]
    fn dictate_prefixes_extract_the_remainder() {
        assert_eq!(classify_local("dictate hello world"), dict("hello world"));
        assert_eq!(
            classify_local("Type this: Hello there."),
            dict("Hello there.")
        );
        assert_eq!(
            classify_local("type out the quarterly summary"),
            dict("the quarterly summary")
        );
        assert_eq!(
            classify_local("write this down, buy milk"),
            dict("buy milk")
        );
        assert_eq!(classify_local("Write down - call Sam"), dict("call Sam"));
        assert_eq!(classify_local("paste this, Dear Sir"), dict("Dear Sir"));
        assert_eq!(
            classify_local("paste thank you very much"),
            dict("thank you very much")
        );
    }

    #[test]
    fn dictate_prefers_the_longest_prefix() {
        // "write this down" must win over "write down" semantics and
        // "paste this" over "paste": the remainder must not start with "this".
        assert_eq!(classify_local("paste this is great"), dict("is great"));
        assert_eq!(classify_local("write this down now"), dict("now"));
    }

    #[test]
    fn dictate_preserves_original_casing_unicode_and_newlines() {
        assert_eq!(
            classify_local("DICTATE Caf\u{e9} \u{4f60}\u{597d} NASA"),
            dict("Caf\u{e9} \u{4f60}\u{597d} NASA")
        );
        assert_eq!(
            classify_local("dictate line one\nline two"),
            dict("line one\nline two")
        );
        assert_eq!(
            classify_local("Dictate:   spaced   out"),
            dict("spaced   out")
        );
    }

    #[test]
    fn dictate_without_a_remainder_is_not_dictation() {
        for p in [
            "dictate",
            "paste",
            "type this",
            "write down",
            "paste this",
            "dictate .",
            "dictate: -",
            "",
        ] {
            assert_eq!(classify_local(p), LocalIntent::None, "phrase: {p:?}");
        }
    }

    #[test]
    fn clear_role_phrases_match_only_while_a_role_is_active() {
        for p in [
            "back to normal",
            "Stop acting.",
            "stop the role",
            "drop the role",
            "End the role!",
        ] {
            assert_eq!(
                classify_local_in_role(p, false, true),
                LocalIntent::ClearRole,
                "phrase: {p:?}"
            );
            assert_eq!(
                classify_local_in_role(p, false, false),
                classify_local(p),
                "no role: {p:?}"
            );
        }
    }

    #[test]
    fn clear_role_needs_the_whole_utterance() {
        for p in ["I am back to normal", "back to normal please", "stop"] {
            assert_ne!(
                classify_local_in_role(p, false, true),
                LocalIntent::ClearRole,
                "phrase: {p:?}"
            );
        }
    }

    #[test]
    fn bare_type_is_not_a_dictation_prefix() {
        assert_eq!(classify_local("type of music is jazz"), LocalIntent::None);
        assert_eq!(classify_local("type hello"), LocalIntent::None);
    }

    #[test]
    fn prefix_requires_a_word_boundary() {
        assert_eq!(classify_local("pasted text"), LocalIntent::None);
        assert_eq!(classify_local("dictation is fun"), LocalIntent::None);
        assert_eq!(classify_local("pastel colors"), LocalIntent::None);
    }

    #[test]
    fn ordinary_speech_is_none() {
        assert_eq!(classify_local("what's the weather like"), LocalIntent::None);
        assert_eq!(classify_local("open Safari"), LocalIntent::None);
        assert_eq!(classify_local("   "), LocalIntent::None);
    }
}

#[cfg(test)]
#[path = "converse_local_cancel_tests.rs"]
mod converse_local_cancel_tests;
