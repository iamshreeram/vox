//! Launch checks and process spawning for background coding jobs (S2b, R2-4, R3-2).
//! Unix only. The approved binary is re-checked against its FileId, the project
//! directory is a pinned handle, and the child changes into that handle with
//! fchdir in pre_exec, so a renamed or replaced path cannot move the job.

use std::fs::{File, Metadata};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use super::JobError;
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};

/// Builds the identity of a file from its metadata.
pub(super) fn file_id_of(meta: &Metadata) -> FileId {
    FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_secs: meta.mtime(),
    }
}

/// Checks that the approved binary is still an absolute, executable regular
/// file whose identity matches the approval.
pub(super) fn check_binary(config: &LaunchConfig) -> Result<(), JobError> {
    let path: &Path = &config.binary;
    if !path.is_absolute() {
        return Err(JobError::BadBinary);
    }
    let meta = std::fs::metadata(path).map_err(|_| JobError::BadBinary)?;
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Err(JobError::BadBinary);
    }
    if file_id_of(&meta) != config.binary_id {
        return Err(JobError::BadBinary);
    }
    Ok(())
}

/// Checks that the pinned project handle still refers to a directory.
pub(super) fn check_dir(fd: &OwnedFd) -> Result<(), JobError> {
    let clone = fd.try_clone().map_err(|_| JobError::BadDir)?;
    let meta = File::from(clone).metadata().map_err(|_| JobError::BadDir)?;
    if meta.is_dir() {
        Ok(())
    } else {
        Err(JobError::BadDir)
    }
}

/// Refuses an agent selection the configured CLI cannot receive: an agent was
/// chosen but no agent flag is configured. Uses the existing BadBinary variant
/// (the CLI is not usable for this start), so no new error variant is needed.
pub(super) fn check_agent(launch: &JobLaunch) -> Result<(), JobError> {
    match (&launch.selection.agent, &launch.config.agent_flag) {
        (Some(_), None) => Err(JobError::BadBinary),
        _ => Ok(()),
    }
}

/// Spawns the coding CLI as the leader of a new process group. The argv is
/// `[binary, agent_flag, agent?, prompt_flag, task]` with no shell; the agent
/// pair is present only when an agent is selected. stdin is null; stdout and
/// stderr are piped; the working directory is the pinned project handle.
pub(super) fn spawn_job(launch: &JobLaunch) -> Result<Child, JobError> {
    let dir_fd = launch.dir.as_raw_fd();
    let mut cmd = Command::new(&launch.config.binary);
    if let (Some(agent), Some(flag)) = (&launch.selection.agent, &launch.config.agent_flag) {
        cmd.arg(flag).arg(agent);
    }
    cmd.arg(&launch.config.prompt_flag)
        .arg(&launch.task)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // SAFETY: the pre_exec closure runs in the forked child before exec and calls
    // only fchdir, which is async-signal-safe. `dir_fd` is owned by `launch`, which
    // outlives this spawn call, so the descriptor is valid while the closure runs.
    // A failing fchdir returns Err, so the child exits before exec and spawn fails.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fchdir(dir_fd) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    cmd.spawn().map_err(|_| JobError::SpawnFailed)
}
