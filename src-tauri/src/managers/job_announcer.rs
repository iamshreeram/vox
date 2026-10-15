//! Low-priority speech for background jobs and the confirmation guard (M3).
//! Scaffolding only: bodies are filled by later M3 steps.

#![allow(dead_code, unused_variables)]

use std::sync::Arc;

/// A mouth that can speak only when nothing else is speaking.
pub trait LowPriorityMouth: Send + Sync {
    /// Speaks `text` only if the speaker is idle. Returns `true` when it was spoken.
    fn try_speak_if_idle(&self, text: &str) -> bool;
}

/// Kind of a background announcement.
pub enum AnnounceKind {
    /// A progress update.
    Progress,
    /// The job finished.
    Finish,
}

/// Queues background announcements and speaks them when the speaker is idle.
pub struct Announcer;

impl Announcer {
    /// Creates an announcer that speaks through `mouth`.
    pub fn new(mouth: Arc<dyn LowPriorityMouth>) -> Self {
        todo!()
    }

    /// Queues one announcement (speaks now if idle, otherwise later).
    pub fn announce(&self, kind: AnnounceKind, text: String) {
        todo!()
    }

    /// Marks a conversation as active or not (announcements wait while active).
    pub fn set_conversation_active(&self, active: bool) {
        todo!()
    }

    /// Holds speech while a confirmation is in progress. Released on drop.
    pub fn confirmation_guard(&self) -> ConfirmationGuard {
        todo!()
    }

    /// Removes and returns the queued announcements.
    pub fn take_pending(&self) -> Vec<String> {
        todo!()
    }

    /// Speaks queued announcements now that the speaker is idle.
    pub fn notify_idle(&self) {
        todo!()
    }
}

/// RAII hold on announcements during a confirmation.
pub struct ConfirmationGuard;
