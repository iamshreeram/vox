//! Stage 2 agent hand-off, pure part: the events of one agent run, the run trait the
//! conversation loop polls, the prepared (immutable) launch and its fingerprint, and the
//! result cap. No I/O. The production run lives in `converse_platform.rs`.
//!
//! Design: DESIGN-PR2-STAGE2.md Revision 3, sections 2.3 and 2.5.

use std::time::Duration;

use crate::managers::converse_plan::MAX_ROLE_CHARS;

/// Longest agent reply kept, in chars. Longer replies are cut on a char boundary.
pub const MAX_RESULT_CHARS: usize = 4000;

/// Spoken when an agent run fails and the error has no user-facing text of its own.
pub const AGENT_FAILED_FALLBACK: &str = "The agent request failed.";

/// How one agent run ended. Exactly one terminal event is sent per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// The agent's reply text (already capped by `cap_result`).
    Done(String),
    /// A fixed, user-safe failure line.
    Failed(String),
    /// The run was cancelled by the caller.
    Cancelled,
}

/// One poll of a running agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPoll {
    /// Nothing yet.
    Pending,
    /// The terminal event of the run.
    Event(AgentEvent),
    /// The worker vanished without a terminal event (it panicked or was dropped). Terminal.
    Lost,
}

/// A started agent run, owned by the conversation loop.
pub trait AgentRun: Send {
    /// Non-blocking. Returns the terminal event once, then `Pending` (see the implementation).
    fn poll(&mut self) -> AgentPoll;
    /// Non-blocking request to stop the run. Never waits for the agent.
    fn cancel(&self);
    /// Time since the run was started.
    fn elapsed(&self) -> Duration;
}

/// Why an agent run could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStartError {
    /// The host cannot run agents at all.
    Unsupported,
    /// The agent is disabled or has no command.
    NotConfigured,
    /// Another agent request holds the agent.
    Busy,
    /// Any other failure, with a short user-safe reason.
    Failed(String),
}

/// A conversation agent request with every setting it needs, fixed before the user is asked
/// anything. `start_agent` uses this value only; it never re-reads the session or settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedAgentLaunch {
    pub task: String,
    pub role: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub turn_timeout: Duration,
    pub fingerprint: u64,
}

impl PreparedAgentLaunch {
    /// Builds a launch and computes its fingerprint from the launch settings.
    pub fn new(
        task: String,
        role: Option<String>,
        command: String,
        args: Vec<String>,
        cwd: String,
        turn_timeout: Duration,
    ) -> Self {
        let fingerprint = compute_fingerprint(&command, &args, &cwd, role.as_deref(), turn_timeout);
        PreparedAgentLaunch {
            task,
            role,
            command,
            args,
            cwd,
            turn_timeout,
            fingerprint,
        }
    }

    /// The text sent to the agent: the task alone, or the task under the role (role capped).
    pub fn prompt_text(&self) -> String {
        match self.role.as_deref() {
            None => self.task.clone(),
            Some(role) => {
                let role: String = role.chars().take(MAX_ROLE_CHARS).collect();
                format!("Act in this role: {role}\n\nRequest: {}", self.task)
            }
        }
    }
}

/// FNV-1a 64-bit offset basis and prime. Fixed constants, so the value is stable across runs
/// and builds (unlike `DefaultHasher`, whose algorithm is not guaranteed).
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Feeds one length-prefixed byte field into the hash, so `("ab","c")` and `("a","bc")` differ.
fn feed(hash: &mut u64, bytes: &[u8]) {
    for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

/// Stable fingerprint of the launch settings: command, args, cwd, role and turn timeout. The
/// task is not part of it (it is per request). Equal inputs always give equal output.
pub fn compute_fingerprint(
    command: &str,
    args: &[String],
    cwd: &str,
    role: Option<&str>,
    turn_timeout: Duration,
) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    feed(&mut hash, command.as_bytes());
    feed(&mut hash, &(args.len() as u64).to_le_bytes());
    for arg in args {
        feed(&mut hash, arg.as_bytes());
    }
    feed(&mut hash, cwd.as_bytes());
    match role {
        None => feed(&mut hash, &[0]),
        Some(role) => {
            feed(&mut hash, &[1]);
            feed(&mut hash, role.as_bytes());
        }
    }
    feed(&mut hash, &turn_timeout.as_nanos().to_le_bytes());
    hash
}

/// Caps an agent reply at `MAX_RESULT_CHARS` chars (char boundary). Nothing else changes.
pub fn cap_result(text: &str) -> String {
    text.trim().chars().take(MAX_RESULT_CHARS).collect()
}

/// True when the hand-off can run: the agent is enabled and has a non-blank command.
pub fn hand_off_configured(enabled: bool, command: Option<&str>) -> bool {
    enabled && command.is_some_and(|c| !c.trim().is_empty())
}
