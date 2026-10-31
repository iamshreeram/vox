//! Start, adoption and cleanup decisions for the detached update runner (DESIGN
//! D2, R2-2, R2-3, R3-1). Launchd is reached only through the `JobControl`
//! trait, so every path here is testable with a fake.

use super::source_update_runner::{
    build_job_plist, ensure_state_dir, is_valid_run_id, job_label, log_path, plist_path, reconcile,
    rotate_logs, status_path, update_state_dir, validate_source_dir, write_plist_file, Decision,
    JobControl, JobParams, LockState, ReconcileInput, RunnerError, KEEP_RUN_LOGS,
};
use log::warn;
use std::path::{Path, PathBuf};

/// What `start_update` does, given whether this process already runs an update
/// and what the lock says (R2-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartGate {
    /// Nothing is running: launch a new run.
    Proceed,
    /// This process already runs a run: show its progress instead.
    Attach,
    /// Someone else holds the update lock: refuse.
    Refuse,
}

/// Start gate. A run this process owns is attached to; a live lock from anywhere
/// else is refused. A dead or unreadable lock is left alone here and blocks the
/// start later through the install blocker, so nothing is ever deleted.
pub fn start_gate(has_active_run: bool, lock: LockState) -> StartGate {
    if has_active_run {
        StartGate::Attach
    } else if lock == LockState::HeldAlive {
        StartGate::Refuse
    } else {
        StartGate::Proceed
    }
}

/// Inputs for one launch. Paths must be absolute; the plist builder checks them.
#[derive(Debug, Clone, Copy)]
pub struct LaunchInputs<'a> {
    /// Fresh run id (16 lowercase hex).
    pub run_id: &'a str,
    /// Vox application data directory.
    pub app_data: &'a Path,
    /// Checkout root containing `scripts/update-macos.sh`.
    pub source_dir: &'a Path,
    /// Full 40-hex commit to build.
    pub target_full: &'a str,
    /// Real home directory.
    pub home: &'a Path,
    /// Temporary directory (also the lock parent).
    pub tmpdir: &'a Path,
    /// Lock directory the script must use.
    pub lock_dir: &'a Path,
    /// Allow-listed proxy variables to forward.
    pub proxy_env: &'a [(String, String)],
}

/// What a successful launch created on disk and in launchd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launched {
    /// Job label (`com.iamshreeram.vox.update.<run_id>`).
    pub label: String,
    /// Update state directory.
    pub state_dir: PathBuf,
    /// Status file the script writes.
    pub status_file: PathBuf,
    /// The plist that was bootstrapped.
    pub plist: PathBuf,
}

/// Writes the plist for a new run (mode 0600), bootstraps the job once, and
/// returns what was created. Rotates old logs first, while no job is live, and
/// keeps room for the run being started so `KEEP_RUN_LOGS` runs remain. A failed
/// bootstrap removes the plist again. Never falls back to any other launcher.
pub fn launch(
    control: &dyn JobControl,
    inputs: &LaunchInputs<'_>,
) -> Result<Launched, RunnerError> {
    if !is_valid_run_id(inputs.run_id) {
        return Err(RunnerError::InvalidValue("run_id"));
    }
    validate_source_dir(inputs.source_dir)?;
    let state_dir = update_state_dir(inputs.app_data);
    ensure_state_dir(&state_dir)?;
    if let Err(error) = rotate_logs(&state_dir, KEEP_RUN_LOGS - 1) {
        warn!("could not rotate update logs: {error}");
    }
    let params = JobParams {
        run_id: inputs.run_id.to_string(),
        source_dir: inputs.source_dir.to_path_buf(),
        status_file: status_path(&state_dir),
        state_dir: state_dir.clone(),
        log_file: log_path(&state_dir, inputs.run_id),
        target_full: inputs.target_full.to_string(),
        home: inputs.home.to_path_buf(),
        tmpdir: inputs.tmpdir.to_path_buf(),
        lock_dir: inputs.lock_dir.to_path_buf(),
        proxy_env: inputs.proxy_env.to_vec(),
    };
    let xml = build_job_plist(&params)?;
    let plist = plist_path(&state_dir, inputs.run_id);
    write_plist_file(&plist, &xml)?;
    if let Err(error) = control.bootstrap(&plist) {
        remove_quietly(&plist);
        return Err(error);
    }
    Ok(Launched {
        label: job_label(inputs.run_id),
        status_file: params.status_file,
        state_dir,
        plist,
    })
}

/// Removes a run's registration, plist and old logs. Idempotent. Never touches
/// the lock and never runs while a process is live (callers check first).
/// Every step is attempted; the first error is returned.
pub fn cleanup_run(
    control: &dyn JobControl,
    state_dir: &Path,
    run_id: &str,
) -> Result<(), RunnerError> {
    if !is_valid_run_id(run_id) {
        return Err(RunnerError::InvalidValue("run_id"));
    }
    let booted = control.bootout(&job_label(run_id));
    remove_quietly(&plist_path(state_dir, run_id));
    let rotated = rotate_logs(state_dir, KEEP_RUN_LOGS).map_err(RunnerError::from);
    booted.and(rotated)
}

/// What startup adoption does with an existing run (R2-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptPlan {
    /// Leave it alone.
    Nothing,
    /// Take the run over: publish, watch and clean up through the poller.
    Watch,
    /// Nothing is watched any more: clean up now.
    CleanupNow,
}

/// Startup adoption. A finished run whose plist is already gone was cleaned up
/// by an earlier instance and stays quiet. Otherwise the reconcile table decides.
pub fn adopt_plan(input: &ReconcileInput<'_>, plist_exists: bool) -> AdoptPlan {
    let finished = input
        .status
        .as_ref()
        .is_some_and(|status| status.phase.is_terminal());
    if finished && !input.job.is_live() && !plist_exists {
        return AdoptPlan::Nothing;
    }
    match reconcile(input) {
        Decision::None => AdoptPlan::Nothing,
        Decision::CleanupFinished => AdoptPlan::CleanupNow,
        Decision::Adopt
        | Decision::PublishDone
        | Decision::PublishFailed(_)
        | Decision::MarkInterrupted(_) => AdoptPlan::Watch,
    }
}

fn remove_quietly(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warn!("could not remove an update plist: {error}");
        }
    }
}

#[cfg(test)]
#[path = "source_update_runner_run_tests.rs"]
mod tests;
