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

const SINGLE_FILLERS: [&str; 11] = [
    "please", "hey", "hi", "vox", "okay", "ok", "so", "and", "now", "then", "lets",
];

/// Multi-word fillers; must be tried before single-word fillers.
const MULTI_FILLERS: [&[&str]; 7] = [
    &["can", "you"],
    &["could", "you"],
    &["would", "you"],
    &["will", "you"],
    &["i", "want", "you", "to"],
    &["i", "need", "you", "to"],
    &["go", "ahead", "and"],
];

const QUESTION_WORDS: [&str; 21] = [
    "what",
    "whats",
    "why",
    "how",
    "when",
    "where",
    "who",
    "which",
    "is",
    "are",
    "do",
    "does",
    "did",
    "should",
    "explain",
    "tell",
    "summarize",
    "summarise",
    "describe",
    "define",
    "compare",
];

const REVISE_WORDS: [&str; 11] = [
    "rewrite", "rephrase", "revise", "reword", "shorten", "lengthen", "edit", "fix", "simplify",
    "polish", "tighten",
];

const DRAFT_WORDS: [&str; 3] = ["write", "draft", "compose"];

const ACTION_WORDS: [&str; 22] = [
    "open", "launch", "start", "play", "pause", "resume", "switch", "search", "find", "show", "go",
    "create", "run", "close", "quit", "mute", "unmute", "lock", "save", "copy", "paste",
    "increase",
];

/// Lowercase alphanumeric words; apostrophes are dropped (so "don't" -> "dont").
fn tokenize(utterance: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in utterance.chars() {
        if c == '\'' || c == '\u{2019}' {
            continue;
        }
        if c.is_alphanumeric() {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn starts_with_words(tokens: &[String], words: &[&str]) -> bool {
    tokens.len() >= words.len() && words.iter().zip(tokens).all(|(w, t)| *w == t.as_str())
}

/// Tokens remaining after leading fillers are skipped. Each loop iteration consumes at
/// least one token, so this terminates.
fn skip_fillers(tokens: &[String]) -> &[String] {
    let mut rest = tokens;
    'outer: loop {
        for phrase in MULTI_FILLERS {
            if starts_with_words(rest, phrase) {
                rest = rest.get(phrase.len()..).unwrap_or(&[]);
                continue 'outer;
            }
        }
        match rest.first() {
            Some(t) if SINGLE_FILLERS.contains(&t.as_str()) => {
                rest = rest.get(1..).unwrap_or(&[]);
            }
            _ => return rest,
        }
    }
}

/// See the module docs for the exact rules.
pub fn classify(utterance: &str, has_draft: bool) -> AckKind {
    let tokens = tokenize(utterance);
    let content = skip_fillers(&tokens);
    let first = content.first().map(String::as_str);
    let second = content.get(1).map(String::as_str);

    let ends_with_question = utterance.trim_end().ends_with('?');
    if ends_with_question || first.is_some_and(|w| QUESTION_WORDS.contains(&w)) {
        return AckKind::Question;
    }
    if has_draft {
        let revise = first.is_some_and(|w| {
            REVISE_WORDS.contains(&w)
                || (matches!(w, "make" | "change")
                    && matches!(second, Some("it" | "that" | "this" | "the")))
        });
        if revise {
            return AckKind::Revise;
        }
    }
    if first.is_some_and(|w| DRAFT_WORDS.contains(&w)) {
        return AckKind::Draft;
    }
    if first.is_some_and(|w| ACTION_WORDS.contains(&w)) {
        return AckKind::Action;
    }
    AckKind::Other
}

/// `idx = utterance.chars().count() % pool.len()`; when `pool[idx] == last` take
/// `(idx + 1) % pool.len()`. Deterministic. `text`/`after` come from the classified kind.
pub fn choose(utterance: &str, has_draft: bool, last: Option<&str>) -> Ack {
    let kind = classify(utterance, has_draft);
    let items = pool(kind);
    let len = items.len();
    // Every pool is a non-empty const array, so `len` is never 0 here.
    let mut idx = if len == 0 {
        0
    } else {
        utterance.chars().count() % len
    };
    if len > 0 && items.get(idx).copied() == last {
        idx = (idx + 1) % len;
    }
    Ack {
        text: items.get(idx).copied().unwrap_or(""),
        after: after_for(kind),
        kind,
    }
}

#[cfg(test)]
#[path = "converse_ack_tests.rs"]
mod tests;
