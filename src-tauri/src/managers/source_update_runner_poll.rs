//! Pure state machine for watching one update run (DESIGN D3, R2-2, R3-1, R2-7).
//!
//! `poll_step` takes the bytes of the status file, the job and lock facts and
//! returns the effects the async shell must apply. It never touches the
//! filesystem, launchd or the UI, so every rule is covered by the table tests
//! in `source_update_runner_poll_tests.rs`.
//!
//! Rules in short: a live job is never cleaned up, whatever the status says;
//! a finished run is published once; cleanup only goes through `reconcile`
//! (no live process, lock not held alive); the lock is never an effect.

use super::source_update_runner::{
    parse_status, reconcile, running_matches, terminal_result, Decision, JobProbe, LockState,
    ReconcileInput, RunPhase, RunStatus, UpdateProgress, INTERRUPTED_AFTER_SECS,
};

/// Consecutive invalid status reads, with the job dead, before the run fails as unreadable.
pub const UNREADABLE_AFTER_READS: u32 = 30;
/// Minimum gap between two tray refreshes, in milliseconds.
pub const TRAY_MIN_INTERVAL_MS: u64 = 2_000;
/// Percent change that refreshes the tray even inside the same phase.
pub const TRAY_PERCENT_STEP: u8 = 5;
/// Message recorded when a run ends without a terminal status.
pub const INTERRUPTED_MESSAGE: &str = "interrupted";
/// Message shown when the status file stays unreadable.
pub const UNREADABLE_MESSAGE: &str = "status unreadable";

/// Something the shell must do. Lock files are never an effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Cache the progress and emit `source-update-progress`.
    PublishProgress(UpdateProgress),
    /// Rebuild the tray menu.
    RefreshTray,
    /// The run succeeded; the progress is the final one (phase done, 100).
    PublishDone(UpdateProgress),
    /// The run failed; the progress carries the message.
    PublishFailed(UpdateProgress),
    /// Atomically write this status file (used for `failed: interrupted`).
    WriteStatus(RunStatus),
    /// Boot out the job registration, remove its plist and rotate old logs.
    Cleanup,
    /// The poller ends after this.
    Stop,
}

/// Memory of one watched run between ticks.
#[derive(Debug, Clone)]
pub struct PollState {
    run_id: String,
    last: Option<RunStatus>,
    progress: Option<UpdateProgress>,
    outcome_published: bool,
    failed_unreadable: bool,
    invalid_reads: u32,
    tray_at_ms: Option<u64>,
    tray_phase: Option<RunPhase>,
    tray_percent: u8,
    tray_pending: bool,
}

impl PollState {
    /// Fresh state for the run `run_id`.
    pub fn new(run_id: &str) -> Self {
        Self {
            run_id: run_id.to_string(),
            last: None,
            progress: None,
            outcome_published: false,
            failed_unreadable: false,
            invalid_reads: 0,
            tray_at_ms: None,
            tray_phase: None,
            tray_percent: 0,
            tray_pending: false,
        }
    }

    /// Consecutive invalid reads so far.
    #[cfg(test)]
    pub fn invalid_reads(&self) -> u32 {
        self.invalid_reads
    }

    /// The last progress the shell was asked to publish.
    #[cfg(test)]
    pub fn published_progress(&self) -> Option<&UpdateProgress> {
        self.progress.as_ref()
    }

    /// True once a valid status for this run has been read.
    pub fn has_status(&self) -> bool {
        self.last.is_some()
    }
}

/// Age used for the reconcile rules. Only a status written for this run counts;
/// before that, the run's plist age (or time since the poller started) is used so
/// a previous run's `status.json` can never look like this run's silence.
pub fn status_age_for(
    has_status: bool,
    status_file_age: Option<u64>,
    plist_age: Option<u64>,
    since_start_secs: u64,
) -> u64 {
    if has_status {
        status_file_age.unwrap_or(since_start_secs)
    } else {
        plist_age.unwrap_or(since_start_secs)
    }
}

/// One tick's observations, gathered by the shell.
#[derive(Debug, Clone)]
pub struct PollInput<'a> {
    /// Bounded bytes of the status file; `None` when missing or unreadable.
    pub status_bytes: Option<&'a [u8]>,
    /// Age of the status file (or of the plist when there is no status), seconds.
    pub status_age_secs: u64,
    /// Launchd state of the run's job (probed at most every 2 s).
    pub job: JobProbe,
    /// Lock directory state (probed at most every 2 s).
    pub lock: LockState,
    /// Full SHA of the running build, or empty when unknown.
    pub running_build_sha: &'a str,
    /// True when a valid `pending.json` in state `built` exists for this run (R7-4).
    pub pending_built: bool,
    /// Current Unix time in seconds.
    pub now_unix: u64,
    /// Monotonic milliseconds since the poller started.
    pub now_ms: u64,
}

/// True when a `done` status must wait: the job still runs and the running build
/// is not the target yet (the installer is restarting Vox).
pub fn done_waits_for_restart(status: &RunStatus, running_build_sha: &str, live: bool) -> bool {
    status.phase == RunPhase::Done && live && !running_matches(status, running_build_sha)
}

/// Advances the state by one tick and returns the effects in the order the
/// shell must apply them.
pub fn poll_step(state: &mut PollState, input: &PollInput<'_>) -> Vec<Effect> {
    let live = input.job.is_live();
    // Unknown counts as maybe alive: no cleanup, no interruption, no release (R9-2).
    let maybe_alive = live || input.job.is_unknown();
    let mut effects = Vec::new();
    ingest(state, input);
    publish_progress_change(state, input, maybe_alive, &mut effects);
    flush_pending_tray(state, input.now_ms, &mut effects);
    publish_outcome(state, input, maybe_alive, &mut effects);
    if !maybe_alive {
        decide_cleanup(state, input, &mut effects);
    }
    effects
}

/// Accepts a valid status for this run or counts an invalid read. The last
/// accepted status is kept on invalid input.
fn ingest(state: &mut PollState, input: &PollInput<'_>) {
    let accepted = input.status_bytes.and_then(|bytes| {
        parse_status(bytes, &state.run_id, state.last.as_ref(), input.now_unix).ok()
    });
    match accepted {
        Some(status) => {
            state.invalid_reads = 0;
            state.last = Some(status);
        }
        None => state.invalid_reads = state.invalid_reads.saturating_add(1),
    }
}

/// Emits `PublishProgress` only when the observable progress changed.
fn publish_progress_change(
    state: &mut PollState,
    input: &PollInput<'_>,
    live: bool,
    effects: &mut Vec<Effect>,
) {
    let Some(status) = state.last.clone() else {
        return;
    };
    if status.phase.is_terminal() && done_waits_for_restart(&status, input.running_build_sha, live)
    {
        return;
    }
    let progress = UpdateProgress::from(&status);
    if state.progress.as_ref() == Some(&progress) {
        return;
    }
    state.progress = Some(progress.clone());
    effects.push(Effect::PublishProgress(progress));
    request_tray(state, status.phase, status.percent, input.now_ms, effects);
}

/// Refreshes the tray on a phase change or a 5-point change, at most once per
/// `TRAY_MIN_INTERVAL_MS`; otherwise remembers that a refresh is owed.
fn request_tray(
    state: &mut PollState,
    phase: RunPhase,
    percent: u8,
    now_ms: u64,
    effects: &mut Vec<Effect>,
) {
    let changed = state.tray_phase != Some(phase)
        || percent.abs_diff(state.tray_percent) >= TRAY_PERCENT_STEP;
    if !changed {
        return;
    }
    match state.tray_at_ms {
        Some(at) if now_ms.saturating_sub(at) < TRAY_MIN_INTERVAL_MS => state.tray_pending = true,
        _ => refresh_tray(state, phase, percent, now_ms, effects),
    }
}

/// Pays off a refresh that was deferred by the coalescing rule.
fn flush_pending_tray(state: &mut PollState, now_ms: u64, effects: &mut Vec<Effect>) {
    if !state.tray_pending {
        return;
    }
    let Some((phase, percent)) = state.last.as_ref().map(|s| (s.phase, s.percent)) else {
        state.tray_pending = false;
        return;
    };
    let due = state
        .tray_at_ms
        .is_none_or(|at| now_ms.saturating_sub(at) >= TRAY_MIN_INTERVAL_MS);
    if due {
        refresh_tray(state, phase, percent, now_ms, effects);
    }
}

fn refresh_tray(
    state: &mut PollState,
    phase: RunPhase,
    percent: u8,
    now_ms: u64,
    effects: &mut Vec<Effect>,
) {
    effects.push(Effect::RefreshTray);
    state.tray_at_ms = Some(now_ms);
    state.tray_phase = Some(phase);
    state.tray_percent = percent;
    state.tray_pending = false;
}

/// Publishes the outcome once: a terminal status, or an unreadable status file
/// after `UNREADABLE_AFTER_READS` invalid reads while the job is dead.
fn publish_outcome(
    state: &mut PollState,
    input: &PollInput<'_>,
    live: bool,
    effects: &mut Vec<Effect>,
) {
    if state.outcome_published {
        return;
    }
    if let Some(status) = state.last.clone().filter(|s| s.phase.is_terminal()) {
        if done_waits_for_restart(&status, input.running_build_sha, live) {
            return;
        }
        let effect = match terminal_result(&status, input.running_build_sha) {
            Ok(()) => Effect::PublishDone(UpdateProgress::from(&status)),
            Err(message) => Effect::PublishFailed(failed_progress(state, &message)),
        };
        effects.push(effect);
        state.outcome_published = true;
    } else if !live && state.invalid_reads >= UNREADABLE_AFTER_READS {
        effects.push(Effect::PublishFailed(failed_progress(
            state,
            UNREADABLE_MESSAGE,
        )));
        state.outcome_published = true;
        state.failed_unreadable = true;
    }
}

/// Cleanup and interruption, decided by `reconcile` (R3-1). Only called when the
/// job has no live process.
fn decide_cleanup(state: &mut PollState, input: &PollInput<'_>, effects: &mut Vec<Effect>) {
    if state.failed_unreadable {
        if input.lock != LockState::HeldAlive {
            effects.push(Effect::Cleanup);
            effects.push(Effect::Stop);
        }
        return;
    }
    let decision = reconcile(&ReconcileInput {
        status: state.last.clone(),
        status_age_secs: input.status_age_secs,
        job: input.job,
        lock: input.lock,
        running_build_sha: input.running_build_sha,
        published: state.outcome_published,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
        pending_built: input.pending_built,
    });
    match decision {
        Decision::MarkInterrupted(message) => {
            if let Some(status) = state.last.clone() {
                let interrupted = RunStatus {
                    phase: RunPhase::Failed,
                    message: INTERRUPTED_MESSAGE.to_string(),
                    ts: input.now_unix,
                    ..status
                };
                effects.push(Effect::WriteStatus(interrupted.clone()));
                state.last = Some(interrupted);
            }
            if !state.outcome_published {
                effects.push(Effect::PublishFailed(failed_progress(state, &message)));
                state.outcome_published = true;
            }
        }
        Decision::CleanupFinished => {
            effects.push(Effect::Cleanup);
            effects.push(Effect::Stop);
        }
        Decision::PublishFailed(message) => {
            if !state.outcome_published {
                effects.push(Effect::PublishFailed(failed_progress(state, &message)));
                state.outcome_published = true;
            }
        }
        Decision::Rest => {
            // Resting: the watcher stops for good, with no cleanup and no outcome.
            effects.push(Effect::Stop);
        }
        Decision::None | Decision::Adopt | Decision::PublishDone => {}
    }
}

/// Failed progress for this run, based on the last known progress.
fn failed_progress(state: &PollState, message: &str) -> UpdateProgress {
    let base = state
        .last
        .as_ref()
        .map(UpdateProgress::from)
        .or_else(|| state.progress.clone())
        .unwrap_or_else(|| UpdateProgress {
            run_id: state.run_id.clone(),
            phase: RunPhase::Failed,
            percent: 0,
            target: String::new(),
            message: String::new(),
            estimated: false,
        });
    UpdateProgress {
        phase: RunPhase::Failed,
        message: message.to_string(),
        ..base
    }
}

#[cfg(test)]
#[path = "source_update_runner_poll_tests.rs"]
mod tests;
