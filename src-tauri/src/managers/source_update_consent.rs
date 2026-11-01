//! Consent-dependent state of the automatic update (R6-7, R7-6, R7-7, R9-1).
//!
//! Covers the per-run `run-<id>.auto` marker, the silent and auto-pending flags that the
//! footer and the tray read, the cleanup of a build that finished without consent, the
//! conversion of an automatic run into a manual one, and the manual-commit request that the
//! commit sequence will consume. Decisions are pure functions; the thin wrappers do the file
//! and app-state work.
//!
//! Attached to `source_update_auto` with a `#[path]` attribute (the module list in
//! `managers/mod.rs` is not touched by this step).

#![cfg_attr(not(test), allow(dead_code))]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use log::{info, warn};
use tauri::AppHandle;

use crate::managers::source_update_auto::{
    load_auto_state, save_auto_state, AutoResult, AutoState,
};
use crate::managers::source_update_pending::{
    PendingOrigin, PendingRun, PendingState, PendingStore,
};
use crate::managers::source_update_runner::{
    is_valid_run_id, now_unix, update_state_dir, write_private_atomic, UpdateProgress,
};

/// Prefix of the marker file of an automatic run: `run-<run_id>.auto`.
pub const AUTO_MARKER_PREFIX: &str = "run-";
/// Suffix of the marker file of an automatic run.
pub const AUTO_MARKER_SUFFIX: &str = ".auto";
/// Content written into a marker. Its presence is the signal; the text is fixed.
const AUTO_MARKER_BODY: &[u8] = b"auto\n";

/// Path of the marker for `run_id` inside `state_dir`. `None` for a malformed run id.
pub fn auto_marker_path(state_dir: &Path, run_id: &str) -> Option<PathBuf> {
    is_valid_run_id(run_id)
        .then(|| state_dir.join(format!("{AUTO_MARKER_PREFIX}{run_id}{AUTO_MARKER_SUFFIX}")))
}

/// The `run-<id>.auto` markers of automatic runs, with the same file discipline as the pending
/// record: regular files only, symlinks refused, mode 0600 (written atomically).
#[derive(Debug, Clone)]
pub struct AutoMarker {
    state_dir: PathBuf,
}

impl AutoMarker {
    /// Markers rooted at `state_dir`. Creates nothing.
    pub fn new(state_dir: &Path) -> Self {
        Self {
            state_dir: state_dir.to_path_buf(),
        }
    }

    /// Writes the marker for `run_id` before the run is launched. Refuses an existing symlink.
    pub fn write(&self, run_id: &str) -> io::Result<()> {
        let path = self.path_for(run_id)?;
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if meta.file_type().is_symlink() || !meta.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "auto marker is not a regular file",
                ));
            }
        }
        write_private_atomic(&path, AUTO_MARKER_BODY)
    }

    /// True when a regular marker file exists for `run_id`. Symlinks and any error count as absent.
    pub fn exists(&self, run_id: &str) -> bool {
        let Ok(path) = self.path_for(run_id) else {
            return false;
        };
        fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file() && !meta.file_type().is_symlink())
    }

    /// Removes the marker for `run_id`. Idempotent. Refuses to remove anything that is not a
    /// regular file (a symlink is never followed and never removed here).
    pub fn remove(&self, run_id: &str) -> io::Result<()> {
        let path = self.path_for(run_id)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "auto marker is not a regular file",
            )),
            Ok(_) => fs::remove_file(&path),
        }
    }

    fn path_for(&self, run_id: &str) -> io::Result<PathBuf> {
        auto_marker_path(&self.state_dir, run_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid run id"))
    }
}

/// Silent flag of progress for one run (R7-7 a): true exactly when the run's marker exists.
pub fn silent_for_run(state_dir: &Path, run_id: &str) -> bool {
    AutoMarker::new(state_dir).exists(run_id)
}

/// Facts behind the availability flag `auto_pending` (R7-7 b).
#[derive(Debug, Clone, Copy)]
pub struct AutoPendingFacts<'a> {
    /// [`crate::settings::auto_update_effective`] for the current settings.
    pub auto_effective: bool,
    /// The active run is an automatic run (its marker exists).
    pub active_run_marked: bool,
    /// The pending record, if any (its origin and state decide).
    pub pending: Option<&'a PendingRun>,
    /// Full SHA of the target the notice is about.
    pub target: &'a str,
    /// The persisted attempt state, if any.
    pub last: Option<&'a AutoState>,
}

/// Whether the "update available" notice is hidden because an automatic attempt on `target`
/// is healthy: effective, and running, built or committing (R7-7 b). A failed or skipped last
/// attempt on the same target brings the notice back.
pub fn auto_pending(facts: &AutoPendingFacts<'_>) -> bool {
    if !facts.auto_effective {
        return false;
    }
    let healthy = facts.active_run_marked || auto_record_for(facts.pending, facts.target);
    healthy && !last_attempt_failed_for(facts.last, facts.target)
}

/// True when `pending` is an automatic record for `target` in state built or committing.
fn auto_record_for(pending: Option<&PendingRun>, target: &str) -> bool {
    pending.is_some_and(|run| run.origin == PendingOrigin::Auto && run.target_full == target)
}

/// True when the last recorded attempt on `target` failed or was skipped.
fn last_attempt_failed_for(last: Option<&AutoState>, target: &str) -> bool {
    last.is_some_and(|state| {
        state.last_target.as_deref() == Some(target)
            && matches!(
                state.last_result,
                Some(AutoResult::Failed | AutoResult::Skipped)
            )
    })
}

/// What to do with a pending record when the consent that started it is gone (R9-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupPlan {
    /// Leave the record and its staged files alone.
    Keep,
    /// Remove the built record and its staged directory, and record `skipped:disabled`.
    RemoveBuilt,
}

/// Pure decision (R9-1): a built record without consent is removed. A committing record is
/// never touched, and a record with consent is kept.
pub fn cleanup_plan(pending: Option<&PendingRun>, consent: bool) -> CleanupPlan {
    match pending {
        Some(run) if !consent && run.state == PendingState::Built => CleanupPlan::RemoveBuilt,
        _ => CleanupPlan::Keep,
    }
}

/// Applies [`cleanup_plan`] under `state_dir`. For `RemoveBuilt` it removes `pending.json` and
/// its staged directory, then records `skipped:disabled` for the target while keeping the
/// earlier failure memory. An unreadable record is kept (the commit path fails closed on it).
pub fn cleanup_after_build_without_consent(
    state_dir: &Path,
    consent: bool,
    now: u64,
) -> Result<CleanupPlan, String> {
    let store = PendingStore::new(state_dir);
    let Ok(pending) = store.load() else {
        return Ok(CleanupPlan::Keep);
    };
    let plan = cleanup_plan(pending.as_ref(), consent);
    if let (CleanupPlan::RemoveBuilt, Some(run)) = (plan, pending.as_ref()) {
        store
            .remove_all()
            .map_err(|_| "could not remove the built update".to_string())?;
        let previous = match load_auto_state(state_dir) {
            Ok(state) => state.unwrap_or_default(),
            Err(_) => AutoState::default(),
        };
        save_auto_state(
            state_dir,
            &previous.with_skipped(&run.target_full, "disabled", now),
        )
        .map_err(|_| "could not record the skipped update".to_string())?;
    }
    Ok(plan)
}

/// Called after consent is withdrawn (checks or the automatic switch turned off). Discards a
/// built-but-not-committed update, then tells the footer and tray the state changed. A running
/// build is not cancelled: it finishes and the cleanup happens when its record appears (R9-1).
pub fn on_consent_withdrawn(app: &AppHandle) {
    match crate::portable::app_data_dir(app) {
        Ok(app_data) => {
            let state_dir = update_state_dir(&app_data);
            match cleanup_after_build_without_consent(&state_dir, false, now_unix()) {
                Ok(CleanupPlan::RemoveBuilt) => {
                    info!("removed a built automatic update after consent was withdrawn");
                }
                Ok(CleanupPlan::Keep) => {}
                Err(error) => warn!("could not clean up after withdrawn consent: {error}"),
            }
        }
        Err(error) => warn!("could not locate the update state after withdrawn consent: {error}"),
    }
    crate::managers::source_update_auto::status::emit_auto_update_changed(app);
}

/// What a manual "Update now" changes about automatic runs (R7-7 c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManualConversion {
    /// The active run was automatic; its marker is removed, so its progress is no longer silent.
    pub remove_active_marker: bool,
    /// A built record exists; a manual commit is requested (the commit sequence consumes it).
    pub request_commit: bool,
}

/// Pure decision for a manual start.
pub fn manual_conversion_plan(active_run_marked: bool, pending_built: bool) -> ManualConversion {
    ManualConversion {
        remove_active_marker: active_run_marked,
        request_commit: pending_built,
    }
}

/// Applies a manual start to the automatic state under `state_dir`: removes the active run's
/// marker and the marker of a built record, and requests a manual commit for a built record.
pub fn apply_manual_conversion(
    state_dir: &Path,
    active_run_id: Option<&str>,
) -> Result<ManualConversion, String> {
    let markers = AutoMarker::new(state_dir);
    let pending = PendingStore::new(state_dir).load().ok().flatten();
    let pending_built = pending
        .as_ref()
        .is_some_and(|run| run.state == PendingState::Built);
    let active_marked = active_run_id.is_some_and(|id| markers.exists(id));
    let plan = manual_conversion_plan(active_marked, pending_built);
    if plan.remove_active_marker {
        if let Some(id) = active_run_id {
            markers
                .remove(id)
                .map_err(|_| "could not remove the automatic marker".to_string())?;
        }
    }
    if let Some(run) = pending.as_ref().filter(|_| pending_built) {
        // A marker of the built run is only ever written by the auto start; drop it too.
        let _ = markers.remove(&run.run_id);
        request_manual_commit();
    }
    Ok(plan)
}

/// Runs [`apply_manual_conversion`] for the running app. Returns the plan, or `None` when the
/// app data directory is unknown. Nothing else is changed.
pub fn convert_to_manual_now(
    app: &AppHandle,
    active_run_id: Option<&str>,
) -> Option<ManualConversion> {
    let app_data = crate::portable::app_data_dir(app).ok()?;
    match apply_manual_conversion(&update_state_dir(&app_data), active_run_id) {
        Ok(plan) => Some(plan),
        Err(error) => {
            warn!("could not convert the automatic update for a manual start: {error}");
            None
        }
    }
}

/// A one-shot request, recorded by a manual start, for the commit sequence to consume.
#[derive(Debug, Default)]
pub struct CommitRequest {
    requested: AtomicBool,
}

impl CommitRequest {
    /// No request pending.
    pub const fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
        }
    }

    /// Records a request.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    /// Returns whether a request was pending and clears it.
    pub fn take(&self) -> bool {
        self.requested.swap(false, Ordering::SeqCst)
    }
}

/// The process-wide manual-commit request.
static MANUAL_COMMIT: CommitRequest = CommitRequest::new();

/// Records that the owner asked for a manual commit of the built update.
pub fn request_manual_commit() {
    MANUAL_COMMIT.request();
}

/// Consumes the manual-commit request. The commit sequence (next step) calls this.
pub fn take_manual_commit_request() -> bool {
    MANUAL_COMMIT.take()
}

/// Sets the silent flag of a progress value from the marker on disk.
pub fn annotate_progress(app: &AppHandle, mut progress: UpdateProgress) -> UpdateProgress {
    progress.silent = match crate::portable::app_data_dir(app) {
        Ok(app_data) => silent_for_run(&update_state_dir(&app_data), &progress.run_id),
        Err(_) => false,
    };
    progress
}

/// Sets `auto_pending` of a published check result from settings, the marker, the pending
/// record and the attempt state.
pub fn annotate_info(
    app: &AppHandle,
    mut info: crate::managers::source_update::SourceUpdateInfo,
) -> crate::managers::source_update::SourceUpdateInfo {
    let settings = crate::settings::get_settings(app);
    let auto_effective = crate::settings::auto_update_effective(&settings);
    info.auto_pending = auto_effective && pending_now(app, &info.latest_full, auto_effective);
    info
}

/// Reads the disk and the active run for [`auto_pending`]. Read failures count as "not pending".
fn pending_now(app: &AppHandle, target: &str, auto_effective: bool) -> bool {
    let Ok(app_data) = crate::portable::app_data_dir(app) else {
        return false;
    };
    let state_dir = update_state_dir(&app_data);
    let pending = PendingStore::new(&state_dir).load().ok().flatten();
    let last = load_auto_state(&state_dir).ok().flatten();
    let active_marked = crate::managers::source_update::active_run_id(app)
        .is_some_and(|id| AutoMarker::new(&state_dir).exists(&id));
    auto_pending(&AutoPendingFacts {
        auto_effective,
        active_run_marked: active_marked,
        pending: pending.as_ref(),
        target,
        last: last.as_ref(),
    })
}

#[cfg(test)]
#[path = "source_update_consent_tests.rs"]
mod tests;
