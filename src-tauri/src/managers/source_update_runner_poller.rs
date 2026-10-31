//! Async shell around the pure poll machine (DESIGN D3). It owns the 500 ms tick,
//! the bounded status read and the launchd and lock probes (at most every 2 s),
//! and it applies the effects `poll_step` returns. It decides nothing itself.

use super::source_update::{cache_progress, recheck_after_done, run_finished};
use super::source_update_runner::{
    file_age_secs, lock_state, now_unix, process_alive, read_status_bounded, write_status_file,
    JobControl, JobState, LaunchctlJobControl, LockState,
};
use super::source_update_runner_poll::{poll_step, Effect, PollInput, PollState};
use super::source_update_runner_run::cleanup_run;
use log::warn;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// Interval between two status reads.
const TICK: Duration = Duration::from_millis(500);
/// Interval between two launchd and lock probes.
const PROBE_EVERY: Duration = Duration::from_secs(2);

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

/// Starts the poller for `config` on Tauri's async runtime.
pub fn spawn(app: AppHandle, config: PollerConfig) {
    tauri::async_runtime::spawn(async move {
        run(app, config).await;
    });
}

async fn run(app: AppHandle, config: PollerConfig) {
    let control = LaunchctlJobControl::current();
    let started = Instant::now();
    let mut state = PollState::new(&config.run_id);
    let mut probe: Option<(Instant, JobState, LockState)> = None;
    loop {
        let due = probe
            .as_ref()
            .is_none_or(|(at, _, _)| at.elapsed() >= PROBE_EVERY);
        if due {
            probe = probe_now(&control, &config).await.or(probe);
        }
        let (job, lock) = probe.as_ref().map_or(
            (JobState::Absent, LockState::HeldAlive),
            |(_, job, lock)| (*job, *lock),
        );
        let bytes = read_status_bounded(&config.status_file).ok().flatten();
        let age = file_age_secs(&config.status_file)
            .or_else(|| file_age_secs(&config.plist))
            .unwrap_or(0);
        let input = PollInput {
            status_bytes: bytes.as_deref(),
            status_age_secs: age,
            job,
            lock,
            running_build_sha: &config.running_build_sha,
            now_unix: now_unix(),
            now_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        let effects = poll_step(&mut state, &input);
        if apply(&app, &config, &control, effects) {
            break;
        }
        tokio::time::sleep(TICK).await;
    }
    run_finished(&app, &config.run_id);
}

/// Reads launchd and the lock off the async runtime. `None` if the probe task failed.
async fn probe_now(
    control: &LaunchctlJobControl,
    config: &PollerConfig,
) -> Option<(Instant, JobState, LockState)> {
    let control = *control;
    let label = config.label.clone();
    let lock_dir = config.lock_dir.clone();
    tokio::task::spawn_blocking(move || {
        (
            Instant::now(),
            control.state(&label),
            lock_state(&lock_dir, &process_alive),
        )
    })
    .await
    .ok()
}

/// Applies the effects in order. Returns `true` when the poller must stop.
fn apply(
    app: &AppHandle,
    config: &PollerConfig,
    control: &LaunchctlJobControl,
    effects: Vec<Effect>,
) -> bool {
    for effect in effects {
        match effect {
            Effect::PublishProgress(progress) => cache_progress(app, progress),
            Effect::RefreshTray => crate::tray::update_tray_menu(app),
            Effect::PublishDone(progress) => {
                cache_progress(app, progress);
                crate::tray::update_tray_menu(app);
                recheck_after_done(app);
            }
            Effect::PublishFailed(progress) => {
                cache_progress(app, progress);
                crate::tray::update_tray_menu(app);
            }
            Effect::WriteStatus(status) => {
                if let Err(error) = write_status_file(&config.status_file, &status) {
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
