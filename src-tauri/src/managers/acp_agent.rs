//! Persistent ACP (Agent Client Protocol) client.
//!
//! CONTRACT SKELETON. Every body below is `todo!()` until step S1 implements it
//! against the design (DESIGN.md rev 3.1) and the tests in `acp_agent_tests.rs`.
//! Signatures, constants and type shapes here are part of the contract: do not
//! change them without updating the design and the tests.

#![allow(unused_variables, dead_code)]

use std::path::PathBuf;
use std::time::Duration;

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

impl std::fmt::Display for AcpAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        todo!()
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
    todo!()
}
