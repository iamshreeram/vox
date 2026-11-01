//! RED tests for M3 S3c-1 (DESIGN Rev 5: D8, R2-3, R2-12, R3-2, D3). Phase A: these
//! compile against the todo!() stubs and fail at runtime. Included from converse_platform.rs.
//! Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::agent_jobs::{JobError, JobManager, JobObserver, JobSnapshot};
use crate::managers::agent_tools::project::ProjectResolver;
use crate::managers::converse_confirm::{
    ActionPolicy, DirFd, FileId, JobLaunch, JobStartOutcome, LaunchConfig, PinError, PolicySource,
};
use crate::managers::converse_plan::{
    JobRequest, PlanInput, PlanKind, PlanPrompt, TurnPlan, JOB_PLAN_ADDENDUM,
};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::LowPriorityMouth;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A fresh temp directory removed on drop.
pub(super) struct TempDir(PathBuf);

impl TempDir {
    pub(super) fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vox-m3-platform-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }
    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn configured(roots: &[&Path]) -> Vec<String> {
    roots
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[cfg(unix)]
fn ino_of(path: &Path) -> (u64, u64) {
    let meta = std::fs::metadata(path).expect("stat");
    (meta.dev(), meta.ino())
}

#[cfg(unix)]
#[cfg(unix)]
fn fd_ino(fd: &DirFd) -> (u64, u64) {
    let file = std::fs::File::from(fd.try_clone().expect("dup fd"));
    let meta = file.metadata().expect("fstat fd");
    (meta.dev(), meta.ino())
}

#[cfg(unix)]
pub(super) fn file_id_of(path: &Path) -> FileId {
    // A missing binary gets a zero identity: the spawn-time check must reject it, not the helper.
    let Ok(meta) = std::fs::metadata(path) else {
        return FileId {
            dev: 0,
            ino: 0,
            len: 0,
            mtime_secs: 0,
        };
    };
    FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_secs: meta.mtime(),
    }
}

/// Non-unix twin: identities come from unix inode metadata, so every file gets the zero
/// identity here. Only unix builds run jobs, so this value is never used for a launch.
#[cfg(not(unix))]
pub(super) fn file_id_of(path: &Path) -> FileId {
    let _ = path;
    FileId {
        dev: 0,
        ino: 0,
        len: 0,
        mtime_secs: 0,
    }
}

/// Writes an executable sh script (fake coding CLI) into `dir`.
#[cfg(unix)]
pub(super) fn write_fake_cli(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-cli");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Non-unix twin: no fake CLI can run off unix, so the path is returned unwritten.
#[cfg(not(unix))]
pub(super) fn write_fake_cli(dir: &Path, _body: &str) -> PathBuf {
    dir.join("fake-cli")
}

// ---------------------------------------------------------------------------------------
// pin_project_in: resolution table and the pinned inode (D8, R2-2, M3-10)
// ---------------------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn pin_exact_name_returns_the_projects_directory() {
    let root = TempDir::new("pin-exact");
    let proj = root.path().join("alpha");
    std::fs::create_dir(&proj).expect("mkdir");
    let resolver = ProjectResolver::new(&configured(&[root.path()]));
    let fd = pin_project_in(&resolver, "alpha").expect("exact name must pin");
    assert_eq!(
        fd_ino(&fd),
        ino_of(&proj),
        "pinned fd must be the project directory"
    );
}

#[cfg(unix)]
#[test]
fn pin_is_still_the_same_inode_after_the_path_is_renamed() {
    let root = TempDir::new("pin-rename");
    let proj = root.path().join("alpha");
    std::fs::create_dir(&proj).expect("mkdir");
    let before = ino_of(&proj);
    let resolver = ProjectResolver::new(&configured(&[root.path()]));
    let fd = pin_project_in(&resolver, "alpha").expect("pin");
    std::fs::rename(&proj, root.path().join("alpha-moved")).expect("rename");
    std::fs::create_dir(&proj).expect("create replacement at the old path");
    assert_eq!(
        fd_ino(&fd),
        before,
        "pinned fd must keep the original inode"
    );
    assert_ne!(
        ino_of(&proj),
        before,
        "the path now names a different directory"
    );
}

#[cfg(unix)]
#[test]
fn pin_unknown_name_is_unknown() {
    let root = TempDir::new("pin-unknown");
    std::fs::create_dir(root.path().join("alpha")).expect("mkdir");
    let resolver = ProjectResolver::new(&configured(&[root.path()]));
    assert!(matches!(
        pin_project_in(&resolver, "missing"),
        Err(PinError::Unknown)
    ));
}

#[cfg(unix)]
#[test]
fn pin_name_found_in_two_roots_is_ambiguous() {
    let a = TempDir::new("pin-amb-a");
    let b = TempDir::new("pin-amb-b");
    std::fs::create_dir(a.path().join("dup")).expect("mkdir a");
    std::fs::create_dir(b.path().join("dup")).expect("mkdir b");
    let resolver = ProjectResolver::new(&configured(&[a.path(), b.path()]));
    assert!(matches!(
        pin_project_in(&resolver, "dup"),
        Err(PinError::Ambiguous)
    ));
}

#[cfg(unix)]
#[test]
fn pin_symlinked_project_escaping_the_root_is_refused() {
    let root = TempDir::new("pin-escape-root");
    let outside = TempDir::new("pin-escape-outside");
    std::os::unix::fs::symlink(outside.path(), root.path().join("evil")).expect("symlink");
    let resolver = ProjectResolver::new(&configured(&[root.path()]));
    let result = pin_project_in(&resolver, "evil");
    assert!(
        matches!(result, Err(PinError::Unknown) | Err(PinError::Unavailable)),
        "a symlink to outside the root must never pin"
    );
}

#[cfg(unix)]
#[test]
fn pin_with_no_roots_is_unavailable() {
    let resolver = ProjectResolver::new(&[]);
    assert!(matches!(
        pin_project_in(&resolver, "alpha"),
        Err(PinError::Unavailable)
    ));
}

#[cfg(unix)]
#[test]
fn pin_with_only_relative_or_missing_roots_is_unavailable() {
    let resolver = ProjectResolver::new(&["relative/root".to_string()]);
    assert!(matches!(
        pin_project_in(&resolver, "alpha"),
        Err(PinError::Unavailable)
    ));
}

#[cfg(unix)]
#[test]
fn pin_invalid_project_name_is_refused_not_resolved() {
    let root = TempDir::new("pin-badname");
    std::fs::create_dir(root.path().join("alpha")).expect("mkdir");
    let resolver = ProjectResolver::new(&configured(&[root.path()]));
    for bad in ["", "../alpha", "a/b", ".."] {
        assert!(
            pin_project_in(&resolver, bad).is_err(),
            "must refuse name {bad:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// start_job_on / map_job_error: outcome mapping (D8, M3-10)
// ---------------------------------------------------------------------------------------

#[test]
fn map_ok_id_is_started_with_that_id() {
    assert!(matches!(map_job_error(Ok(7)), JobStartOutcome::Started(7)));
}

#[test]
fn map_busy_is_busy() {
    assert!(matches!(
        map_job_error(Err(JobError::Busy)),
        JobStartOutcome::Busy
    ));
}

#[test]
fn map_bad_binary_and_bad_dir_are_not_configured() {
    assert!(matches!(
        map_job_error(Err(JobError::BadBinary)),
        JobStartOutcome::NotConfigured
    ));
    assert!(matches!(
        map_job_error(Err(JobError::BadDir)),
        JobStartOutcome::NotConfigured
    ));
}

#[test]
fn map_unsupported_is_unsupported() {
    assert!(matches!(
        map_job_error(Err(JobError::Unsupported)),
        JobStartOutcome::Unsupported
    ));
}

#[test]
fn map_spawn_failure_is_the_fixed_refusal_text() {
    match map_job_error(Err(JobError::SpawnFailed)) {
        JobStartOutcome::Refused(text) => {
            assert_eq!(text, "The coding job couldn't start.");
        }
        _ => panic!("spawn failure must be Refused with the fixed text"),
    }
}

#[test]
fn map_shutting_down_is_the_fixed_closing_refusal() {
    match map_job_error(Err(JobError::ShuttingDown)) {
        JobStartOutcome::Refused(text) => {
            assert_eq!(text, "Vox is closing, so I didn't start it.");
        }
        _ => panic!("shutdown must be Refused with the closing text"),
    }
}

// B2a: a start refused by the update reservation speaks the fixed update text.
#[test]
fn map_update_in_progress_is_the_fixed_update_refusal() {
    match map_job_error(Err(JobError::UpdateInProgress)) {
        JobStartOutcome::Refused(text) => assert_eq!(text, UPDATE_BUSY),
        _ => panic!("update in progress must be Refused with the update text"),
    }
}

// B2a: every gate holder maps to its own refusal text; the update holder maps to UPDATE_BUSY.
#[test]
fn admit_refusal_maps_each_holder_to_its_message() {
    assert_eq!(admit_refusal(&Busy(Owner::Dictation)), DICTATION_BUSY);
    assert_eq!(admit_refusal(&Busy(Owner::Conversation)), ALREADY_RUNNING);
    assert_eq!(admit_refusal(&Busy(Owner::Enrollment)), ENROLLMENT_BUSY);
    assert_eq!(admit_refusal(&Busy(Owner::Updater)), UPDATE_BUSY);
    assert_eq!(
        UPDATE_BUSY,
        "Vox is installing an update. Try again in a minute."
    );
}

struct NullObserver;

impl JobObserver for NullObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, _snap: &JobSnapshot) {}
}

fn launch_in(dir: &TempDir, binary: &Path, task: &str) -> JobLaunch {
    let fd: DirFd = std::fs::File::open(dir.path()).expect("open dir").into();
    JobLaunch {
        project: "proj".to_string(),
        dir: fd,
        task: task.to_string(),
        config: LaunchConfig {
            binary: binary.to_path_buf(),
            binary_id: file_id_of(binary),
            prompt_flag: "-p".to_string(),
            agent_flag: None,
            timeout: Duration::from_secs(60),
        },
        selection: Default::default(),
    }
}

/// Cancels the manager's job when dropped, so a failing assertion never leaves the
/// `sleep 15` fake CLI group running (cancel is a no-op when idle).
struct CancelOnDrop(Arc<JobManager>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(unix)]
#[test]
fn start_without_a_manager_is_unsupported() {
    let dir = TempDir::new("start-none");
    let bin = write_fake_cli(dir.path(), "sleep 1");
    let outcome = start_job_on(None, launch_in(&dir, &bin, "task"), Arc::new(NullObserver));
    assert!(matches!(outcome, JobStartOutcome::Unsupported));
}

#[cfg(unix)]
#[test]
fn start_with_a_manager_starts_then_second_start_is_busy() {
    let dir = TempDir::new("start-busy");
    // Sleeps long enough that the second start happens while the first still runs.
    let bin = write_fake_cli(dir.path(), "sleep 15");
    let mgr = JobManager::new();
    let _cancel = CancelOnDrop(Arc::clone(&mgr));
    let first = start_job_on(
        Some(&mgr),
        launch_in(&dir, &bin, "task one"),
        Arc::new(NullObserver),
    );
    let JobStartOutcome::Started(_) = first else {
        panic!("first start must be Started");
    };
    let second = start_job_on(
        Some(&mgr),
        launch_in(&dir, &bin, "task two"),
        Arc::new(NullObserver),
    );
    assert!(matches!(second, JobStartOutcome::Busy));
    assert!(has_active_job_on(Some(&mgr)));
    assert!(cancel_job_on(Some(&mgr)), "cancel must report a stop");
}

#[cfg(unix)]
#[test]
fn start_with_a_missing_binary_is_not_configured() {
    let dir = TempDir::new("start-badbin");
    let missing = dir.path().join("no-such-cli");
    let mgr = JobManager::new();
    let outcome = start_job_on(
        Some(&mgr),
        launch_in(&dir, &missing, "task"),
        Arc::new(NullObserver),
    );
    assert!(matches!(outcome, JobStartOutcome::NotConfigured));
    assert!(!has_active_job_on(Some(&mgr)));
}

#[cfg(unix)]
#[test]
fn start_after_shutdown_is_the_closing_refusal() {
    let dir = TempDir::new("start-closed");
    let bin = write_fake_cli(dir.path(), "sleep 1");
    let mgr = JobManager::new();
    mgr.close_admission();
    let outcome = start_job_on(
        Some(&mgr),
        launch_in(&dir, &bin, "task"),
        Arc::new(NullObserver),
    );
    match outcome {
        JobStartOutcome::Refused(text) => {
            assert_eq!(text, "Vox is closing, so I didn't start it.");
        }
        _ => panic!("a closed manager must refuse with the closing text"),
    }
}

#[test]
fn cancel_and_has_active_are_false_without_a_manager() {
    assert!(!cancel_job_on(None));
    assert!(!has_active_job_on(None));
}

#[test]
fn cancel_with_no_active_job_reports_nothing_to_cancel() {
    let mgr = JobManager::new();
    assert!(!has_active_job_on(Some(&mgr)));
    assert!(!cancel_job_on(Some(&mgr)));
}
