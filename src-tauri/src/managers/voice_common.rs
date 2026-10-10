//! Shared helpers for whole-utterance voice commands (media controls and
//! custom voice shortcuts).
//!
//! This file is intentionally byte-identical in every feature branch that
//! uses it, so git merges the "both added" case without a conflict. Do not
//! edit it in only one branch.

#![allow(dead_code)]

/// Normalizes a transcript for whole-utterance phrase comparison.
///
/// Contract (shared by media controls and custom shortcuts):
/// - lowercase;
/// - trim surrounding whitespace;
/// - strip a leading token `please` ONLY when it is followed by whitespace
///   (so `pleasepause` and `please,` are left alone);
/// - strip trailing characters in `.` `!` `?` `,` `;` (and whitespace between
///   them) -- no other punctuation is ever stripped;
/// - collapse runs of internal whitespace to single spaces.
pub fn normalize_phrase(input: &str) -> String {
    let lowered = input.to_lowercase();
    let mut text = lowered.trim();
    if let Some(rest) = text.strip_prefix("please") {
        if rest.chars().next().is_some_and(char::is_whitespace) {
            text = rest.trim_start();
        }
    }
    let text = text
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '.' | '!' | '?' | ',' | ';'));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the post-transcription hook must do after a voice action matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceHookOutcome {
    /// Do not paste the transcript into the focused app.
    pub skip_paste: bool,
    /// Hand the transcript to the agent bridge.
    pub escalate_to_agent: bool,
}

impl VoiceHookOutcome {
    /// A matched voice action -- whether it succeeded or failed -- consumes
    /// the utterance: it is never pasted and never escalated to the agent.
    pub const HANDLED: Self = Self {
        skip_paste: true,
        escalate_to_agent: false,
    };
}

/// Event the hook should emit after a voice action ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    /// Emit `voice-command-executed` with this description.
    Executed(String),
    /// Emit `voice-command-error` with this message.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_trims_and_collapses_whitespace() {
        assert_eq!(normalize_phrase("  Pause   Music  "), "pause music");
        assert_eq!(normalize_phrase("NEXT\tTRACK"), "next track");
        assert_eq!(normalize_phrase("next\n\ntrack"), "next track");
    }

    #[test]
    fn strips_only_trailing_sentence_punctuation() {
        assert_eq!(normalize_phrase("pause."), "pause");
        assert_eq!(normalize_phrase("pause!?"), "pause");
        assert_eq!(normalize_phrase("pause , ."), "pause");
        assert_eq!(normalize_phrase("pause;"), "pause");
        // Interior and other punctuation is preserved.
        assert_eq!(normalize_phrase("volume 50%"), "volume 50%");
        assert_eq!(normalize_phrase("volume -5"), "volume -5");
        assert_eq!(normalize_phrase("volume 5.5"), "volume 5.5");
        assert_eq!(normalize_phrase("what's up"), "what's up");
        assert_eq!(normalize_phrase("(pause)"), "(pause)");
    }

    #[test]
    fn please_is_stripped_only_as_a_whole_leading_token() {
        assert_eq!(normalize_phrase("please pause"), "pause");
        assert_eq!(normalize_phrase("Please   Pause"), "pause");
        assert_eq!(normalize_phrase("please\tpause"), "pause");
        assert_eq!(normalize_phrase("pleasepause"), "pleasepause");
        assert_eq!(normalize_phrase("please, pause"), "please, pause");
        assert_eq!(normalize_phrase("please"), "please");
        assert_eq!(normalize_phrase("pause please"), "pause please");
    }

    #[test]
    fn empty_and_whitespace_normalize_to_empty() {
        assert_eq!(normalize_phrase(""), "");
        assert_eq!(normalize_phrase("   \t\n"), "");
        assert_eq!(normalize_phrase("..."), "");
    }

    #[test]
    fn unicode_and_long_input_do_not_panic() {
        assert_eq!(normalize_phrase("İSTANBUL"), "i\u{307}stanbul");
        let _ = normalize_phrase(&"é".repeat(10_000));
        let _ = normalize_phrase(&"please ".repeat(5_000));
        assert_eq!(normalize_phrase("暂停。"), "暂停。");
    }

    #[test]
    fn handled_outcome_never_pastes_or_escalates() {
        assert!(VoiceHookOutcome::HANDLED.skip_paste);
        assert!(!VoiceHookOutcome::HANDLED.escalate_to_agent);
    }
}
