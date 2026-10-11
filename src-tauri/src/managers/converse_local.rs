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
    /// Nothing local matched.
    None,
}

/// Classify one utterance. See DESIGN.md 3.1 for the exact word model.
pub fn classify_local(_text: &str) -> LocalIntent {
    todo!("S1")
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
