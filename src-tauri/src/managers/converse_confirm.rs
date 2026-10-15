//! Spoken confirmation gate and the typed action preparation that feeds it
//! (M3). Scaffolding only: bodies are filled by later M3 steps.

#![allow(dead_code, unused_imports, unused_variables)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::managers::command_router::CommandAction;
use crate::managers::converse::{Ears, Hands, Mouth};
pub use crate::managers::converse_plan::JobRequest;
use crate::managers::converse_plan::TurnPlan;
use crate::managers::job_announcer::Announcer;
use crate::settings::AppSettings;

/// Handle type for a pinned project directory (unix: an owned descriptor).
#[cfg(unix)]
pub type DirFd = std::os::fd::OwnedFd;

/// Handle type for a pinned project directory (non-unix placeholder).
#[cfg(not(unix))]
pub struct DirFd;

/// Classification of a spoken yes/no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The user approved.
    Yes,
    /// The user refused.
    No,
    /// The answer was not clear.
    Unclear,
}

/// Classifies a transcribed answer into a [`Verdict`].
pub fn classify_confirmation(text: &str) -> Verdict {
    todo!()
}

/// Why a confirmation did not approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclineReason {
    /// The user said no.
    No,
    /// The answer was not clear.
    Unclear,
    /// Nothing was heard in time.
    Silence,
    /// The session was cancelled.
    Cancelled,
    /// A speech or capture error occurred.
    Error,
}

/// Outcome of a confirmation prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The user approved the action.
    Approved,
    /// The action was declined, with the reason.
    Declined(DeclineReason),
}

/// Timing knobs of the confirmation gate.
pub struct ConfirmConfig {
    /// How long to wait for the speaker to go quiet.
    pub quiet_wait: Duration,
    /// How long to listen for the answer.
    pub listen_timeout: Duration,
    /// Guard after speech ends, before the microphone is armed.
    pub post_speech_guard: Duration,
}

impl Default for ConfirmConfig {
    fn default() -> Self {
        ConfirmConfig {
            quiet_wait: Duration::from_secs(2),
            listen_timeout: Duration::from_secs(8),
            post_speech_guard: Duration::from_millis(350),
        }
    }
}

/// Asks a yes/no question aloud and waits for the answer.
pub async fn confirm_action(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    question: &str,
    cfg: &ConfirmConfig,
) -> Decision {
    todo!()
}

/// Which action paths the current settings allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPolicy {
    /// Whether the confirmation gate is active.
    pub gate_active: bool,
    /// Whether background jobs may be started.
    pub jobs_allowed: bool,
}

/// Derives the [`ActionPolicy`] from the settings.
pub fn effective_policy(settings: &AppSettings) -> ActionPolicy {
    todo!()
}

/// Identity of a file on disk, used to detect replaced binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileId {
    /// Device number.
    pub dev: u64,
    /// Inode number.
    pub ino: u64,
    /// Length in bytes.
    pub len: u64,
    /// Modification time in whole seconds.
    pub mtime_secs: i64,
}

/// Launch settings for the configured coding CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    /// Path to the coding CLI binary.
    pub binary: PathBuf,
    /// Identity of the binary at approval time.
    pub binary_id: FileId,
    /// Flag that precedes the task prompt.
    pub prompt_flag: String,
    /// Maximum run time of a job.
    pub timeout: Duration,
}

/// Snapshot of everything an approval depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalFingerprint {
    /// Gate state at approval time.
    pub gate_active: bool,
    /// Job permission at approval time.
    pub jobs_allowed: bool,
    /// Launch settings at approval time, if any.
    pub launch: Option<LaunchConfig>,
}

/// Live source of the action policy and launch settings.
pub trait PolicySource: Send + Sync {
    /// The current action policy.
    fn current(&self) -> ActionPolicy;
    /// The current launch settings, if jobs are configured.
    fn launch_config(&self) -> Option<LaunchConfig>;
}

/// A model-proposed command that passed validation.
pub struct ValidatedCommand {
    /// Short label spoken in confirmations.
    pub label: String,
    /// The validated action (the value returned by `restrict_model_action`).
    pub(crate) action: CommandAction,
}

/// A fully prepared background job, ready to start.
pub struct JobLaunch {
    /// Project the job runs in.
    pub project: String,
    /// Pinned project directory handle.
    pub dir: DirFd,
    /// Task text for the coding CLI.
    pub task: String,
    /// Launch settings captured at preparation time.
    pub config: LaunchConfig,
}

/// An action that is validated and ready to execute after approval.
pub enum PreparedAction {
    /// Paste the given text.
    Paste {
        /// Text to paste.
        text: String,
    },
    /// Run a validated command.
    Command {
        /// The validated command.
        cmd: ValidatedCommand,
    },
    /// Start a background job.
    Job {
        /// The prepared launch.
        launch: JobLaunch,
    },
}

/// Why an action could not be prepared.
pub enum PrepareError {
    /// Refused, with fixed spoken text.
    Refused(String),
    /// The action kind is not supported.
    Unsupported,
}

/// Validates a plan and prepares the action it asks for.
pub async fn prepare_action(
    plan: &TurnPlan,
    policy: &dyn PolicySource,
    hands: &dyn Hands,
) -> Result<PreparedAction, PrepareError> {
    todo!()
}

/// Captures the approval fingerprint from a policy source.
pub fn fingerprint_of(policy: &dyn PolicySource) -> ApprovalFingerprint {
    todo!()
}

/// Why a project could not be pinned.
pub enum PinError {
    /// The name matched no configured project.
    Unknown,
    /// The name matched more than one project.
    Ambiguous,
    /// Pinning is not available on this platform or configuration.
    Unavailable,
}

/// Outcome of asking to start a background job.
pub enum JobStartOutcome {
    /// Started; carries the job id.
    Started(u64),
    /// Another job is already active.
    Busy,
    /// Jobs are not configured.
    NotConfigured,
    /// The project name is unknown.
    UnknownProject,
    /// Starting jobs is not supported here.
    Unsupported,
    /// Refused, with fixed spoken text.
    Refused(String),
}

/// Fixed question spoken before a prepared action runs (M3 S1 stub).
pub fn confirmation_question(action: &PreparedAction) -> String {
    todo!()
}

/// Fixed spoken reply after a decline (M3 S1 stub).
pub fn decline_message(reason: DeclineReason) -> &'static str {
    todo!()
}

/// True when `path` is an absolute, existing, executable regular file (M3 S1 stub).
pub fn binary_valid(path: &Path) -> bool {
    todo!()
}

/// Launch settings from the current settings, or `None` if the binary is not valid (M3 S1 stub).
pub fn launch_config_from_settings(settings: &AppSettings) -> Option<LaunchConfig> {
    todo!()
}

#[cfg(test)]
#[path = "converse_confirm_tests.rs"]
mod converse_confirm_tests;

#[cfg(test)]
#[path = "converse_confirm_adapter_tests.rs"]
mod converse_confirm_adapter_tests;
