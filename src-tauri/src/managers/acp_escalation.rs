//! Application seam between a finished dictation transcript and the ACP agent.
//!
//! CONTRACT SKELETON. Bodies are `todo!()` until step S1 (DESIGN.md rev 3.1, sections
//! 3, 3a, 3b). Tests: `acp_escalation_tests.rs`.

#![allow(unused_variables, dead_code)]

use crate::managers::acp_agent::{
    AcpAgentConfig, AcpAgentError, AcpAgentManager, AcpReply, PromptOptions,
};
use crate::managers::conversation::ConversationLog;

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
    todo!()
}

/// User-facing text for an error, or `None` when the outcome must stay silent
/// (`Stale`, `Closed`). Agent-provided detail is capped at 200 characters.
pub fn user_message_for(err: &AcpAgentError) -> Option<String> {
    todo!()
}

/// Clear-history seam: clears the log when present and ALWAYS resets the ACP session.
pub fn on_history_cleared(log: Option<&ConversationLog>, p: &dyn AcpPrompter) {
    todo!()
}

/// Context-setting seam: `enabled == false` behaves like `on_history_cleared`; `true` does nothing.
pub fn on_context_setting_changed(
    enabled: bool,
    log: Option<&ConversationLog>,
    p: &dyn AcpPrompter,
) {
    todo!()
}
