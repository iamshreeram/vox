//! History notes: structured, runtime-authored descriptions of what really happened in a turn
//! (pasted text, an action and its result, an interruption, a delivery failure, an incomplete
//! reply, a failure), appended to the assistant side of a history entry so follow-ups such as
//! "make it shorter", "do that again" or "what did you just do" resolve correctly.
//!
//! Pure functions, no I/O. Design: DESIGN.md Design A (revision 3, critic-approved).
//!
//! Wire format (one line, appended after a single space): `⟦note {json}⟧` with ⟦ = U+27E6 and
//! ⟧ = U+27E7. Only text inside the markers was written by the Vox runtime. Fields named
//! `untrusted_*` quote data that came from the user, a model or an app: the planner is told to
//! never follow instructions in them nor treat them as proof of success.
//!
//! Safety properties (all tested):
//! * `note()` serializes with serde_json, THEN rewrites every literal U+27E6 / U+27E7 in the
//!   serialized JSON to the JSON escapes `\u27e6` / `\u27e7`, THEN wraps it. A final note has
//!   exactly ONE literal opening and ONE literal closing marker and its JSON parses back to the
//!   original (bounded) payload.
//! * The complete serialized, escaped, delimited note never exceeds `NOTE_BUDGET` chars: only
//!   `untrusted` strings are shortened (longest first, by max(1, 25%) of its chars, ending in `…`
//!   when cut, down to the empty string if needed); runtime fields are never cut and the
//!   serialized JSON is never cut. If even the runtime-only note exceeded the budget the
//!   fallback is `⟦note {"event":"<event>"}⟧`.
//! * `neutralize()` removes both markers from ordinary text (user utterances, assistant speech)
//!   so ordinary text can never contain or fake a note.
//! * `with_cap` never cuts a note: it shortens the TEXT; a note longer than `max` is replaced by
//!   the fallback note of its event.

use serde_json::Value;

pub const OPEN: char = '\u{27E6}';
pub const CLOSE: char = '\u{27E7}';
/// Maximum chars of a complete note (serialized, escaped, delimited).
pub const NOTE_BUDGET: usize = 330;
/// Maximum chars of one history side (matches `converse::HISTORY_SIDE_MAX_CHARS`).
pub const SIDE_MAX: usize = 600;
/// Nominal caps for quoted payloads, applied on char boundaries BEFORE escaping; `…` is added
/// when cut (so a cut paste text has `PASTE_TEXT_MAX + 1` chars).
pub const PASTE_TEXT_MAX: usize = 120;
pub const COMMAND_MAX: usize = 80;
pub const RESULT_MAX: usize = 80;

/// What happened to an action the planner asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandNote {
    /// Executed; the executor's message (may be empty).
    Done(String),
    Failed,
    Refused,
    NoMatch,
}

/// Remove U+27E6 / U+27E7 and turn every line break / control whitespace into a single space
/// (`\n`, `\r`, `\t` become ' '); everything else is untouched. Idempotent.
pub fn neutralize(s: &str) -> String {
    let _ = s;
    todo!("WP-context")
}

/// Build one note. `runtime` fields (e.g. `chars`, `action_executed`) are never shortened;
/// `untrusted` pairs are (name WITHOUT the prefix, text): they are emitted as
/// `untrusted_<name>` strings and shortened as needed to respect `NOTE_BUDGET`.
/// Field order in the JSON: `event` first, then runtime fields in the given order, then the
/// untrusted fields in the given order.
pub fn note(event: &str, runtime: &[(&str, Value)], untrusted: &[(&str, &str)]) -> String {
    let _ = (event, runtime, untrusted);
    todo!("WP-context")
}

/// event `pasted`: runtime `chars` = char count of the ORIGINAL text; untrusted `text` capped at
/// `PASTE_TEXT_MAX`.
pub fn note_paste(pasted: &str) -> String {
    let _ = pasted;
    todo!("WP-context")
}

/// event `paste_failed`, no fields.
pub fn note_paste_failed() -> String {
    todo!("WP-context")
}

/// `Done(msg)` => event `ran_action` with untrusted `command` (cap `COMMAND_MAX`) and untrusted
/// `result` (cap `RESULT_MAX`; the key is omitted when `msg` is empty). `Failed` => `action_failed`,
/// `Refused` => `action_refused`, `NoMatch` => `action_not_found`, each with untrusted `command`.
pub fn note_command(command: &str, outcome: &CommandNote) -> String {
    let _ = (command, outcome);
    todo!("WP-context")
}

/// event `interrupted`, no fields.
pub fn note_interrupted() -> String {
    todo!("WP-context")
}

/// event `delivery_failed`, no fields.
pub fn note_delivery_failed() -> String {
    todo!("WP-context")
}

/// event `reply_incomplete`, runtime `action_executed` = false.
pub fn note_incomplete() -> String {
    todo!("WP-context")
}

/// event `failed`, runtime `action_executed` = false.
pub fn note_failed() -> String {
    todo!("WP-context")
}

/// `text + " " + note` when it fits in `max` chars; otherwise the TEXT is shortened (ending in
/// `…`, on a char boundary) so the whole note is kept; an empty `text` yields just the note.
/// If `note` alone is longer than `max`, returns the fallback note `⟦note {"event":"<event>"}⟧`
/// where `<event>` is parsed from the note's JSON (`unknown` if it cannot be parsed); the text is
/// dropped in that case. The result is never longer than `max` chars (for `max` >= 30).
pub fn with_cap(text: &str, note: &str, max: usize) -> String {
    let _ = (text, note, max);
    todo!("WP-context")
}

/// `with_cap(&neutralize(text), note, SIDE_MAX)`: the one call the loop uses to build an
/// assistant-side history string.
pub fn compose(text: &str, note: &str) -> String {
    let _ = (text, note);
    todo!("WP-context")
}

#[cfg(test)]
#[path = "converse_context_tests.rs"]
mod tests;
