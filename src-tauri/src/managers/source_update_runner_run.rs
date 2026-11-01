//! Start, adoption and cleanup decisions for the detached update runner (DESIGN
//! D2, R2-2, R2-3, R3-1, R7-1, R8-2). Launchd is reached only through the
//! `JobControl` trait, so every path here is testable with a fake.

use super::source_update_auto::consent::AutoMarker;
use super::source_update_pending::{PendingRun, PendingState, PENDING_FILE_NAME};
#[cfg(test)]
use super::source_update_runner::build_job_plist;
use super::source_update_runner::{
    build_job_plist_kind, ensure_state_dir, is_valid_run_id, job_label, lock_state, log_path,
    plist_path, process_alive, reconcile, rotate_logs, status_path, update_state_dir,
    validate_source_dir, write_plist_file, Decision, JobControl, JobParams, LockState,
    ReconcileInput, RunKind, RunnerError, KEEP_RUN_LOGS,
};
use log::warn;
use std::fs;
use std::io;
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

/// Owned copy of the paths and proxies a job needs, for long-lived callers that
/// cannot hold [`LaunchInputs`] borrows (the commit environment).
#[derive(Debug, Clone)]
pub struct RunSite {
    /// Vox application data directory.
    pub app_data: PathBuf,
    /// Checkout root containing `scripts/update-macos.sh`.
    pub source_dir: PathBuf,
    /// Real home directory.
    pub home: PathBuf,
    /// Temporary directory.
    pub tmpdir: PathBuf,
    /// Lock directory the script must use.
    pub lock_dir: PathBuf,
    /// Allow-listed proxy variables to forward.
    pub proxy_env: Vec<(String, String)>,
}

impl RunSite {
    /// Borrowed inputs for `run_id` (and, for builds, the target given separately).
    pub fn inputs<'a>(&'a self, run_id: &'a str, target_full: &'a str) -> LaunchInputs<'a> {
        LaunchInputs {
            run_id,
            app_data: &self.app_data,
            source_dir: &self.source_dir,
            target_full,
            home: &self.home,
            tmpdir: &self.tmpdir,
            lock_dir: &self.lock_dir,
            proxy_env: &self.proxy_env,
        }
    }
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
    launch_kind(control, inputs, RunKind::All)
}

/// Launches the build phase of a run (DESIGN R7-1). When `auto` is set the
/// `run-<id>.auto` marker is written BEFORE the job is bootstrapped, and removed
/// again if the launch fails (fail closed). Refuses while a pending record, a
/// registered run plist or an update lock exists.
#[cfg_attr(not(test), allow(dead_code))]
pub fn launch_build(
    control: &dyn JobControl,
    inputs: &LaunchInputs<'_>,
    auto: bool,
) -> Result<Launched, RunnerError> {
    if !is_valid_run_id(inputs.run_id) {
        return Err(RunnerError::InvalidValue("run_id"));
    }
    validate_source_dir(inputs.source_dir)?;
    let state_dir = update_state_dir(inputs.app_data);
    ensure_state_dir(&state_dir)?;
    refuse_when_busy(&state_dir, inputs.lock_dir)?;
    let marker = AutoMarker::new(&state_dir);
    if auto {
        marker.write(inputs.run_id)?;
    }
    match launch_kind(control, inputs, RunKind::Build { origin_auto: auto }) {
        Ok(launched) => Ok(launched),
        Err(error) => {
            if auto {
                if let Err(remove) = marker.remove(inputs.run_id) {
                    warn!("could not remove an auto-update marker after a failed launch: {remove}");
                }
            }
            Err(error)
        }
    }
}

/// Writes the install plist for a committing record (mode 0600). The plist path
/// is fixed by the record and must not exist yet. Call before [`bootstrap_install`].
pub fn write_install_plist(
    inputs: &LaunchInputs<'_>,
    pending: &PendingRun,
) -> Result<PathBuf, RunnerError> {
    let PendingState::Committing(info) = &pending.state else {
        return Err(RunnerError::InvalidValue("install state"));
    };
    if inputs.run_id != info.install_run_id {
        return Err(RunnerError::InvalidValue("install run id"));
    }
    validate_source_dir(inputs.source_dir)?;
    let state_dir = update_state_dir(inputs.app_data);
    ensure_state_dir(&state_dir)?;
    if info.install_plist != plist_path(&state_dir, &info.install_run_id) {
        return Err(RunnerError::InvalidPath("install_plist"));
    }
    if fs::symlink_metadata(&info.install_plist).is_ok() {
        return Err(RunnerError::Busy("install plist already exists"));
    }
    let params = job_params(inputs, &state_dir, pending.target_full.as_str());
    let xml = build_job_plist_kind(&params, RunKind::Install { pending })?;
    write_plist_file(&info.install_plist, &xml)?;
    Ok(info.install_plist.clone())
}

/// Registers the install job with launchd (`launchctl bootstrap gui/<uid> <plist>`).
pub fn bootstrap_install(
    control: &dyn JobControl,
    pending: &PendingRun,
) -> Result<(), RunnerError> {
    let PendingState::Committing(info) = &pending.state else {
        return Err(RunnerError::InvalidValue("install state"));
    };
    control.bootstrap(&info.install_plist)
}

/// Removes the install plist of a committing record. Idempotent; never fails.
pub fn remove_install_plist(pending: &PendingRun) {
    if let PendingState::Committing(info) = &pending.state {
        remove_quietly(&info.install_plist);
    }
}

/// Boots the install label out of launchd. Idempotent (an absent label is success).
pub fn bootout_install(control: &dyn JobControl, pending: &PendingRun) -> Result<(), RunnerError> {
    let PendingState::Committing(info) = &pending.state else {
        return Err(RunnerError::InvalidValue("install state"));
    };
    control.bootout(&info.install_label)
}

/// Shared launch body for every phase.
fn launch_kind(
    control: &dyn JobControl,
    inputs: &LaunchInputs<'_>,
    kind: RunKind<'_>,
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
    let params = job_params(inputs, &state_dir, inputs.target_full);
    let xml = build_job_plist_kind(&params, kind)?;
    let plist = plist_path(&state_dir, inputs.run_id);
    write_plist_file(&plist, &xml)?;
    if let Err(error) = control.bootstrap(&plist) {
        remove_quietly(&plist);
        return Err(error);
    }
    Ok(Launched {
        label: job_label(inputs.run_id),
        status_file: status_path(&state_dir),
        state_dir,
        plist,
    })
}

fn job_params(inputs: &LaunchInputs<'_>, state_dir: &Path, target_full: &str) -> JobParams {
    JobParams {
        run_id: inputs.run_id.to_string(),
        source_dir: inputs.source_dir.to_path_buf(),
        status_file: status_path(state_dir),
        state_dir: state_dir.to_path_buf(),
        log_file: log_path(state_dir, inputs.run_id),
        target_full: target_full.to_string(),
        home: inputs.home.to_path_buf(),
        tmpdir: inputs.tmpdir.to_path_buf(),
        lock_dir: inputs.lock_dir.to_path_buf(),
        proxy_env: inputs.proxy_env.to_vec(),
    }
}

/// Refuses a new run while a pending record, a registered run plist or any update
/// lock exists. Read-only: nothing is removed here.
#[cfg_attr(not(test), allow(dead_code))]
fn refuse_when_busy(state_dir: &Path, lock_dir: &Path) -> Result<(), RunnerError> {
    if fs::symlink_metadata(state_dir.join(PENDING_FILE_NAME)).is_ok() {
        return Err(RunnerError::Busy("a pending update exists"));
    }
    if lock_state(lock_dir, &process_alive) != LockState::Free {
        return Err(RunnerError::Busy("the update lock is present"));
    }
    for entry in fs::read_dir(state_dir)? {
        let name = entry?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("run-") && name.ends_with(".plist") {
            return Err(RunnerError::Busy("an update run is registered"));
        }
    }
    Ok(())
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
        Decision::None | Decision::Rest => AdoptPlan::Nothing,
        Decision::CleanupFinished => AdoptPlan::CleanupNow,
        Decision::Adopt
        | Decision::PublishDone
        | Decision::PublishFailed(_)
        | Decision::MarkInterrupted(_) => AdoptPlan::Watch,
    }
}

fn remove_quietly(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            warn!("could not remove an update plist: {error}");
        }
    }
}

#[cfg(test)]
#[path = "source_update_runner_run_tests.rs"]
mod tests;
