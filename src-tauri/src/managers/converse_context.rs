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
    s.chars()
        .filter_map(|c| match c {
            OPEN | CLOSE => None,
            '\n' | '\r' | '\t' => Some(' '),
            other => Some(other),
        })
        .collect()
}

/// Build one note. `runtime` fields (e.g. `chars`, `action_executed`) are never shortened;
/// `untrusted` pairs are (name WITHOUT the prefix, text): they are emitted as
/// `untrusted_<name>` strings and shortened as needed to respect `NOTE_BUDGET`.
/// Field order in the JSON: `event` first, then runtime fields in the given order, then the
/// untrusted fields in the given order.
pub fn note(event: &str, runtime: &[(&str, Value)], untrusted: &[(&str, &str)]) -> String {
    let mut fields: Vec<(String, String)> = untrusted
        .iter()
        .map(|(name, text)| (format!("untrusted_{name}"), cap_chars(text, PRECAP_CHARS)))
        .collect();
    loop {
        let candidate = wrap_json(&render(event, runtime, &fields));
        if candidate.chars().count() <= NOTE_BUDGET {
            return candidate;
        }
        // Longest non-empty untrusted string (first one wins ties).
        let mut longest: Option<(usize, usize)> = None;
        for (i, (_, text)) in fields.iter().enumerate() {
            let len = text.chars().count();
            let better = match longest {
                None => len > 0,
                Some((_, best)) => len > best,
            };
            if better {
                longest = Some((i, len));
            }
        }
        match longest {
            None => return event_only_note(event),
            Some((i, len)) => {
                let cut = (len / 4).max(1);
                if let Some((_, text)) = fields.get_mut(i) {
                    *text = shrink_to(text, len - cut);
                }
            }
        }
    }
}

/// event `pasted`: runtime `chars` = char count of the ORIGINAL text; untrusted `text` capped at
/// `PASTE_TEXT_MAX`.
pub fn note_paste(pasted: &str) -> String {
    let original_chars = pasted.chars().count();
    let text = cap_chars(pasted, PASTE_TEXT_MAX);
    note(
        "pasted",
        &[("chars", Value::from(original_chars))],
        &[("text", text.as_str())],
    )
}

/// event `paste_failed`, no fields.
pub fn note_paste_failed() -> String {
    note("paste_failed", &[], &[])
}

/// `Done(msg)` => event `ran_action` with untrusted `command` (cap `COMMAND_MAX`) and untrusted
/// `result` (cap `RESULT_MAX`; the key is omitted when `msg` is empty). `Failed` => `action_failed`,
/// `Refused` => `action_refused`, `NoMatch` => `action_not_found`, each with untrusted `command`.
pub fn note_command(command: &str, outcome: &CommandNote) -> String {
    let command = cap_chars(command, COMMAND_MAX);
    match outcome {
        CommandNote::Done(message) if message.is_empty() => {
            note("ran_action", &[], &[("command", command.as_str())])
        }
        CommandNote::Done(message) => {
            let result = cap_chars(message, RESULT_MAX);
            note(
                "ran_action",
                &[],
                &[("command", command.as_str()), ("result", result.as_str())],
            )
        }
        CommandNote::Failed => note("action_failed", &[], &[("command", command.as_str())]),
        CommandNote::Refused => note("action_refused", &[], &[("command", command.as_str())]),
        CommandNote::NoMatch => note("action_not_found", &[], &[("command", command.as_str())]),
    }
}

/// event `interrupted`, no fields.
pub fn note_interrupted() -> String {
    note("interrupted", &[], &[])
}

/// event `delivery_failed`, no fields.
pub fn note_delivery_failed() -> String {
    note("delivery_failed", &[], &[])
}

/// event `reply_incomplete`, runtime `action_executed` = false.
pub fn note_incomplete() -> String {
    note(
        "reply_incomplete",
        &[("action_executed", Value::Bool(false))],
        &[],
    )
}

/// event `failed`, runtime `action_executed` = false.
pub fn note_failed() -> String {
    note("failed", &[("action_executed", Value::Bool(false))], &[])
}

/// `text + " " + note` when it fits in `max` chars; otherwise the TEXT is shortened (ending in
/// `…`, on a char boundary) so the whole note is kept; an empty `text` yields just the note.
/// If `note` alone is longer than `max`, returns the fallback note `⟦note {"event":"<event>"}⟧`
/// where `<event>` is parsed from the note's JSON (`unknown` if it cannot be parsed); the text is
/// dropped in that case. The result is never longer than `max` chars (for `max` >= 30).
pub fn with_cap(text: &str, note: &str, max: usize) -> String {
    let note_len = note.chars().count();
    if note_len > max {
        let fallback = event_only_note(&event_of(note));
        return if fallback.chars().count() > max {
            event_only_note("unknown")
        } else {
            fallback
        };
    }
    if text.is_empty() {
        return note.to_string();
    }
    if text.chars().count() + 1 + note_len <= max {
        return format!("{text} {note}");
    }
    // Room left for the text part (including its ellipsis) before the separating space.
    let avail = max.saturating_sub(note_len + 1);
    if avail == 0 {
        return note.to_string();
    }
    let mut part: String = text.chars().take(avail - 1).collect();
    part.push(ELLIPSIS);
    format!("{part} {note}")
}

/// `with_cap(&neutralize(text), note, SIDE_MAX)`: the one call the loop uses to build an
/// assistant-side history string.
pub fn compose(text: &str, note: &str) -> String {
    with_cap(&neutralize(text), note, SIDE_MAX)
}

// ---- private helpers ----

const PREFIX: &str = "\u{27E6}note ";
const ELLIPSIS: char = '\u{2026}';
/// Every untrusted string is pre-capped to this many chars before the budget loop.
const PRECAP_CHARS: usize = 400;

/// First `max` chars of `s`, with `…` appended when anything was cut.
fn cap_chars(s: &str, max: usize) -> String {
    let mut it = s.chars();
    let mut out: String = it.by_ref().take(max).collect();
    if it.next().is_some() {
        out.push(ELLIPSIS);
    }
    out
}

/// Shorten `s` to `target` chars ending in `…` (or to "" when `target` is 0).
fn shrink_to(s: &str, target: usize) -> String {
    if target == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(target - 1).collect();
    out.push(ELLIPSIS);
    out
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn json_value(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

/// Build the JSON object text by hand so the field order is exactly as documented.
fn render(event: &str, runtime: &[(&str, Value)], fields: &[(String, String)]) -> String {
    let mut out = String::from("{\"event\":");
    out.push_str(&json_str(event));
    for (key, value) in runtime {
        out.push(',');
        out.push_str(&json_str(key));
        out.push(':');
        out.push_str(&json_value(value));
    }
    for (key, text) in fields {
        out.push(',');
        out.push_str(&json_str(key));
        out.push(':');
        out.push_str(&json_str(text));
    }
    out.push('}');
    out
}

/// Escape literal markers inside the serialized JSON, then wrap it as one note.
fn wrap_json(json: &str) -> String {
    let escaped = json.replace(OPEN, r"\u27e6").replace(CLOSE, r"\u27e7");
    format!("{PREFIX}{escaped}{CLOSE}")
}

/// Fallback note carrying only the event name.
fn event_only_note(event: &str) -> String {
    wrap_json(&format!("{{\"event\":{}}}", json_str(event)))
}

/// Event name of a note, read from its JSON; `unknown` when it cannot be parsed.
fn event_of(note: &str) -> String {
    let inner = note.strip_prefix(PREFIX).unwrap_or(note);
    let inner = inner.strip_suffix(CLOSE).unwrap_or(inner);
    serde_json::from_str::<Value>(inner)
        .ok()
        .and_then(|v| v.get("event").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
#[path = "converse_context_tests.rs"]
mod tests;
