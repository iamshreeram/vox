//! `job_status` read-only tool (M3 S3c-2, DESIGN D7): the injectable status source.
//!
//! Phase A stub: the trait is final; the tool wiring in `tools.rs` is `todo!()`.

#![allow(dead_code, unused_variables)]

use crate::managers::agent_jobs::JobSnapshot;

/// Source of the coding job snapshot for `job_status` (like `ConversationSource`).
/// `None` means no job has run yet.
pub trait JobStatusSource: Send + Sync {
    /// Returns the current or most recent job snapshot, if any.
    fn status(&self) -> Option<JobSnapshot>;
}

#[cfg(test)]
#[path = "job_status_tests.rs"]
mod job_status_tests;
