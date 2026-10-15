//! `job_status` read-only tool (M3 S3c-2, DESIGN D7): the injectable status source.
//!
//! The source trait is injected into the `job_status` tool, which reports the
//! current or most recent coding job snapshot.

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
