//! Persistent ACP (Agent Client Protocol) client.
//!
//! CONTRACT SKELETON. Every body below is `todo!()` until step S1 implements it
//! against the design (DESIGN.md rev 3.1) and the tests in `acp_agent_tests.rs`.
//! Signatures, constants and type shapes here are part of the contract: do not
//! change them without updating the design and the tests.

#![allow(unused_variables, dead_code)]

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(unix)]
use std::process::{ChildStderr, ChildStdin, ChildStdout};
#[cfg(unix)]
use std::sync::mpsc::Receiver;

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

// ---------------------------------------------------------------- connection layer (S1b-2)

/// Why a connection was retired. Stored once by `retire`; waiters map it to an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RetireReason {
    Shutdown,
    Idle,
    Timeout,
    Replaced,
    Eof,
    Protocol(String),
    Io(String),
}

/// What a waiter receives: the JSON-RPC `result`, a JSON-RPC `error` object, or the
/// news that the connection was retired (the reason is read from the connection).
pub(crate) enum ConnFail {
    Rpc(Value),
    Retired,
}

/// Delivery slot of one in-flight request.
type Waiter = mpsc::Sender<Result<Value, ConnFail>>;

/// Bounded state of the turn currently running on a connection.
#[derive(Default)]
pub(crate) struct TurnAcc {
    /// Session whose `session/update` notifications are accepted (None = no turn).
    active_session: Option<String>,
    text: String,
    /// Set when the aggregate would exceed `MAX_RAW_REPLY_BYTES`.
    overflow: bool,
}

/// Source of unique connection ids (diagnostics and test assertions).
static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Locks ignoring poisoning. Why: a panicking helper must not turn every later call into
/// a panic; the protected data is plain state that stays consistent per update.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Waits on `cv` until `done` holds or `deadline` passes; returns the guard either way.
fn wait_deadline<'a, T>(
    cv: &Condvar,
    mut guard: MutexGuard<'a, T>,
    deadline: Instant,
    done: impl Fn(&T) -> bool,
) -> MutexGuard<'a, T> {
    while !done(&guard) {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        guard = cv
            .wait_timeout(guard, deadline - now)
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    guard
}

/// Counts one live helper thread. Created before the thread starts and dropped when the
/// thread's closure ends, so `helper_threads_alive()` never reports a thread that is gone.
struct HelperGuard(Arc<AtomicUsize>);

impl HelperGuard {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter))
    }
}

impl Drop for HelperGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One spawned ACP child with its pipes, helper threads and in-flight requests.
///
/// Lock order (never hold two of these at once, except where noted in `retire`):
/// `pending` / `reason` / `acc` / `exit` / `stderr_tail` / `stderr_done` / `child` are leaves.
/// No lock is held while calling `retire`, `submit`, or any blocking wait.
pub(crate) struct Conn {
    pub(crate) conn_id: u64,
    /// The leader. Locked only briefly for `try_wait`/`wait`; never held across a sleep.
    child: Mutex<Child>,
    /// Process-group id (== leader pid). Only signalled when > 1 (never 0 or -1).
    pgid: i32,
    /// Set exactly once by the winning `retire` call.
    retired: AtomicBool,
    reason: Mutex<Option<RetireReason>>,
    /// Request id -> waiter. Drained by `retire`, so every waiter fails exactly once.
    pending: Mutex<HashMap<u64, Waiter>>,
    /// Bounded outbound queue drained by the writer thread (capacity 8).
    outbound: SyncSender<String>,
    next_id: AtomicU64,
    acc: Mutex<TurnAcc>,
    acc_cv: Condvar,
    /// Last `STDERR_TAIL_CHARS` characters of the child's stderr.
    stderr_tail: Mutex<String>,
    stderr_done: Mutex<bool>,
    stderr_cv: Condvar,
    /// `None` until the leader is reaped; then `Some(exit code or None on signal)`.
    exit: Mutex<Option<Option<i32>>>,
    exit_cv: Condvar,
    helpers: Arc<AtomicUsize>,
}

impl Conn {
    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Error a waiter reports when the connection was retired for `self.reason`.
    pub(crate) fn retire_error(&self) -> AcpAgentError {
        let reason = lock(&self.reason).clone();
        match reason {
            None | Some(RetireReason::Shutdown | RetireReason::Idle | RetireReason::Replaced) => {
                AcpAgentError::Closed
            }
            Some(RetireReason::Timeout) => AcpAgentError::Timeout,
            Some(RetireReason::Eof | RetireReason::Io(_)) => self.exit_error(),
            Some(RetireReason::Protocol(msg)) => AcpAgentError::Protocol(msg),
        }
    }

    /// `AgentExited` with the exit code and stderr tail. Waits at most 1 s for the leader
    /// to be reaped and for the stderr thread to reach EOF, so the tail is complete when
    /// the child died on its own. Why: the caller otherwise races the helper threads.
    pub(crate) fn exit_error(&self) -> AcpAgentError {
        let deadline = Instant::now() + Duration::from_secs(1);
        let exit = wait_deadline(&self.exit_cv, lock(&self.exit), deadline, Option::is_some);
        let code = (*exit).flatten();
        drop(exit);
        let done = wait_deadline(&self.stderr_cv, lock(&self.stderr_done), deadline, |d| *d);
        drop(done);
        let stderr = lock(&self.stderr_tail).clone();
        AcpAgentError::AgentExited { code, stderr }
    }

    /// Records the leader's exit (first writer wins) and wakes `exit_error` waiters.
    fn record_exit(&self, code: Option<i32>) {
        {
            let mut exit = lock(&self.exit);
            if exit.is_none() {
                *exit = Some(code);
            }
        }
        self.exit_cv.notify_all();
    }

    /// Non-blocking reap attempt. Returns true once the leader is reaped.
    fn try_reap(&self) -> bool {
        let status = lock(&self.child).try_wait();
        match status {
            Ok(Some(s)) => {
                self.record_exit(s.code());
                true
            }
            Ok(None) => false,
            Err(_) => {
                self.record_exit(None);
                true
            }
        }
    }

    /// Blocking reap. Used only after SIGKILL, which guarantees the leader dies.
    fn reap_blocking(&self) {
        let status = lock(&self.child).wait();
        self.record_exit(status.ok().and_then(|s| s.code()));
    }

    /// Kills the whole process group: SIGTERM, up to 2 s for the leader, then SIGKILL and
    /// a blocking reap. Why the group: the agent's own helper processes must not survive it.
    #[cfg(unix)]
    fn terminate(&self) {
        if self.pgid > 1 {
            // SAFETY: kill(2) only sends a signal; pgid is validated above (> 1).
            unsafe { libc::kill(-self.pgid, libc::SIGTERM) };
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.try_reap() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if self.pgid > 1 {
            // Sweeps descendants even when the leader already exited on SIGTERM.
            // SAFETY: as above.
            unsafe { libc::kill(-self.pgid, libc::SIGKILL) };
        }
        self.reap_blocking();
    }

    #[cfg(not(unix))]
    fn terminate(&self) {
        let _ = lock(&self.child).kill();
        self.reap_blocking();
    }

    /// Registers, sends and waits for one request. Returns the raw JSON-RPC outcome so
    /// the manager can map initialize/session errors to `HandshakeFailed` itself.
    pub(crate) fn request_raw(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Result<Value, Value>, AcpAgentError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel::<Result<Value, ConnFail>>();
        lock(&self.pending).insert(id, tx);
        // Why: retire sets the flag before draining `pending`; re-checking after the insert
        // closes the window where the drain already happened and our waiter would hang.
        if self.is_retired() {
            lock(&self.pending).remove(&id);
            return Err(self.retire_error());
        }
        if let Err(e) = self.submit(request_json(id, method, params), deadline) {
            lock(&self.pending).remove(&id);
            return Err(e);
        }
        let wait = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Ok(result)) => Ok(Ok(result)),
            Ok(Err(ConnFail::Rpc(err))) => Ok(Err(err)),
            Ok(Err(ConnFail::Retired)) | Err(RecvTimeoutError::Disconnected) => {
                Err(self.retire_error())
            }
            Err(RecvTimeoutError::Timeout) => {
                // A late response may still be in the channel; the caller treats it as a
                // timeout regardless (DESIGN 5.8: timeout precedence).
                lock(&self.pending).remove(&id);
                Err(AcpAgentError::Timeout)
            }
        }
    }

    /// Like `request_raw`, but a JSON-RPC error becomes `Protocol`.
    pub(crate) fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, AcpAgentError> {
        match self.request_raw(method, params, deadline)? {
            Ok(result) => Ok(result),
            Err(err) => Err(AcpAgentError::Protocol(error_detail(&err))),
        }
    }

    /// Best-effort notification: never blocks, errors ignored (used for session/close).
    pub(crate) fn try_notify(&self, method: &str, params: Value) {
        let _ = self.outbound.try_send(notification_json(method, params));
    }

    /// Queues one line for the writer, polling until the deadline or retirement.
    /// Why polling: a blocking send could outlive the absolute deadline.
    fn submit(&self, line: String, deadline: Instant) -> Result<(), AcpAgentError> {
        let mut line = line;
        loop {
            if self.is_retired() {
                return Err(self.retire_error());
            }
            match self.outbound.try_send(line) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        return Err(AcpAgentError::Timeout);
                    }
                    line = back;
                    thread::sleep(Duration::from_millis(5));
                }
                Err(TrySendError::Disconnected(_)) => return Err(self.retire_error()),
            }
        }
    }

    /// Starts collecting agent text for the session `session_id` (the reader ignores every
    /// other session's updates). Must be called before the prompt request is sent.
    pub(crate) fn begin_turn(&self, session_id: &str) {
        let mut acc = lock(&self.acc);
        acc.active_session = Some(session_id.to_owned());
        acc.text.clear();
        acc.overflow = false;
    }

    /// Stops collecting and takes the accumulated text.
    pub(crate) fn end_turn(&self) -> String {
        let mut acc = lock(&self.acc);
        acc.active_session = None;
        std::mem::take(&mut acc.text)
    }

    pub(crate) fn turn_text_len(&self) -> usize {
        lock(&self.acc).text.len()
    }
}

/// Tears a connection down. Idempotent: only the first caller proceeds (DESIGN 6a).
/// Order matters: the reason is stored before `pending` is drained, so a woken waiter
/// always finds the reason; the helper threads are never joined (they exit on their own
/// within one tick once `retired` is set).
pub(crate) fn retire(conn: &Arc<Conn>, reason: RetireReason) {
    if conn
        .retired
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    *lock(&conn.reason) = Some(reason);
    let waiters: Vec<Waiter> = lock(&conn.pending).drain().map(|(_, w)| w).collect();
    for waiter in waiters {
        let _ = waiter.send(Err(ConnFail::Retired));
    }
    // Why: taking the lock before notifying closes the race with a turn that checked
    // `retired` under the lock and is about to wait on the condvar.
    drop(lock(&conn.acc));
    conn.acc_cv.notify_all();
    conn.terminate();
}

/// Result of applying one `session/update` to the turn accumulator.
#[derive(Debug, PartialEq, Eq)]
enum UpdateEffect {
    Ignored,
    Appended,
    Overflow,
}

/// Text of an `agent_message_chunk` whose content is text. Every other update is None.
fn message_chunk_text(params: &Value) -> Option<&str> {
    let update = params.get("update")?;
    if update.get("sessionUpdate")?.as_str()? != "agent_message_chunk" {
        return None;
    }
    let content = update.get("content")?;
    if content.get("type")?.as_str()? != "text" {
        return None;
    }
    content.get("text")?.as_str()
}

/// Applies one notification to the active turn. Only the active session counts (DESIGN 6b):
/// a stray or late update must never leak into another turn's reply.
fn apply_update(conn: &Conn, params: &Value) -> UpdateEffect {
    let mut acc = lock(&conn.acc);
    let session_matches = acc.active_session.is_some()
        && params.get("sessionId").and_then(Value::as_str) == acc.active_session.as_deref();
    if !session_matches {
        return UpdateEffect::Ignored;
    }
    let Some(text) = message_chunk_text(params) else {
        return UpdateEffect::Ignored;
    };
    if text.is_empty() {
        return UpdateEffect::Ignored;
    }
    if acc.text.len().saturating_add(text.len()) > MAX_RAW_REPLY_BYTES {
        acc.overflow = true;
        return UpdateEffect::Overflow;
    }
    acc.text.push_str(text);
    UpdateEffect::Appended
}

/// Routes one stdout line (reader thread only). Dispatch rules: DESIGN 6b.
fn handle_line(conn: &Arc<Conn>, raw: &[u8]) {
    match classify_line(raw) {
        // Junk, invalid JSON and non-objects are dropped without echoing their content.
        None => {}
        Some(Incoming::Response { id, result, error }) => {
            let waiter = lock(&conn.pending).remove(&id);
            if let Some(waiter) = waiter {
                let outcome = match error.filter(|e| !e.is_null()) {
                    Some(err) => Err(ConnFail::Rpc(err)),
                    None => Ok(result.unwrap_or(Value::Null)),
                };
                let _ = waiter.send(outcome);
            }
        }
        Some(Incoming::Request { id, method, .. }) => {
            let reply = if method == "session/request_permission" {
                response_ok_json(&id, permission_denied_result())
            } else {
                response_err_json(&id, -32601, "method not found")
            };
            // Why try_send: the reader must never block on the writer. A full queue means
            // the agent floods requests while not reading; that is a protocol failure.
            if let Err(TrySendError::Full(_)) = conn.outbound.try_send(reply) {
                retire(
                    conn,
                    RetireReason::Protocol("outbound queue full".to_owned()),
                );
            }
        }
        Some(Incoming::Notification { method, params }) => {
            if method == "session/update" {
                match apply_update(conn, &params) {
                    UpdateEffect::Ignored => {}
                    UpdateEffect::Appended => conn.acc_cv.notify_all(),
                    UpdateEffect::Overflow => {
                        retire(conn, RetireReason::Protocol("reply too large".to_owned()));
                    }
                }
            }
        }
    }
}

/// Blocks until `fd` is ready for `events` or one TICK passes (EINTR included). The result
/// is ignored on purpose: callers re-check `retired` and retry after every return.
#[cfg(unix)]
fn wait_fd(fd: RawFd, events: libc::c_short) {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let ms = libc::c_int::try_from(TICK.as_millis()).unwrap_or(100);
    // SAFETY: one valid, initialised pollfd lives for the whole call.
    unsafe { libc::poll(&mut pfd, 1, ms) };
}

/// Puts a pipe end into non-blocking mode so no helper can block past a tick.
#[cfg(unix)]
fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: F_GETFL/F_SETFL only read and update the flags of an fd we own.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// Reader adapter: turns WouldBlock into a tick-bounded poll and gives up once the
/// connection is retired, so a blocked read never outlives the retirement by more than
/// one TICK.
#[cfg(unix)]
struct PollRead<R> {
    src: R,
    fd: RawFd,
    conn: Arc<Conn>,
}

#[cfg(unix)]
impl<R: Read> Read for PollRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            match self.src.read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    if self.conn.is_retired() {
                        return Err(std::io::Error::other("connection retired"));
                    }
                    wait_fd(self.fd, libc::POLLIN);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Writes all of `line`, waiting for POLLOUT on partial writes. Gives up once retired.
#[cfg(unix)]
fn write_line(conn: &Conn, stdin: &mut ChildStdin, fd: RawFd, line: &[u8]) -> std::io::Result<()> {
    let mut sent = 0;
    while sent < line.len() {
        let rest = line.get(sent..).unwrap_or_default();
        match stdin.write(rest) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => sent += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if conn.is_retired() {
                    return Err(std::io::Error::other("connection retired"));
                }
                wait_fd(fd, libc::POLLOUT);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// WRITER thread: sole owner of stdin. Exits when retired or when every sender is gone.
#[cfg(unix)]
fn writer_loop(conn: Arc<Conn>, rx: Receiver<String>, mut stdin: ChildStdin, fd: RawFd) {
    loop {
        if conn.is_retired() {
            return;
        }
        match rx.recv_timeout(TICK) {
            Ok(mut line) => {
                line.push('\n');
                if let Err(e) = write_line(&conn, &mut stdin, fd, line.as_bytes()) {
                    retire(&conn, RetireReason::Io(e.to_string()));
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// READER thread: the dispatcher and the only consumer of stdout.
#[cfg(unix)]
fn reader_loop(conn: Arc<Conn>, stdout: ChildStdout) {
    let fd = stdout.as_raw_fd();
    let src = PollRead {
        src: stdout,
        fd,
        conn: Arc::clone(&conn),
    };
    let mut reader = std::io::BufReader::new(src);
    let mut buf = Vec::new();
    loop {
        if conn.is_retired() {
            return;
        }
        match read_bounded_line(&mut reader, &mut buf, MAX_LINE_BYTES) {
            Ok(LineRead::Line) => handle_line(&conn, &buf),
            Ok(LineRead::Eof) => {
                retire(&conn, RetireReason::Eof);
                return;
            }
            Ok(LineRead::TooLong) => {
                retire(&conn, RetireReason::Protocol("line too long".to_owned()));
                return;
            }
            Err(e) => {
                retire(&conn, RetireReason::Io(e.to_string()));
                return;
            }
        }
    }
}

/// Keeps only the last `STDERR_TAIL_CHARS` characters. Cuts on a char boundary.
fn push_tail(tail: &mut String, chunk: &str) {
    tail.push_str(chunk);
    let total = tail.chars().count();
    if total > STDERR_TAIL_CHARS {
        let cut = tail
            .char_indices()
            .nth(total - STDERR_TAIL_CHARS)
            .map_or(0, |(i, _)| i);
        tail.drain(..cut);
    }
}

/// STDERR thread: drains continuously so a chatty agent can never block on a full pipe.
#[cfg(unix)]
fn stderr_loop(conn: Arc<Conn>, stderr: ChildStderr) {
    let fd = stderr.as_raw_fd();
    let mut src = PollRead {
        src: stderr,
        fd,
        conn: Arc::clone(&conn),
    };
    let mut chunk = [0u8; 4096];
    while let Ok(n) = src.read(&mut chunk) {
        if n == 0 {
            break;
        }
        // Why lossy per chunk: a split multi-byte char shows as U+FFFD, never as a panic.
        let text = String::from_utf8_lossy(chunk.get(..n).unwrap_or_default());
        push_tail(&mut lock(&conn.stderr_tail), &text);
    }
    *lock(&conn.stderr_done) = true;
    conn.stderr_cv.notify_all();
}

/// Kills and reaps a child that never became a connection, then reports `SpawnFailed`.
#[cfg(unix)]
fn abort_spawn(mut child: Child, msg: &str) -> AcpAgentError {
    let _ = child.kill();
    let _ = child.wait();
    AcpAgentError::SpawnFailed(cap_detail(msg))
}

/// Starts one named helper thread that owns a `Conn` handle and a live-count guard.
/// On failure the connection is retired (the child is killed) and `SpawnFailed` returned.
#[cfg(unix)]
fn start_helper<F>(conn: &Arc<Conn>, name: &str, body: F) -> Result<(), AcpAgentError>
where
    F: FnOnce(Arc<Conn>) + Send + 'static,
{
    let guard = HelperGuard::new(&conn.helpers);
    let handle_conn = Arc::clone(conn);
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let _alive = guard;
            body(handle_conn);
        })
        .map(|_handle| ())
        .map_err(|e| {
            retire(conn, RetireReason::Io(e.to_string()));
            AcpAgentError::SpawnFailed(cap_detail(&e.to_string()))
        })
}

impl Conn {
    /// Spawns the child (no shell, own process group, piped stdio, cwd = `cfg.cwd`) and its
    /// three helper threads. A missing program is `CommandNotFound`; any other spawn error
    /// is `SpawnFailed`.
    #[cfg(unix)]
    pub(crate) fn spawn(
        cfg: &AcpAgentConfig,
        helpers: Arc<AtomicUsize>,
    ) -> Result<Arc<Self>, AcpAgentError> {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .current_dir(&cfg.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Why a new group: retirement must reach the agent's own helper processes.
            .process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                AcpAgentError::CommandNotFound
            } else {
                AcpAgentError::SpawnFailed(cap_detail(&e.to_string()))
            }
        })?;
        let pgid = match i32::try_from(child.id()) {
            Ok(pid) if pid > 1 => pid,
            _ => return Err(abort_spawn(child, "invalid child pid")),
        };
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(abort_spawn(child, "missing stdio pipe"));
        };
        // Why O_NONBLOCK: every helper polls with a tick, so none can block past a retirement.
        let (fd_in, fd_out, fd_err) = (stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd());
        for fd in [fd_in, fd_out, fd_err] {
            if let Err(e) = set_nonblocking(fd) {
                return Err(abort_spawn(child, &e.to_string()));
            }
        }

        // Why bounded(8): a stuck agent must hit `Full` (=> retire), never grow memory.
        let (outbound, rx) = mpsc::sync_channel::<String>(8);
        let conn = Arc::new(Conn {
            conn_id: NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst),
            child: Mutex::new(child),
            pgid,
            retired: AtomicBool::new(false),
            reason: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            outbound,
            next_id: AtomicU64::new(1),
            acc: Mutex::new(TurnAcc::default()),
            acc_cv: Condvar::new(),
            stderr_tail: Mutex::new(String::new()),
            stderr_done: Mutex::new(false),
            stderr_cv: Condvar::new(),
            exit: Mutex::new(None),
            exit_cv: Condvar::new(),
            helpers,
        });
        start_helper(&conn, "acp-writer", move |c| {
            writer_loop(c, rx, stdin, fd_in);
        })?;
        start_helper(&conn, "acp-reader", move |c| reader_loop(c, stdout))?;
        start_helper(&conn, "acp-stderr", move |c| stderr_loop(c, stderr))?;
        Ok(conn)
    }

    /// Non-unix platforms have no process-group or poll support: report it plainly.
    #[cfg(not(unix))]
    pub(crate) fn spawn(
        _cfg: &AcpAgentConfig,
        _helpers: Arc<AtomicUsize>,
    ) -> Result<Arc<Self>, AcpAgentError> {
        Err(AcpAgentError::Unsupported)
    }
}
