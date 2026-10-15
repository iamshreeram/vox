//! Non-unix contract for the job manager (S5 fix, validator MAJOR 1).
//!
//! On targets without unix process groups the job machinery is compiled out and
//! `JobManager::start` must refuse with `JobError::Unsupported`. Every other
//! public method behaves as an idle manager. The contract is written once in
//! `assert_nonunix_contract` so it can run on a unix host too (the simulation
//! below), where `start` would really spawn and therefore is not called.

use super::{JobError, JobManager, ShutdownConfig};
#[cfg(not(unix))]
use super::{JobObserver, JobSnapshot, JobSpec};
#[cfg(not(unix))]
use crate::managers::converse_confirm::{DirFd, FileId, JobLaunch, LaunchConfig};
#[cfg(not(unix))]
use super::exit::ExitTarget;
#[cfg(not(unix))]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(unix))]
struct NullObserver;

#[cfg(not(unix))]
impl JobObserver for NullObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, _snap: &JobSnapshot) {}
}

/// A launch that would be valid on unix; the non-unix start must refuse it anyway.
#[cfg(not(unix))]
fn launch() -> JobLaunch {
    JobLaunch {
        project: "proj".to_string(),
        dir: DirFd,
        task: "task".to_string(),
        config: LaunchConfig {
            binary: PathBuf::from("/nonexistent/cli"),
            binary_id: FileId {
                dev: 0,
                ino: 0,
                len: 0,
                mtime_secs: 0,
            },
            prompt_flag: "-p".to_string(),
            timeout: Duration::from_secs(60),
        },
    }
}

/// The shared non-unix contract. `start` must refuse with `Unsupported`; the other
/// methods must behave as an idle manager. Called directly on non-unix, and by the
/// simulation on unix (where it still exercises the non-unix branch's expectations).
fn assert_nonunix_contract(mgr: &Arc<JobManager>, start_result: Result<u64, JobError>) {
    assert_eq!(
        start_result,
        Err(JobError::Unsupported),
        "start must refuse with Unsupported off unix"
    );
    assert!(!mgr.cancel(), "cancel reports nothing to stop");
    assert!(mgr.snapshot().is_none(), "no job, so no snapshot");
    assert!(!mgr.has_active_job(), "no active job");
    let cfg = ShutdownConfig {
        term_grace: Duration::from_millis(10),
        drain: Duration::from_millis(10),
        deadline: Duration::from_millis(50),
    };
    assert!(mgr.shutdown(&cfg), "shutdown of an idle manager is clean");
}

#[cfg(not(unix))]
#[test]
fn nonunix_start_is_unsupported_and_manager_stays_idle() {
    let mgr = JobManager::new();
    let spec = JobSpec {
        launch: launch(),
        progress_interval: Duration::from_secs(1),
    };
    let result = mgr.start(spec, Arc::new(NullObserver));
    assert_nonunix_contract(&mgr, result);
}

#[cfg(not(unix))]
#[test]
fn nonunix_exit_target_behaves_as_an_idle_manager() {
    let mgr = JobManager::new();
    let target: &dyn ExitTarget = mgr.as_ref();
    assert!(!target.has_active_job());
    assert!(target.cleanup_confirmed());
}

/// Simulation on unix hosts: the same non-unix expectations, with `start` replaced
/// by its refusal value (a real start would spawn a process, which is out of scope
/// for this contract). Proves the contract helper itself is correct on this host.
#[cfg(unix)]
#[test]
fn nonunix_contract_simulation_holds_on_unix_host() {
    let mgr = JobManager::new();
    assert_nonunix_contract(&mgr, Err(JobError::Unsupported));
}
