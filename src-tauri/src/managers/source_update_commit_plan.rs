//! Pure startup, adoption and poll planner for the auto-update (R7-3, R8-2, R9-2, R9-3).
//!
//! The planner only decides. It never releases a guard, deletes a file or
//! touches launchd. `Unknown` never leads to a release or a delete: it always
//! means "maybe alive, keep admission closed".

use super::source_update_runner::JobProbe;

/// What is on disk for `pending.json`, as classified by the loader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingInput {
    /// No file.
    Absent,
    /// Present but unreadable, oversize, symlinked or invalid.
    Invalid,
    /// Valid, state `built`.
    Built,
    /// Valid, state `committing`.
    Committing,
}

/// Persisted status of the install run, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusInput {
    /// No status for the install run.
    None,
    /// The install run is still running or stopped without a terminal status.
    NonTerminal,
    /// The install run finished, either done or failed.
    Terminal,
}

/// Everything the planner looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryInput {
    /// The pending record.
    pub pending: PendingInput,
    /// Probe of the install label. Ignored unless `pending` is `Committing`.
    pub probe: JobProbe,
    /// Status of the install run. Ignored unless `pending` is `Committing`.
    pub install_status: StatusInput,
}

/// What the caller must do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// No pending record: nothing to do.
    NothingToDo,
    /// Resting `built` record: keep it, hold no admission, delete nothing.
    RestBuilt,
    /// An install is or may be live: adopt it. `hold_admission` is always true
    /// here; the caller holds admission until the install is terminal or gone.
    AdoptInstall {
        /// Keep `Owner::Updater` and the agent reservation.
        hold_admission: bool,
    },
    /// Committing, the label is conclusively absent and no status is running:
    /// revoke through [`super::source_update_commit::revoke_install`], then clean up.
    RevokeInstall,
    /// Committing with a terminal status and no live process: release admission,
    /// then remove the pending record and its staged directory.
    FinishInstall,
    /// Invalid pending record: remove it and its staged directory, record a failure.
    DiscardInvalid,
}

/// Plans the next recovery step. Pure and total.
pub fn plan_recovery(input: &RecoveryInput) -> RecoveryAction {
    match input.pending {
        PendingInput::Absent => RecoveryAction::NothingToDo,
        PendingInput::Invalid => RecoveryAction::DiscardInvalid,
        PendingInput::Built => RecoveryAction::RestBuilt,
        PendingInput::Committing => plan_committing(input.probe, input.install_status),
    }
}

/// Plans recovery for a `committing` record.
fn plan_committing(probe: JobProbe, status: StatusInput) -> RecoveryAction {
    let adopt = RecoveryAction::AdoptInstall {
        hold_admission: true,
    };
    match status {
        StatusInput::NonTerminal => adopt,
        StatusInput::Terminal => match probe {
            // Finished and no process left: release is safe.
            JobProbe::Absent | JobProbe::Registered { pid: None, .. } => {
                RecoveryAction::FinishInstall
            }
            // A process may still be live, or the state is unknown.
            JobProbe::Registered { pid: Some(_), .. } | JobProbe::Unknown => adopt,
        },
        StatusInput::None => match probe {
            JobProbe::Absent => RecoveryAction::RevokeInstall,
            JobProbe::Registered { .. } | JobProbe::Unknown => adopt,
        },
    }
}

#[cfg(test)]
#[path = "source_update_commit_plan_tests.rs"]
mod tests;
