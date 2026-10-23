//! Application seam between a finished dictation transcript and the ACP agent.
//!
//! DESIGN.md rev 3.1, sections 3, 3a and 3b. Tests: `acp_escalation_tests.rs`.

use crate::managers::acp_agent::{
    AcpAgentConfig, AcpAgentError, AcpAgentManager, AcpReply, PromptOptions,
};
use crate::managers::conversation::{
    build_agent_prompt, should_append_exchange, ConversationLog, Ticket,
};

/// Maximum characters of agent-provided detail shown to the user.
const MAX_USER_DETAIL_CHARS: usize = 200;

pub trait AcpPrompter {
    fn prompt(
        &self,
        cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
    ) -> Result<AcpReply, AcpAgentError>;
    fn reset_session(&self);
}

impl AcpPrompter for AcpAgentManager {
    fn prompt(
        &self,
        cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
    ) -> Result<AcpReply, AcpAgentError> {
        AcpAgentManager::prompt(self, cfg, text, opts)
    }

    fn reset_session(&self) {
        AcpAgentManager::reset_session(self)
    }
}

/// Where escalation results go (Tauri events in production, a recorder in tests).
pub trait EscalationSink {
    fn reply(&self, text: &str);
    fn exchange(&self, user: &str, reply: &str);
    fn error(&self, user_message: &str);
}

pub struct EscalationEnv<'a> {
    pub log: &'a ConversationLog,
    /// `agent_context_enabled` at dispatch time.
    pub ctx_enabled: bool,
    pub memory_enabled: bool,
    pub recall: &'a dyn Fn(&str) -> Vec<String>,
    /// `agent_context_enabled` read again when the reply arrives.
    pub ctx_enabled_now: &'a dyn Fn() -> bool,
    pub now_ms: &'a dyn Fn() -> u64,
}

/// Runs one escalation end to end: ticket, prompt, events, recording. Blocking.
pub fn run_escalation<P: AcpPrompter, S: EscalationSink>(
    p: &P,
    sink: &S,
    env: &EscalationEnv<'_>,
    cfg: &AcpAgentConfig,
    transcript: &str,
) {
    // Ticket only when context is on; history is never re-sent, only facts.
    let ticket: Option<Ticket> = env.ctx_enabled.then(|| env.log.begin());
    let prompt_text = if env.ctx_enabled && env.memory_enabled {
        let facts = (env.recall)(transcript);
        build_agent_prompt(transcript, &facts, &[])
    } else {
        transcript.to_string()
    };
    let opts = PromptOptions {
        reuse_session: env.ctx_enabled,
    };

    match p.prompt(cfg, &prompt_text, opts) {
        Ok(reply) => {
            sink.reply(&reply.text);
            sink.exchange(transcript, &reply.text);
            record_exchange(ticket, env, transcript, &reply.text);
        }
        Err(err) => {
            if let Some(message) = user_message_for(&err) {
                sink.error(&message);
            }
        }
    }
}

/// Records a finished exchange only when the ticket is present, no clear
/// happened since dispatch, and context is still enabled.
fn record_exchange(ticket: Option<Ticket>, env: &EscalationEnv<'_>, transcript: &str, reply: &str) {
    let Some(ticket) = ticket else {
        return;
    };
    if should_append_exchange(ticket.epoch, env.log.epoch(), (env.ctx_enabled_now)()) {
        env.log.record(ticket, transcript, reply, (env.now_ms)());
    }
}

/// User-facing text for an error, or `None` when the outcome must stay silent
/// (`Stale`, `Closed`). Agent-provided detail is capped at 200 characters.
pub fn user_message_for(err: &AcpAgentError) -> Option<String> {
    let message = match err {
        AcpAgentError::Stale | AcpAgentError::Closed => return None,
        AcpAgentError::NotConfigured => {
            "The ACP agent is not configured. Set its command in settings.".to_string()
        }
        AcpAgentError::CommandNotFound => {
            "The ACP agent program was not found. Check its command path in settings.".to_string()
        }
        AcpAgentError::Unsupported => {
            "The ACP agent is not supported on this platform.".to_string()
        }
        AcpAgentError::SpawnFailed(detail) => {
            with_detail("The ACP agent could not be started.", detail)
        }
        AcpAgentError::HandshakeFailed(detail) => {
            with_detail("The ACP agent handshake failed.", detail)
        }
        AcpAgentError::Timeout => "The ACP agent did not answer in time.".to_string(),
        AcpAgentError::Busy => "The ACP agent is busy with another request.".to_string(),
        AcpAgentError::AgentExited { stderr, .. } => {
            with_detail("The ACP agent exited unexpectedly.", stderr)
        }
        AcpAgentError::EmptyReply => "The ACP agent returned an empty reply.".to_string(),
        AcpAgentError::Refused => "The ACP agent declined to answer.".to_string(),
        AcpAgentError::Protocol(detail) => {
            with_detail("The ACP agent sent an unexpected response.", detail)
        }
        AcpAgentError::Io(_) => "The connection to the ACP agent failed.".to_string(),
    };
    Some(message)
}

/// Appends sanitized, capped agent detail to a fixed sentence.
fn with_detail(prefix: &str, detail: &str) -> String {
    let detail = sanitize_user_detail(detail);
    if detail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} {detail}")
    }
}

/// Replaces control characters with spaces, collapses whitespace, and caps the
/// result at [`MAX_USER_DETAIL_CHARS`] characters (on a char boundary).
fn sanitize_user_detail(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .chars()
        .take(MAX_USER_DETAIL_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Clear-history seam: clears the log when present and ALWAYS resets the ACP session.
pub fn on_history_cleared(log: Option<&ConversationLog>, p: &dyn AcpPrompter) {
    if let Some(log) = log {
        log.clear();
    }
    p.reset_session();
}

/// Context-setting seam: `enabled == false` behaves like `on_history_cleared`; `true` does nothing.
pub fn on_context_setting_changed(
    enabled: bool,
    log: Option<&ConversationLog>,
    p: &dyn AcpPrompter,
) {
    if !enabled {
        on_history_cleared(log, p);
    }
}
