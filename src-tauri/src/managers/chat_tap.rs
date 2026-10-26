//! Platform tap for the chat window (DESIGN-CHAT Rev 6, section 2.2). The conversation loop's
//! recording hooks end here: every call is forwarded to the shared `ChatLog`, which owns the
//! worker, the bounds and the emission. Holds no state of its own; no I/O, no logging.

use crate::managers::chat_log::{ChatLog, ChatRole};
use std::sync::Arc;

/// Forwards conversation events into the chat log. Holds no state of its own.
pub struct ChatTap {
    log: Arc<ChatLog>,
}

impl ChatTap {
    /// Wraps the shared log.
    pub fn new(log: Arc<ChatLog>) -> Self {
        ChatTap { log }
    }

    /// Session start: `reset()` plus a System entry `system:started`.
    pub fn on_session_start(&self) {
        self.log.reset();
        self.log.say(ChatRole::System, "system:started");
    }

    /// One recorded line (User or Assistant), already cleaned by the log.
    pub fn on_chat(&self, role: ChatRole, text: &str) {
        self.log.say(role, text);
    }

    /// Conversation state change; an end label also records `system:end:<label>`.
    pub fn on_state(&self, state: &str, end: Option<&str>) {
        self.log.set_state(state, end);
        if let Some(label) = end {
            self.log
                .say(ChatRole::System, &format!("system:end:{label}"));
        }
    }
}
