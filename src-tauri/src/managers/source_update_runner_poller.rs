//! Async shell around the pure poll machine (DESIGN D3). It owns the 500 ms tick,
//! the bounded status read and the launchd and lock probes (at most every 2 s),
//! and it applies the effects `poll_step` returns. It decides nothing itself.
//!
//! Stop contract (R9-4): [`PollerHandle::stop_and_join`] sets a cooperative stop
//! flag and waits for the task's own completion signal (at most
//! [`STOP_JOIN_TIMEOUT`]). Once the flag is set the poller writes no status
//! file and applies no further effects; it ends at its next check.

use super::source_update::{
    cache_progress, recheck_after_done, release_launch_guard, run_finished,
};
use super::source_update_pending::pending_built_for;
use super::source_update_runner::{
    file_age_secs, lock_state, now_unix, process_alive, read_status_bounded, write_status_file,
    JobControl, JobProbe, LaunchctlJobControl, LockState, RunStatus,
};
use super::source_update_runner_poll::{poll_step, status_age_for, Effect, PollInput, PollState};
use super::source_update_runner_run::cleanup_run;
use log::warn;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Interval between two status reads.
const TICK: Duration = Duration::from_millis(500);
/// Interval between two launchd and lock probes.
const PROBE_EVERY: Duration = Duration::from_secs(2);
/// How long a stop waits for the poller task to finish.
pub const STOP_JOIN_TIMEOUT: Duration = Duration::from_secs(10);

/// A poller did not confirm that it stopped within [`STOP_JOIN_TIMEOUT`].
/// The stop flag is set, so it still cannot write status once it wakes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollerStopError;

impl fmt::Display for PollerStopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the update poller did not stop in time")
    }
}

impl std::error::Error for PollerStopError {}

/// Completion signal set when the poller task ends, including on panic.
#[derive(Debug, Default)]
struct Finished {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Finished {
    fn mark_done(&self) {
        *lock(&self.done) = true;
        self.changed.notify_all();
    }

    fn wait_done(&self, timeout: Duration) -> bool {
        let guard = lock(&self.done);
        match self
            .changed
            .wait_timeout_while(guard, timeout, |done| !*done)
        {
            Ok((guard, _)) => *guard,
            Err(poisoned) => *poisoned.into_inner().0,
        }
    }
}

/// Marks the task finished when dropped, so every exit path acknowledges.
#[derive(Debug)]
pub(crate) struct FinishOnDrop(Arc<Finished>);

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        self.0.mark_done();
    }
}

/// The owner's side of a poller: requests the stop and waits for its end.
#[derive(Debug)]
pub struct PollerHandle {
    stop: Arc<AtomicBool>,
    finished: Arc<Finished>,
}

/// The task's side: the flag it checks and the completion signal it sets.
pub(crate) struct PollerLife {
    pub(crate) stop: Arc<AtomicBool>,
    _finish: FinishOnDrop,
}

/// A connected handle and task side.
pub(crate) fn lifecycle() -> (PollerHandle, PollerLife) {
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(Finished::default());
    let handle = PollerHandle {
        stop: Arc::clone(&stop),
        finished: Arc::clone(&finished),
    };
    let life = PollerLife {
        stop,
        _finish: FinishOnDrop(finished),
    };
    (handle, life)
}

impl PollerHandle {
    /// Sets the stop flag and waits up to [`STOP_JOIN_TIMEOUT`] for the task to end.
    /// On timeout returns `Err` and leaves the task to end by itself.
    /// Blocks the calling thread: never call it on an async runtime worker.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn stop_and_join(self) -> Result<(), PollerStopError> {
        self.wait_stopped(STOP_JOIN_TIMEOUT)
    }

    /// Sets the stop flag without waiting.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Sets the stop flag and waits up to `timeout`. The handle stays usable on `Err`.
    pub(crate) fn wait_stopped(&self, timeout: Duration) -> Result<(), PollerStopError> {
        self.request_stop();
        if self.finished.wait_done(timeout) {
            Ok(())
        } else {
            Err(PollerStopError)
        }
    }
}

/// The poller registered for the run that is currently watched (one at a time).
#[derive(Debug, Default)]
pub struct PollerRegistry {
    slot: Mutex<Option<(String, PollerHandle)>>,
}

impl PollerRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `handle` for `run_id`. A previously registered poller is asked to
    /// stop (not joined) and is forgotten.
    pub fn replace(&self, run_id: &str, handle: PollerHandle) {
        let previous = lock(&self.slot).replace((run_id.to_string(), handle));
        if let Some((_, previous)) = previous {
            previous.request_stop();
        }
    }

    /// Stops the registered poller and waits for it. Nothing registered is `Ok`.
    /// On `Err` the poller stays registered, so a later attempt waits for it again.
    pub fn stop_and_join(&self) -> Result<(), PollerStopError> {
        self.stop_and_join_within(STOP_JOIN_TIMEOUT)
    }

    fn stop_and_join_within(&self, timeout: Duration) -> Result<(), PollerStopError> {
        let mut slot = lock(&self.slot);
        let Some((_, handle)) = slot.as_ref() else {
            return Ok(());
        };
        handle.wait_stopped(timeout)?;
        *slot = None;
        Ok(())
    }

    /// Run id of the registered poller, if any.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn registered_run(&self) -> Option<String> {
        lock(&self.slot).as_ref().map(|(run_id, _)| run_id.clone())
    }

    /// True while the poller registered for `run_id` has not finished (B3b-2b).
    pub fn is_running(&self, run_id: &str) -> bool {
        lock(&self.slot)
            .as_ref()
            .is_some_and(|(id, handle)| id == run_id && !handle.finished.wait_done(Duration::ZERO))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Everything the poller needs to know about the run it watches.
#[derive(Debug, Clone)]
pub struct PollerConfig {
    /// Run identifier.
    pub run_id: String,
    /// Launchd label of the run's job.
    pub label: String,
    /// Update state directory.
    pub state_dir: PathBuf,
    /// Status file the script writes.
    pub status_file: PathBuf,
    /// Plist of the run's job.
    pub plist: PathBuf,
    /// Update lock directory (read only).
    pub lock_dir: PathBuf,
    /// Full SHA of the running build, or empty when unknown.
    pub running_build_sha: String,
}

/// Starts the poller for `config` on Tauri's async runtime. Returns the handle
/// that stops it. Callers may drop the handle to fire and forget.
pub fn spawn(app: AppHandle, config: PollerConfig) -> PollerHandle {
    let (handle, life) = lifecycle();
    tauri::async_runtime::spawn(async move {
        run(app, config, life).await;
    });
    handle
}

/// True while the poller may still act.
pub(crate) fn should_continue(stop: &AtomicBool) -> bool {
    !stop.load(Ordering::SeqCst)
}

/// Writes `status` unless a stop was requested. Returns whether it was written.
pub(crate) fn write_status_unless_stopped(
    stop: &AtomicBool,
    path: &Path,
    status: &RunStatus,
    write: impl FnOnce(&Path, &RunStatus) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    if !should_continue(stop) {
        return Ok(false);
    }
    write(path, status)?;
    Ok(true)
}

async fn run(app: AppHandle, config: PollerConfig, life: PollerLife) {
    let control = LaunchctlJobControl::current();
    let started = Instant::now();
    let mut state = PollState::new(&config.run_id);
    let mut probe: Option<Probed> = None;
    loop {
        if !should_continue(&life.stop) {
            break;
        }
        let due = probe.as_ref().is_none_or(|p| p.at.elapsed() >= PROBE_EVERY);
        if due {
            probe = probe_now(&control, &config).await.or(probe);
        }
        if !should_continue(&life.stop) {
            break;
        }
        let (job, lock, pending_built) = probe
            .as_ref()
            .map_or((JobProbe::Unknown, LockState::HeldAlive, false), |p| {
                (p.job, p.lock, p.pending_built)
            });
        let bytes = read_status_bounded(&config.status_file).ok().flatten();
        let age = status_age_for(
            state.has_status(),
            file_age_secs(&config.status_file),
            file_age_secs(&config.plist),
            started.elapsed().as_secs(),
        );
        let input = PollInput {
            status_bytes: bytes.as_deref(),
            status_age_secs: age,
            job,
            lock,
            running_build_sha: &config.running_build_sha,
            pending_built,
            now_unix: now_unix(),
            now_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        let effects = poll_step(&mut state, &input);
        if apply(&app, &config, &control, effects, &life.stop) {
            break;
        }
        tokio::time::sleep(TICK).await;
    }
    run_finished(&app, &config.run_id);
    // The watcher ended (resting, terminal or stopped): the driver may act now (B3b-2b).
    crate::managers::source_update_driver::kick();
}

/// One launchd, lock and pending probe, with the instant it was taken.
#[derive(Debug, Clone, Copy)]
struct Probed {
    at: Instant,
    job: JobProbe,
    lock: LockState,
    pending_built: bool,
}

/// Reads launchd, the lock and the pending file off the async runtime. `None` if the probe task failed.
async fn probe_now(control: &LaunchctlJobControl, config: &PollerConfig) -> Option<Probed> {
    let control = *control;
    let label = config.label.clone();
    let lock_dir = config.lock_dir.clone();
    let state_dir = config.state_dir.clone();
    let run_id = config.run_id.clone();
    tokio::task::spawn_blocking(move || Probed {
        at: Instant::now(),
        job: control.probe(&label),
        lock: lock_state(&lock_dir, &process_alive),
        pending_built: pending_built_for(&state_dir, &run_id),
    })
    .await
    .ok()
}

/// Applies the effects in order, stopping before any effect once a stop was
/// requested. Returns `true` when the poller must stop.
fn apply(
    app: &AppHandle,
    config: &PollerConfig,
    control: &LaunchctlJobControl,
    effects: Vec<Effect>,
    stop: &AtomicBool,
) -> bool {
    for effect in effects {
        if !should_continue(stop) {
            return true;
        }
        match effect {
            Effect::PublishProgress(progress) => cache_progress(app, progress),
            Effect::RefreshTray => crate::tray::update_tray_menu(app),
            Effect::PublishDone(progress) => {
                crate::managers::source_update_driver::on_terminal(app, &config.run_id, &progress);
                cache_progress(app, progress);
                release_launch_guard(app);
                crate::tray::update_tray_menu(app);
                recheck_after_done(app);
            }
            Effect::PublishFailed(progress) => {
                crate::managers::source_update_driver::on_terminal(app, &config.run_id, &progress);
                cache_progress(app, progress);
                release_launch_guard(app);
                crate::tray::update_tray_menu(app);
            }
            Effect::WriteStatus(status) => {
                if let Err(error) = write_status_unless_stopped(
                    stop,
                    &config.status_file,
                    &status,
                    write_status_file,
                ) {
                    warn!("could not record an interrupted update: {error}");
                }
            }
            Effect::Cleanup => {
                if let Err(error) = cleanup_run(control, &config.state_dir, &config.run_id) {
                    warn!("update cleanup did not finish: {error}");
                }
            }
            Effect::Stop => return true,
        }
    }
    false
}

#[cfg(test)]
#[path = "source_update_runner_poller_tests.rs"]
mod tests;
