//! Early speech from a streaming plan (design R9 stage 3).
//!
//! SKELETON: bodies are `todo!("WP-stream")`; tests in `converse_stream_tests.rs` are the spec.
//!
//! `PlanStreamer` consumes the raw text deltas of the model's single JSON plan object and
//! releases COMPLETED SENTENCES of the `speech` value as soon as they are safe to speak.
//! It is deliberately conservative: anything unexpected disables it for the rest of the turn
//! (the loop then falls back to speaking the final, fully parsed plan). Nothing here is
//! trusted for ACTIONS: the final plan is still parsed by `converse_plan::parse_plan`, and
//! `reconcile` decides whether the early speech is consistent with it.
//!
//! Grammar accepted for early speech (everything else => disabled):
//!   optional whitespace, `{`, first member `"kind"` with a string value that is
//!   `answer` or `clarify` (ASCII case-insensitive), then members in any order. String
//!   values may be streamed; `"speech"` is the spoken text, `"emotion"` is mapped with
//!   `Emotion::from_label`; other keys with string/number/true/false/null values are skipped;
//!   an object/array value, a duplicate `kind`/`speech` key, or an invalid string escape
//!   disables the streamer. Text after the closing `}` is ignored.
//!
//! Sentence rule: decoded speech characters accumulate; a boundary is `.`, `!`, `?` or `…`
//! followed by a whitespace character that has ARRIVED (so `3.5` never splits and a
//! trailing sentence that ends at the closing quote is left for the final plan). A
//! candidate shorter than `MIN_SENTENCE_CHARS` (after canonicalization) keeps accumulating
//! into the next sentence. Total early text is capped at `MAX_EARLY_CHARS`; later sentences
//! are simply not released (they become the remainder).

use crate::managers::converse_plan::{PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;

pub const MIN_SENTENCE_CHARS: usize = 20;
pub const MAX_EARLY_CHARS: usize = 400;

/// Canonical spoken form: the same sanitizer every speech path uses
/// (`tts::sanitize_speech_text`: brackets/control chars removed, whitespace collapsed, trimmed);
/// empty string when nothing speakable remains.
pub fn canon(_text: &str) -> String {
    todo!("WP-stream")
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlySentence {
    /// Already canonical (`canon`) and non-empty.
    pub text: String,
    /// The emotion seen so far in the stream, else `Neutral`.
    pub emotion: Emotion,
}

#[derive(Debug, Default)]
pub struct PlanStreamer {
    // implementation-defined state
}

impl PlanStreamer {
    pub fn new() -> Self {
        todo!("WP-stream")
    }

    /// Feed one text delta (any split, including inside escapes/UTF-16 surrogate pairs);
    /// returns the sentences that became safe to speak because of this delta (usually empty).
    /// Never panics; after `is_disabled()` it always returns an empty Vec.
    pub fn push(&mut self, _delta: &str) -> Vec<EarlySentence> {
        todo!("WP-stream")
    }

    /// Emotion seen so far (from an `"emotion"` member), if any.
    pub fn emotion(&self) -> Option<Emotion> {
        todo!("WP-stream")
    }

    /// True once the stream was rejected for early speech (wrong shape/kind, duplicate key,
    /// bad escape, ...).
    pub fn is_disabled(&self) -> bool {
        todo!("WP-stream")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconcile {
    /// Speak `text` (may be empty => nothing left to speak) AFTER whatever was already spoken
    /// early; the turn is an answer/clarify/end turn.
    Remainder { text: String },
    /// Nothing was spoken early and the plan is an action plan (paste/command): run the legacy
    /// path unchanged.
    Legacy,
    /// Early speech exists but contradicts the final plan: stop speaking, execute nothing.
    Abort,
}

/// Decide how the final plan relates to the sentences already spoken early (`spoken` holds
/// canonical sentences in order, WITHOUT any acknowledgement phrase).
/// * `spoken` empty: Answer/Clarify/End => `Remainder{canon(speech)}`; Paste/Command => `Legacy`.
/// * `spoken` non-empty: kind must be Answer or Clarify, else `Abort`; with
///   `joined = spoken.join(" ")` and `full = canon(plan.speech)`: `full == joined` =>
///   `Remainder{""}`; `full` starts with `joined` and the next char is whitespace =>
///   `Remainder{rest.trim()}`; otherwise `Abort`.
pub fn reconcile(_plan: &TurnPlan, _spoken: &[String]) -> Reconcile {
    todo!("WP-stream")
}

#[cfg(test)]
#[path = "converse_stream_tests.rs"]
mod tests;
