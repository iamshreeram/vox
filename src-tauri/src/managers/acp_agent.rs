//! Persistent ACP (Agent Client Protocol) client.
//!
//! CONTRACT SKELETON. Every body below is `todo!()` until step S1 implements it
//! against the design (DESIGN.md rev 3.1) and the tests in `acp_agent_tests.rs`.
//! Signatures, constants and type shapes here are part of the contract: do not
//! change them without updating the design and the tests.

#![allow(unused_variables, dead_code)]

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

/// Longest accepted stdout line. Enforced while reading (limit + 1), never allocates beyond it.
pub const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// Aggregate chunk text per turn. Over => `Protocol("reply too large")` and the child is retired.
pub const MAX_RAW_REPLY_BYTES: usize = 256 * 1024;
/// Reply cap after sanitising, truncated at a char boundary.
pub const MAX_REPLY_CHARS: usize = 16_000;
pub const STDERR_TAIL_CHARS: usize = 2_000;
pub const MAX_ERR_DETAIL_CHARS: usize = 200;
/// Helper-thread poll interval.
pub const TICK: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq)]
pub struct AcpAgentConfig {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub turn_timeout: Duration,
    pub handshake_timeout: Duration,
    pub cancel_grace: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptOptions {
    /// true: reuse one ACP session across turns. false: a fresh session per turn.
    pub reuse_session: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpReply {
    pub text: String,
    pub agent_name: Option<String>,
}

#[derive(Debug)]
pub enum AcpAgentError {
    NotConfigured,
    CommandNotFound,
    Unsupported,
    SpawnFailed(String),
    HandshakeFailed(String),
    Timeout,
    Busy,
    Stale,
    Closed,
    AgentExited { code: Option<i32>, stderr: String },
    EmptyReply,
    Refused,
    Protocol(String),
    Io(std::io::Error),
}

/// One short, user-safe line per error. Agent-provided text is always passed through
/// `cap_detail` so a hostile agent cannot flood or spoof the message.
impl std::fmt::Display for AcpAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured => write!(f, "ACP agent is not configured"),
            Self::CommandNotFound => write!(f, "ACP agent command was not found"),
            Self::Unsupported => write!(f, "ACP agent is not supported on this platform"),
            Self::SpawnFailed(msg) => write!(f, "ACP agent could not start: {}", cap_detail(msg)),
            Self::HandshakeFailed(msg) => {
                write!(f, "ACP agent handshake failed: {}", cap_detail(msg))
            }
            Self::Timeout => write!(f, "ACP agent timed out"),
            Self::Busy => write!(f, "ACP agent is busy"),
            Self::Stale => write!(f, "ACP agent request was superseded"),
            Self::Closed => write!(f, "ACP agent is closed"),
            Self::AgentExited { code, stderr } => {
                match code {
                    Some(c) => write!(f, "ACP agent exited (code {c})")?,
                    None => write!(f, "ACP agent exited")?,
                }
                let tail = cap_detail(stderr);
                if !tail.is_empty() {
                    write!(f, ": {tail}")?;
                }
                Ok(())
            }
            Self::EmptyReply => write!(f, "ACP agent returned an empty reply"),
            Self::Refused => write!(f, "ACP agent refused the request"),
            Self::Protocol(msg) => write!(f, "ACP protocol error: {}", cap_detail(msg)),
            Self::Io(e) => write!(f, "ACP agent I/O error: {}", cap_detail(&e.to_string())),
        }
    }
}

impl std::error::Error for AcpAgentError {}

/// One long-lived ACP child process behind a small state machine. `Send + Sync`.
pub struct AcpAgentManager {
    _private: (),
}

impl Default for AcpAgentManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpAgentManager {
    /// Idle shutdown of 30 minutes.
    pub fn new() -> Self {
        todo!()
    }

    pub fn with_idle_shutdown(idle: Duration) -> Self {
        todo!()
    }

    /// Blocking. See DESIGN.md sections 4-6.
    pub fn prompt(
        &self,
        cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
    ) -> Result<AcpReply, AcpAgentError> {
        todo!()
    }

    /// Spawn + initialize only. Returns the agent name when the agent reports one.
    pub fn warm_up(&self, cfg: &AcpAgentConfig) -> Result<Option<String>, AcpAgentError> {
        todo!()
    }

    pub fn reset_session(&self) {
        todo!()
    }

    pub fn is_running(&self) -> bool {
        todo!()
    }

    /// Retires the current child; the manager stays usable.
    pub fn shutdown(&self) {
        todo!()
    }

    /// Irreversible (app exit): later prompt/warm_up return `Closed`; nothing can spawn afterwards.
    pub fn close(&self) {
        todo!()
    }

    /// Live helper threads (reader, writer, stderr) of this manager's children.
    #[cfg(test)]
    pub fn helper_threads_alive(&self) -> usize {
        todo!()
    }

    /// Strong count of the shared inner state. Must stay 1: the reaper and helper
    /// threads hold only weak references.
    #[cfg(test)]
    pub fn inner_strong_count(&self) -> usize {
        todo!()
    }
}

/// Strips ESC/CSI/OSC sequences and control characters except `\n` and `\t`, trims,
/// and caps the result at `MAX_REPLY_CHARS` characters (char boundary).
pub fn sanitize_reply(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_REPLY_CHARS * 4));
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Why: terminal escape sequences would move the cursor or change terminal state
            // when the reply is shown or pasted.
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    // CSI: parameter and intermediate bytes, then one final byte in @..=~.
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    // OSC: ends at BEL or at ESC followed by a backslash (string terminator).
                    let mut prev_esc = false;
                    for n in chars.by_ref() {
                        if n == '\u{7}' || (prev_esc && n == '\\') {
                            break;
                        }
                        prev_esc = n == '\u{1b}';
                    }
                }
                // Other two-character ESC pairs: drop the introducer and the next char.
                Some(next) if next != '\u{1b}' => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out.trim().chars().take(MAX_REPLY_CHARS).collect()
}

/// Makes agent-provided text safe for one error line: controls removed, whitespace
/// collapsed, capped. Why: agent text is untrusted and must not break the log or UI line.
pub(crate) fn cap_detail(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let collapsed = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(MAX_ERR_DETAIL_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Outcome of one bounded line read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LineRead {
    Line,
    Eof,
    TooLong,
}

/// Reads one '\n'-terminated line into `buf`, never storing more than `max + 1` bytes.
/// Why: an agent that never sends a newline must not grow our memory without bound.
pub(crate) fn read_bounded_line<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<LineRead> {
    buf.clear();
    loop {
        let (consumed, found) = {
            let available = r.fill_buf()?;
            if available.is_empty() {
                return Ok(LineRead::Eof);
            }
            let room = (max + 1).saturating_sub(buf.len());
            let window = available
                .get(..available.len().min(room))
                .unwrap_or_default();
            match window.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    buf.extend_from_slice(window.get(..=i).unwrap_or_default());
                    (i + 1, true)
                }
                None => {
                    buf.extend_from_slice(window);
                    (window.len(), false)
                }
            }
        };
        r.consume(consumed);
        if found {
            return Ok(LineRead::Line);
        }
        if buf.len() > max {
            return Ok(LineRead::TooLong);
        }
    }
}

/// One classified message from the agent.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Incoming {
    /// Agent-to-client request; `id` is echoed back verbatim (any JSON type).
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// Response to one of our requests (ids are the integers we generated).
    Response {
        id: u64,
        result: Option<Value>,
        error: Option<Value>,
    },
}

/// Parses one stdout line. Why: the agent may print junk (escape bytes) before its JSON,
/// so parsing starts at the first '{'; anything else is dropped rather than trusted.
pub(crate) fn classify_line(raw: &[u8]) -> Option<Incoming> {
    let start = raw.iter().position(|&b| b == b'{')?;
    let value: Value = serde_json::from_slice(raw.get(start..)?).ok()?;
    let obj = value.as_object()?;
    let method = obj.get("method").and_then(Value::as_str).map(str::to_owned);
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match (method, obj.get("id")) {
        (Some(method), Some(id)) => Some(Incoming::Request {
            id: id.clone(),
            method,
            params,
        }),
        (Some(method), None) => Some(Incoming::Notification { method, params }),
        (None, Some(id)) if obj.contains_key("result") || obj.contains_key("error") => {
            Some(Incoming::Response {
                id: id.as_u64()?,
                result: obj.get("result").cloned(),
                error: obj.get("error").cloned(),
            })
        }
        _ => None,
    }
}

/// Client request as one JSON line (the writer adds the newline).
pub(crate) fn request_json(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

/// Client notification (no id) as one JSON line.
pub(crate) fn notification_json(method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string()
}

/// Success response to an agent request, echoing its id verbatim.
pub(crate) fn response_ok_json(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// Error response to an agent request (e.g. -32601 for unsupported methods).
pub(crate) fn response_err_json(id: &Value, code: i64, message: &str) -> String {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}).to_string()
}

/// Answer to `session/request_permission`: always denied. Why: Vox grants no client-side
/// capability in this version; the agent's own tools are not controlled by this answer.
pub(crate) fn permission_denied_result() -> Value {
    json!({"outcome": {"outcome": "cancelled"}})
}

/// Code and message of a JSON-RPC error object, capped for display.
pub(crate) fn error_detail(err: &Value) -> String {
    let code = err
        .get("code")
        .and_then(Value::as_i64)
        .map_or_else(|| "unknown".to_string(), |c| c.to_string());
    let message = err.get("message").and_then(Value::as_str).unwrap_or("");
    cap_detail(&format!("code {code}: {message}"))
}

/// What a `session/prompt` stop reason means for the turn (DESIGN 6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    Reply,
    ReplyIfText,
    Refused,
    Cancelled,
    Unknown,
}

pub(crate) fn map_stop_reason(v: Option<&str>) -> StopOutcome {
    match v {
        Some("end_turn" | "max_tokens") => StopOutcome::Reply,
        Some("max_turn_requests") => StopOutcome::ReplyIfText,
        Some("refusal") => StopOutcome::Refused,
        Some("cancelled") => StopOutcome::Cancelled,
        _ => StopOutcome::Unknown,
    }
}

pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false
        },
        "clientInfo": {"name": "vox", "version": env!("CARGO_PKG_VERSION")}
    })
}

pub(crate) fn session_new_params(cwd: &std::path::Path) -> Value {
    json!({"cwd": cwd.to_string_lossy().into_owned(), "mcpServers": []})
}

pub(crate) fn prompt_params(session_id: &str, text: &str) -> Value {
    json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]})
}

pub(crate) fn cancel_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}

pub(crate) fn close_params(session_id: &str) -> Value {
    json!({"sessionId": session_id})
}
