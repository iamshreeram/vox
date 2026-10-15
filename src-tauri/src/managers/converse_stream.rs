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
//!   optional whitespace, `{`, first member `\"kind\"` with a string value that is
//!   `answer` or `clarify` (ASCII case-insensitive), then members in any order. String
//!   values may be streamed; `\"speech\"` is the spoken text, `\"emotion\"` is mapped with
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

/// Longest member key that can still match `kind`, `speech` or `emotion`.
const MAX_KEY_CHARS: usize = 32;
/// Longest unquoted literal value (number/true/false/null) we accept.
const MAX_LITERAL_CHARS: usize = 64;

/// Canonical spoken form: the same sanitizer every speech path uses
/// (`tts::sanitize_speech_text`: brackets/control chars removed, whitespace collapsed, trimmed);
/// empty string when nothing speakable remains.
pub fn canon(text: &str) -> String {
    crate::managers::tts::sanitize_speech_text(text).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
pub struct EarlySentence {
    /// Already canonical (`canon`) and non-empty.
    pub text: String,
    /// The emotion seen so far in the stream, else `Neutral`.
    pub emotion: Emotion,
}

/// Lexical state of the streamer.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum St {
    /// Before the opening `{`.
    #[default]
    Start,
    /// Expecting a member key (`"`), right after `{` or `,`.
    ExpectKey,
    /// Inside a member key string.
    Key,
    /// After a member key, expecting `:`.
    AfterKey,
    /// Expecting a member value.
    Value,
    /// Inside a string value.
    Str,
    /// Inside an unquoted literal value.
    Literal,
    /// After a member value, expecting `,` or `}`.
    AfterValue,
    /// After the closing `}`: everything is ignored.
    Closed,
    /// Rejected: inert for the rest of the turn.
    Disabled,
}

/// Which member the current value belongs to.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Target {
    Kind,
    Speech,
    Emotion,
    #[default]
    Other,
}

/// JSON string escape state, carried across `push` calls.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Esc {
    #[default]
    Plain,
    Backslash,
    /// Reading the four hex digits of a `\uXXXX`; `high` is the pending high surrogate
    /// when this unit is the low half of a surrogate pair.
    Hex {
        value: u32,
        digits: u8,
        high: Option<u32>,
    },
    /// A high surrogate was read; a `\u` low surrogate must follow.
    NeedBackslash {
        high: u32,
    },
    NeedU {
        high: u32,
    },
}

/// Result of feeding one character to the string-literal decoder.
enum Piece {
    More,
    Char(char),
    Close,
    Bad,
}

#[derive(Debug, Default)]
pub struct PlanStreamer {
    state: St,
    esc: Esc,
    /// Current member key (capped at `MAX_KEY_CHARS`).
    key: String,
    /// Member the current value belongs to.
    target: Target,
    /// Number of members seen so far.
    members: usize,
    kind_seen: bool,
    speech_seen: bool,
    /// Decoded text of the current `kind` or `emotion` string value.
    value_buf: String,
    /// Current unquoted literal value.
    literal: String,
    /// Canonical-ish speech collected since the last release (collapsed as `sanitize` would).
    pending: String,
    /// Whether the previous decoded speech character was a sentence terminator.
    prev_term: bool,
    /// Set once the early cap blocked a release; nothing further is released.
    stopped: bool,
    spoken_chars: usize,
    spoken_count: usize,
    emotion: Option<Emotion>,
}

impl PlanStreamer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one text delta (any split, including inside escapes/UTF-16 surrogate pairs);
    /// returns the sentences that became safe to speak because of this delta (usually empty).
    /// Never panics; after `is_disabled()` it always returns an empty Vec.
    pub fn push(&mut self, delta: &str) -> Vec<EarlySentence> {
        let mut out = Vec::new();
        if self.state == St::Disabled {
            return out;
        }
        for c in delta.chars() {
            if !self.step(c, &mut out) {
                self.state = St::Disabled;
                break;
            }
        }
        out
    }

    /// Emotion seen so far (from an `\"emotion\"` member), if any.
    pub fn emotion(&self) -> Option<Emotion> {
        self.emotion
    }

    /// True once the stream was rejected for early speech (wrong shape/kind, duplicate key,
    /// bad escape, ...).
    pub fn is_disabled(&self) -> bool {
        self.state == St::Disabled
    }

    /// Advance by one character. `false` means the stream must be rejected.
    fn step(&mut self, c: char, out: &mut Vec<EarlySentence>) -> bool {
        match self.state {
            St::Start => {
                if c.is_whitespace() {
                    true
                } else if c == '{' {
                    self.state = St::ExpectKey;
                    true
                } else {
                    false
                }
            }
            St::ExpectKey => {
                if c.is_whitespace() {
                    true
                } else if c == '"' {
                    self.key.clear();
                    self.esc = Esc::Plain;
                    self.state = St::Key;
                    true
                } else {
                    false
                }
            }
            St::Key => match self.string_piece(c) {
                Piece::More => true,
                Piece::Bad => false,
                Piece::Char(ch) => {
                    if self.key.chars().count() < MAX_KEY_CHARS {
                        self.key.push(ch);
                    }
                    true
                }
                Piece::Close => self.close_key(),
            },
            St::AfterKey => {
                if c.is_whitespace() {
                    true
                } else if c == ':' {
                    self.state = St::Value;
                    true
                } else {
                    false
                }
            }
            St::Value => self.value_start(c),
            St::Str => match self.string_piece(c) {
                Piece::More => true,
                Piece::Bad => false,
                Piece::Char(ch) => {
                    self.emit(ch, out);
                    true
                }
                Piece::Close => self.close_string(out),
            },
            St::Literal => {
                if c.is_whitespace() || c == ',' || c == '}' {
                    if self.literal.is_empty() {
                        return false;
                    }
                    self.literal.clear();
                    self.state = St::AfterValue;
                    self.step(c, out)
                } else if is_literal_char(c) && self.literal.chars().count() < MAX_LITERAL_CHARS {
                    self.literal.push(c);
                    true
                } else {
                    false
                }
            }
            St::AfterValue => {
                if c.is_whitespace() {
                    true
                } else if c == ',' {
                    self.state = St::ExpectKey;
                    true
                } else if c == '}' {
                    self.state = St::Closed;
                    true
                } else {
                    false
                }
            }
            St::Closed => true,
            St::Disabled => false,
        }
    }

    /// First character of a member value.
    fn value_start(&mut self, c: char) -> bool {
        if c.is_whitespace() {
            return true;
        }
        match c {
            '"' => {
                self.value_buf.clear();
                self.esc = Esc::Plain;
                self.state = St::Str;
                true
            }
            '{' | '[' | '}' | ',' => false,
            _ => {
                // Non-string kind/speech values are rejected; other literals are skipped.
                if matches!(self.target, Target::Kind | Target::Speech) || !is_literal_char(c) {
                    return false;
                }
                self.literal.clear();
                self.literal.push(c);
                self.state = St::Literal;
                true
            }
        }
    }

    /// Decode one character of a JSON string (escapes, `\uXXXX`, surrogate pairs).
    fn string_piece(&mut self, c: char) -> Piece {
        let esc = self.esc;
        match esc {
            Esc::Plain => match c {
                '"' => Piece::Close,
                '\\' => {
                    self.esc = Esc::Backslash;
                    Piece::More
                }
                _ => Piece::Char(c),
            },
            Esc::Backslash => {
                self.esc = Esc::Plain;
                let decoded = match c {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        self.esc = Esc::Hex {
                            value: 0,
                            digits: 0,
                            high: None,
                        };
                        return Piece::More;
                    }
                    _ => return Piece::Bad,
                };
                Piece::Char(decoded)
            }
            Esc::Hex {
                value,
                digits,
                high,
            } => {
                let Some(d) = c.to_digit(16) else {
                    return Piece::Bad;
                };
                let value = value * 16 + d;
                if digits < 3 {
                    self.esc = Esc::Hex {
                        value,
                        digits: digits + 1,
                        high,
                    };
                    return Piece::More;
                }
                self.esc = Esc::Plain;
                self.finish_unit(value, high)
            }
            Esc::NeedBackslash { high } => {
                if c == '\\' {
                    self.esc = Esc::NeedU { high };
                    Piece::More
                } else {
                    Piece::Bad
                }
            }
            Esc::NeedU { high } => {
                if c == 'u' {
                    self.esc = Esc::Hex {
                        value: 0,
                        digits: 0,
                        high: Some(high),
                    };
                    Piece::More
                } else {
                    Piece::Bad
                }
            }
        }
    }

    /// A complete `\uXXXX` code unit (`high` = pending high surrogate, if any).
    fn finish_unit(&mut self, unit: u32, high: Option<u32>) -> Piece {
        match high {
            Some(h) => {
                if (0xDC00..=0xDFFF).contains(&unit) {
                    let code =
                        0x1_0000 + (h.wrapping_sub(0xD800) << 10) + unit.wrapping_sub(0xDC00);
                    char::from_u32(code).map_or(Piece::Bad, Piece::Char)
                } else {
                    Piece::Bad
                }
            }
            None => {
                if (0xD800..=0xDBFF).contains(&unit) {
                    self.esc = Esc::NeedBackslash { high: unit };
                    Piece::More
                } else if (0xDC00..=0xDFFF).contains(&unit) {
                    Piece::Bad
                } else {
                    char::from_u32(unit).map_or(Piece::Bad, Piece::Char)
                }
            }
        }
    }

    /// The closing quote of a member key.
    fn close_key(&mut self) -> bool {
        let target = match self.key.as_str() {
            "kind" => Target::Kind,
            "speech" => Target::Speech,
            "emotion" => Target::Emotion,
            _ => Target::Other,
        };
        // `kind` must be the first member, and only once.
        if self.members == 0 && target != Target::Kind {
            return false;
        }
        match target {
            Target::Kind => {
                if self.kind_seen {
                    return false;
                }
                self.kind_seen = true;
            }
            Target::Speech => {
                if self.speech_seen {
                    return false;
                }
                self.speech_seen = true;
            }
            Target::Emotion | Target::Other => {}
        }
        self.members += 1;
        self.target = target;
        self.state = St::AfterKey;
        true
    }

    /// The closing quote of a string value.
    fn close_string(&mut self, _out: &mut Vec<EarlySentence>) -> bool {
        match self.target {
            Target::Kind => {
                let ok = self.value_buf.eq_ignore_ascii_case("answer")
                    || self.value_buf.eq_ignore_ascii_case("clarify");
                self.value_buf.clear();
                if !ok {
                    return false;
                }
            }
            Target::Emotion => {
                self.emotion = Some(Emotion::from_label(&self.value_buf));
                self.value_buf.clear();
            }
            // The closing quote of speech releases nothing: the tail is for the final plan.
            Target::Speech | Target::Other => {}
        }
        self.state = St::AfterValue;
        true
    }

    /// One decoded character of a string value.
    fn emit(&mut self, ch: char, out: &mut Vec<EarlySentence>) {
        match self.target {
            Target::Kind | Target::Emotion => self.value_buf.push(ch),
            Target::Speech => self.feed_speech(ch, out),
            Target::Other => {}
        }
    }

    /// One decoded character of the speech value.
    fn feed_speech(&mut self, c: char, out: &mut Vec<EarlySentence>) {
        let was_term = self.prev_term;
        self.prev_term = matches!(c, '.' | '!' | '?' | '…');
        if self.stopped {
            return;
        }
        if c.is_whitespace() && was_term {
            self.release_candidate(out);
            if self.stopped {
                return;
            }
        }
        let mapped = match c {
            '[' | ']' | '\n' | '\r' | '\t' => ' ',
            c if c.is_control() => return,
            c => c,
        };
        if mapped.is_whitespace() {
            if !self.pending.is_empty() && !self.pending.ends_with(' ') {
                self.pending.push(' ');
            }
        } else {
            self.pending.push(mapped);
        }
        if self.pending.chars().count() > MAX_EARLY_CHARS {
            // Longer than the whole early budget: never release from here on.
            self.stopped = true;
            self.pending.clear();
        }
    }

    /// Called when whitespace arrives right after a sentence terminator.
    fn release_candidate(&mut self, out: &mut Vec<EarlySentence>) {
        let candidate = std::mem::take(&mut self.pending);
        let text = canon(&candidate);
        let n = text.chars().count();
        if n < MIN_SENTENCE_CHARS {
            // Too short on its own: keep accumulating into the next sentence.
            self.pending = candidate;
            return;
        }
        let join = usize::from(self.spoken_count > 0);
        let total = self.spoken_chars + join + n;
        if total > MAX_EARLY_CHARS {
            self.stopped = true;
            return;
        }
        self.spoken_chars = total;
        self.spoken_count += 1;
        out.push(EarlySentence {
            text,
            emotion: self.emotion.unwrap_or(Emotion::Neutral),
        });
    }
}

fn is_literal_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '.')
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
pub fn reconcile(plan: &TurnPlan, spoken: &[String]) -> Reconcile {
    if spoken.is_empty() {
        return match plan.kind {
            // M3: Job plans are not produced yet (parsed in S3a, executed in S3b)
            PlanKind::Answer | PlanKind::Clarify | PlanKind::End | PlanKind::Job => {
                Reconcile::Remainder {
                    text: canon(&plan.speech),
                }
            }
            PlanKind::Paste | PlanKind::Command => Reconcile::Legacy,
        };
    }
    if !matches!(
        plan.kind,
        PlanKind::Answer | PlanKind::Clarify | PlanKind::Job
    ) {
        return Reconcile::Abort;
    }
    let joined = spoken.join(" ");
    let full = canon(&plan.speech);
    if full == joined {
        return Reconcile::Remainder {
            text: String::new(),
        };
    }
    if let Some(rest) = full.strip_prefix(joined.as_str()) {
        if matches!(rest.chars().next(), Some(ch) if ch.is_whitespace()) {
            return Reconcile::Remainder {
                text: rest.trim().to_string(),
            };
        }
    }
    Reconcile::Abort
}

#[cfg(test)]
#[path = "converse_stream_tests.rs"]
mod tests;
