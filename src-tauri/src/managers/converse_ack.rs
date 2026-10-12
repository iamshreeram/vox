//! Contextual spoken acknowledgement: a short, varied phrase chosen from what the user
//! just said, instead of one static "One moment.".
//!
//! Pure functions, no I/O. Design: DESIGN.md Design B (revision 3, critic-approved).
//!
//! Tokens are lowercase words split on non-alphanumeric characters (apostrophes are
//! dropped, so "don't" becomes "dont"). Leading fillers are skipped: please, hey, hi, vox,
//! okay, ok, so, and, now, then, "can you", "could you", "would you", "will you",
//! "i want you to", "i need you to", "go ahead and", lets.
//!
//! Classification priority (QUESTION FIRST, so a question never gets a promise):
//! 1. `Question`: the trimmed utterance ends with '?', or the first non-filler token is one
//!    of what, whats, why, how, when, where, who, which, is, are, do, does, did, should,
//!    explain, tell, summarize, summarise, describe, define, compare.
//! 2. `Revise`: `has_draft`, not a question, and the first non-filler token is one of
//!    rewrite, rephrase, revise, reword, shorten, lengthen, edit, fix, simplify, polish,
//!    tighten, OR it is "make" / "change" followed by one of it, that, this, the.
//! 3. `Draft`: not a question and the first non-filler token is write, draft or compose.
//! 4. `Action`: not a question and the first non-filler token is one of open, launch,
//!    start, play, pause, resume, switch, search, find, show, go, create, run, close, quit,
//!    mute, unmute, lock, save, copy, paste, increase, decrease. (delete, send, call,
//!    remind, add, set, turn, take, text, email, reply, message, respond, tweet are
//!    deliberately NOT here: ambiguous or consequential.)
//! 5. `Other`.
//!
//! The delays are heuristics (earlier for kinds that cannot be spoken early, later for
//! questions whose first sentence usually arrives by itself).

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckKind {
    Revise,
    Draft,
    Action,
    Question,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub text: &'static str,
    pub after: Duration,
    pub kind: AckKind,
}

pub const REVISE_POOL: [&str; 2] = ["Sure, revising that.", "Okay, reworking it."];
pub const DRAFT_POOL: [&str; 2] = ["Okay, drafting that.", "Sure, writing that."];
pub const ACTION_POOL: [&str; 2] = ["On it.", "Okay, working on that."];
pub const QUESTION_POOL: [&str; 3] = ["Let me think.", "Good question.", "Let me check."];
pub const OTHER_POOL: [&str; 2] = ["One moment.", "Okay."];

/// Delay before the acknowledgement may be spoken (milliseconds).
pub const ACTION_AFTER_MS: u64 = 600;
pub const QUESTION_AFTER_MS: u64 = 1500;
pub const OTHER_AFTER_MS: u64 = 1000;

pub fn pool(kind: AckKind) -> &'static [&'static str] {
    match kind {
        AckKind::Revise => &REVISE_POOL,
        AckKind::Draft => &DRAFT_POOL,
        AckKind::Action => &ACTION_POOL,
        AckKind::Question => &QUESTION_POOL,
        AckKind::Other => &OTHER_POOL,
    }
}

pub fn after_for(kind: AckKind) -> Duration {
    Duration::from_millis(match kind {
        AckKind::Revise | AckKind::Draft | AckKind::Action => ACTION_AFTER_MS,
        AckKind::Question => QUESTION_AFTER_MS,
        AckKind::Other => OTHER_AFTER_MS,
    })
}

/// See the module docs for the exact rules.
pub fn classify(utterance: &str, has_draft: bool) -> AckKind {
    let _ = (utterance, has_draft);
    todo!("WP-ack")
}

/// `idx = utterance.chars().count() % pool.len()`; when `pool[idx] == last` take
/// `(idx + 1) % pool.len()`. Deterministic. `text`/`after` come from the classified kind.
pub fn choose(utterance: &str, has_draft: bool, last: Option<&str>) -> Ack {
    let _ = (utterance, has_draft, last);
    todo!("WP-ack")
}

#[cfg(test)]
#[path = "converse_ack_tests.rs"]
mod tests;
