//! Pure permission policy for ACP `session/request_permission` (DESIGN.md
//! Revision 3: M2 "Types (S1)", "Trust boundary" and C6).
//!
//! No IO, no threads, no Tauri types. Tests: `acp_permission_tests.rs`.
//!
//! Trust boundary: the configured ACP executable and the permission metadata
//! it reports (tool kind, option kinds, rawInput) are trusted because the
//! owner chose and authenticated that agent. `AskWrites` is a metadata-based
//! convenience policy, NOT a sandbox: an agent that deliberately lies about
//! its metadata is out of scope, and the owner should use `AskAll` for agents
//! they are unsure about. Within that boundary every decision fails closed:
//! anything unrecognised asks the user or is cancelled, and `AllowAlways` /
//! `RejectAlways` options are never selected.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use specta::Type;
use std::fmt;

/// Maximum characters of a request title. The title is untrusted agent text.
pub const MAX_TITLE_CHARS: usize = 160;

/// Maximum characters of one labelled operation-detail value.
const MAX_FIELD_CHARS: usize = 200;

/// Maximum characters of a whole operation-detail summary.
const MAX_SUMMARY_CHARS: usize = 400;

/// rawInput keys that suggest the operation writes, runs or sends something.
/// Matched case-insensitively. A read-like request carrying any of them asks.
const CONTRADICTION_KEYS: [&str; 15] = [
    "command",
    "cmd",
    "script",
    "shell",
    "argv",
    "args",
    "new_string",
    "new_text",
    "old_string",
    "patch",
    "diff",
    "content",
    "contents",
    "write",
    "url",
];

/// Labelled operation-detail fields, in display order: label, rawInput keys in
/// priority order, and the separator used when an array value is joined.
const DETAIL_FIELDS: [(&str, &[&str], &str); 4] = [
    ("command", &["command", "cmd"], " "),
    (
        "path",
        &["path", "file_path", "filepath", "file", "paths"],
        ", ",
    ),
    ("url", &["url", "uri"], " "),
    ("query", &["query"], " "),
];

/// Kind of tool operation as reported by the agent (`toolCall.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Reads data without changing anything.
    Read,
    /// Changes the contents of existing data.
    Edit,
    /// Removes data.
    Delete,
    /// Moves or renames data.
    Move,
    /// Searches data.
    Search,
    /// Runs a command or program.
    Execute,
    /// Internal reasoning with no external effect.
    Think,
    /// Fetches something from outside the machine.
    Fetch,
    /// Switches the agent's mode.
    SwitchMode,
    /// Explicitly declared "other" kind.
    Other,
}

/// Kind of an offered permission option (`options[].kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermOptionKind {
    /// Allow this one operation.
    AllowOnce,
    /// Allow this kind of operation for the rest of the session. Never selected by Vox.
    AllowAlways,
    /// Reject this one operation.
    RejectOnce,
    /// Reject this kind of operation persistently. Never selected by Vox.
    RejectAlways,
    /// Missing or unrecognised option kind. Never selected.
    Unknown,
}

/// One permission option the agent offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermOption {
    /// Identifier to echo back in the response. Always non-empty.
    pub option_id: String,
    /// Display label, sanitized and bounded. Untrusted text.
    pub name: String,
    /// Parsed option kind.
    pub kind: PermOptionKind,
}

/// Bounded, display-only description of what an operation will touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationDetails {
    /// Labelled lines such as `command: ...` or `path: ...`. At most 400
    /// characters, no control characters. Untrusted text, for display only.
    pub summary: String,
    /// True when any value or the whole summary was shortened.
    pub truncated: bool,
}

/// A parsed, sanitized `session/request_permission` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    /// ACP session the request belongs to.
    pub session_id: String,
    /// `toolCall.toolCallId`, when the agent sent one.
    pub tool_call_id: Option<String>,
    /// Sanitized title, at most [`MAX_TITLE_CHARS`] characters. Untrusted text.
    pub title: String,
    /// Parsed tool kind. `Other` when missing or unknown.
    pub kind: ToolKind,
    /// Offered options, in the order the agent sent them. Options without a string `optionId` are dropped.
    pub options: Vec<PermOption>,
    /// True when `rawInput` is an object with a key that contradicts a read-like kind.
    pub raw_input_contradicts: bool,
    /// Display details derived from `rawInput`, when any recognised field exists.
    pub details: Option<OperationDetails>,
}

/// Opaque error for a malformed `session/request_permission` payload.
///
/// Callers treat it as "cancel": a request that cannot be parsed is never answered with a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseError;

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("malformed permission request")
    }
}

impl std::error::Error for ParseError {}

/// Parses a `session/request_permission` params object.
///
/// Total: never panics on any JSON. A non-object payload, a missing or
/// non-string `sessionId`, a missing `toolCall` object, or missing or
/// non-array `options` returns [`ParseError`]. Options without a string
/// `optionId` are skipped. Unknown kinds map to `Unknown` (options) or
/// `Other` (tool kind), which both fail closed.
pub fn parse_permission_request(params: &Value) -> Result<PermissionRequest, ParseError> {
    let obj = params.as_object().ok_or(ParseError)?;
    let session_id = obj
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or(ParseError)?;
    let tool_call = obj
        .get("toolCall")
        .and_then(Value::as_object)
        .ok_or(ParseError)?;
    let raw_options = obj
        .get("options")
        .and_then(Value::as_array)
        .ok_or(ParseError)?;

    let tool_call_id = tool_call
        .get("toolCallId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let raw_title = tool_call.get("title").and_then(Value::as_str).unwrap_or("");
    let (title, _) = sanitize_text(raw_title, MAX_TITLE_CHARS);
    let kind = parse_tool_kind(tool_call.get("kind"));

    let raw_input = tool_call.get("rawInput");
    let raw_input_contradicts = raw_input.and_then(Value::as_object).is_some_and(|map| {
        map.keys()
            .any(|key| CONTRADICTION_KEYS.contains(&key.to_ascii_lowercase().as_str()))
    });
    let details = raw_input.and_then(extract_details);
    let options = raw_options.iter().filter_map(parse_option).collect();

    Ok(PermissionRequest {
        session_id: session_id.to_string(),
        tool_call_id,
        title,
        kind,
        options,
        raw_input_contradicts,
        details,
    })
}

fn parse_tool_kind(value: Option<&Value>) -> ToolKind {
    match value.and_then(Value::as_str) {
        Some("read") => ToolKind::Read,
        Some("edit") => ToolKind::Edit,
        Some("delete") => ToolKind::Delete,
        Some("move") => ToolKind::Move,
        Some("search") => ToolKind::Search,
        Some("execute") => ToolKind::Execute,
        Some("think") => ToolKind::Think,
        Some("fetch") => ToolKind::Fetch,
        Some("switch_mode") => ToolKind::SwitchMode,
        _ => ToolKind::Other,
    }
}

fn parse_option_kind(value: Option<&Value>) -> PermOptionKind {
    match value.and_then(Value::as_str) {
        Some("allow_once") => PermOptionKind::AllowOnce,
        Some("allow_always") => PermOptionKind::AllowAlways,
        Some("reject_once") => PermOptionKind::RejectOnce,
        Some("reject_always") => PermOptionKind::RejectAlways,
        _ => PermOptionKind::Unknown,
    }
}

fn parse_option(value: &Value) -> Option<PermOption> {
    let map = value.as_object()?;
    let option_id = map
        .get("optionId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    let raw_name = map.get("name").and_then(Value::as_str).unwrap_or("");
    let (name, _) = sanitize_text(raw_name, MAX_TITLE_CHARS);
    Some(PermOption {
        option_id: option_id.to_string(),
        name,
        kind: parse_option_kind(map.get("kind")),
    })
}

/// Makes untrusted text safe to show on one line, bounded to `max_chars`.
///
/// Drops control characters and bidi or format characters that can spoof
/// display. Turns tab, CR, LF and the Unicode line/paragraph separators into
/// a single space (runs collapse). Trims, then truncates so the result is at
/// most `max_chars` characters, counting a trailing `\u{2026}`. Returns the
/// text and whether it was truncated. Counts characters, never bytes.
pub fn sanitize_text(input: &str, max_chars: usize) -> (String, bool) {
    let mut cleaned = String::with_capacity(input.len());
    for c in input.chars() {
        if is_separator(c) {
            if !cleaned.ends_with(' ') {
                cleaned.push(' ');
            }
        } else if !is_dropped(c) {
            cleaned.push(c);
        }
    }
    truncate_chars(cleaned.trim(), max_chars)
}

fn is_separator(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_dropped(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

/// Truncates `text` to at most `max_chars` characters, ending with `\u{2026}` when cut.
fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    if max_chars == 0 {
        return (String::new(), true);
    }
    let mut out: String = text.chars().take(max_chars - 1).collect();
    out.push('\u{2026}');
    (out, true)
}

/// Derives a bounded, display-only summary of what an operation will touch.
///
/// Recognised rawInput fields, in order: `command`/`cmd` (string or string
/// array, joined with spaces), `path`/`file_path`/`filepath`/`file`/`paths`
/// (string or string array, joined with ", "), `url`/`uri`, `query`. Keys
/// match case-insensitively. Returns `None` when `raw_input` is not an object
/// or has no usable recognised field. The summary is untrusted text, shown
/// for display only. It never drives a decision.
pub fn extract_details(raw_input: &Value) -> Option<OperationDetails> {
    let map = raw_input.as_object()?;
    let mut lines = Vec::new();
    let mut truncated = false;
    for (label, keys, joiner) in DETAIL_FIELDS {
        let Some(text) = lookup(map, keys).and_then(|value| value_text(value, joiner)) else {
            continue;
        };
        let (value, field_truncated) = sanitize_text(&text, MAX_FIELD_CHARS);
        if value.is_empty() {
            continue;
        }
        truncated |= field_truncated;
        lines.push(format!("{label}: {value}"));
    }
    if lines.is_empty() {
        return None;
    }
    let (summary, summary_truncated) = truncate_chars(&lines.join("\n"), MAX_SUMMARY_CHARS);
    Some(OperationDetails {
        summary,
        truncated: truncated || summary_truncated,
    })
}

/// Finds the first rawInput value whose key matches `keys`, in priority order.
fn lookup<'a>(map: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|wanted| {
        map.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
            .map(|(_, value)| value)
    })
}

/// Renders a string, or an array of strings joined by `joiner`. Any other
/// shape (including an array with a non-string element) yields `None`, so the
/// UI shows the "details unavailable" warning instead of a misleading value.
fn value_text(value: &Value, joiner: &str) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let parts = items
                .iter()
                .map(Value::as_str)
                .collect::<Option<Vec<&str>>>()?;
            if parts.is_empty() {
                None
            } else {
                Some(parts.join(joiner))
            }
        }
        _ => None,
    }
}

/// How permission requests are answered without asking the user.
///
/// `AskWrites` is a metadata-based convenience policy, not a sandbox. Use
/// `AskAll` for any agent whose behaviour is not fully trusted.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Auto-allow read-like operations (read, search, think) when safe. Ask for everything else.
    #[default]
    AskWrites,
    /// Ask the user for every request.
    AskAll,
    /// Reject every request without asking.
    Deny,
    /// Select the request's `allow_once` option without asking ("Allow everything").
    /// Never selects `allow_always`. With no `allow_once` offered it asks.
    AllowAll,
}

/// Result of the automatic policy for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoDecision {
    /// Select this `allow_once` option without asking.
    Allow {
        /// Option identifier to echo back.
        option_id: String,
    },
    /// Select this `reject_once` option. `None` means the agent offered none, so the request is cancelled.
    Reject {
        /// Option identifier to echo back, if the agent offered a reject option.
        option_id: Option<String>,
    },
    /// Ask the user.
    Ask,
}

/// Applies the automatic policy for `mode`. See the module docs for the trust boundary.
///
/// `Deny` rejects. `AskAll` always asks. `AllowAll` selects `allow_once` for every
/// request that offers one, whatever its kind or `rawInput`. `AskWrites` allows only when the kind
/// is Read, Search or Think, `rawInput` has no contradicting key, and an
/// `allow_once` option exists. Everything else asks. `allow_always` is never
/// selected.
pub fn decide_auto(mode: PermissionMode, req: &PermissionRequest) -> AutoDecision {
    match mode {
        PermissionMode::Deny => AutoDecision::Reject {
            option_id: first_option(req, PermOptionKind::RejectOnce),
        },
        PermissionMode::AskAll => AutoDecision::Ask,
        PermissionMode::AllowAll => match first_option(req, PermOptionKind::AllowOnce) {
            Some(option_id) => AutoDecision::Allow { option_id },
            None => AutoDecision::Ask,
        },
        PermissionMode::AskWrites => {
            let read_like = matches!(
                req.kind,
                ToolKind::Read | ToolKind::Search | ToolKind::Think
            );
            match first_option(req, PermOptionKind::AllowOnce) {
                Some(option_id) if read_like && !req.raw_input_contradicts => {
                    AutoDecision::Allow { option_id }
                }
                _ => AutoDecision::Ask,
            }
        }
    }
}

fn first_option(req: &PermissionRequest, wanted: PermOptionKind) -> Option<String> {
    req.options
        .iter()
        .find(|opt| opt.kind == wanted)
        .map(|opt| opt.option_id.clone())
}

/// What the user, a timer or a cancel path decided for a presented request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserChoice {
    /// The user allowed this operation once.
    Allow,
    /// The user denied this operation.
    Reject,
    /// The request was cancelled (turn ended, run replaced, or an agent-side cancel).
    Cancelled,
    /// The approval timed out.
    TimedOut,
}

/// The response to send for a `session/request_permission` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Select the option with this identifier.
    Selected(String),
    /// Report that the prompt was cancelled.
    Cancelled,
}

/// Maps a user choice onto the offered options. Fails closed.
///
/// `Allow` selects only an `allow_once` option, or cancels when there is none.
/// `Reject` selects the first `reject_once` option, or cancels. `TimedOut` and
/// `Cancelled` behave like `Reject`. `allow_always` and `reject_always` are never
/// selected.
pub fn outcome_for(choice: UserChoice, req: &PermissionRequest) -> Outcome {
    let wanted = match choice {
        UserChoice::Allow => PermOptionKind::AllowOnce,
        UserChoice::Reject | UserChoice::TimedOut | UserChoice::Cancelled => {
            PermOptionKind::RejectOnce
        }
    };
    match first_option(req, wanted) {
        Some(option_id) => Outcome::Selected(option_id),
        None => Outcome::Cancelled,
    }
}

/// Builds the JSON-RPC `result` body for a `session/request_permission` response.
pub fn outcome_json(outcome: &Outcome) -> Value {
    match outcome {
        Outcome::Selected(option_id) => {
            json!({"outcome": {"outcome": "selected", "optionId": option_id}})
        }
        Outcome::Cancelled => json!({"outcome": {"outcome": "cancelled"}}),
    }
}
