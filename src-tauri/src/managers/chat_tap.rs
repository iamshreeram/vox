//! Platform tap for the chat window (DESIGN-CHAT Rev 6, section 2.2). The conversation loop's
//! recording hooks end here: every call is forwarded to the shared `ChatLog`, which owns the
//! worker, the bounds and the emission. Stub: bodies are `unimplemented!()` until the
//! implementation step lands; `chat_tap_tests.rs` is the authoritative spec.
#![allow(dead_code)]

use crate::managers::chat_log::{ChatLog, ChatRole};
use std::sync::Arc;

/// Forwards conversation events into the chat log. Holds no state of its own.
pub struct ChatTap {
    log: Arc<ChatLog>,
}

impl ChatTap {
    /// Wraps the shared log.
    pub fn new(_log: Arc<ChatLog>) -> Self {
        unimplemented!("ChatTap::new: not implemented yet (red spec)")
    }

    /// Session start: `reset()` plus a System entry `system:started`.
    pub fn on_session_start(&self) {
        unimplemented!("ChatTap::on_session_start: not implemented yet (red spec)")
    }

    /// One recorded line (User or Assistant), already cleaned by the log.
    pub fn on_chat(&self, _role: ChatRole, _text: &str) {
        unimplemented!("ChatTap::on_chat: not implemented yet (red spec)")
    }

    /// Conversation state change; an `idle` state with an end label also records
    /// `system:end:<label>`.
    pub fn on_state(&self, _state: &str, _end: Option<&str>) {
        unimplemented!("ChatTap::on_state: not implemented yet (red spec)")
    }
}
