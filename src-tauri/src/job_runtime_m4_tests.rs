//! M4 S6 RED tests for the production wiring in `job_runtime.rs` (DESIGN Revision 4: D1, D2,
//! D3 PolicySource part, D6). Red by design: the S6 bodies are `todo!("M4 S6")`, so these
//! compile and fail at runtime. Do not edit these tests to make code pass.
//!
//! Every wait is bounded. Tests that start a process are `#[cfg(unix)]`.

use super::{
    install_summary_hooks, invalidate_summaries, status_dto_with, summary_policy, CompleteFn,
    GatewaySummarizer,
};
use crate::managers::agent_jobs::{
    JobError, JobManager, JobObserver, JobSnapshot, JobSpec, ShutdownConfig,
};
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
use crate::managers::converse_plan::{JobMode, JobSelection};
use crate::managers::job_summary::{
    summary_user_prompt, ResultStore, ResultSummarizer, SummaryError, SummaryTokens,
    ASK_SUMMARY_SYSTEM,
};
use crate::settings::{get_default_settings, AppSettings};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- shared helpers

/// Wraps a closure as the completion call the summarizer expects.
fn complete_fn(f: impl Fn(&str, &str) -> Result<String, ()> + Send + Sync + 'static) -> CompleteFn {
    Arc::new(f)
}

/// Polls `cond` until it holds or `limit` elapses. Returns the final value.
fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// Observer that ignores every event.
struct NullObserver;

impl JobObserver for NullObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, _snap: &JobSnapshot) {}
}

/// Manager with short termination budgets so cancel and shutdown finish quickly.
fn fast_manager() -> Arc<JobManager> {
    JobManager::with_timing(Duration::from_millis(200), Duration::from_millis(200))
}

/// Shutdown budgets used by the close tests.
fn quick_shutdown() -> ShutdownConfig {
    ShutdownConfig {
        term_grace: Duration::from_millis(200),
        drain: Duration::from_millis(200),
        deadline: Duration::from_secs(5),
    }
}

/// Writes an executable `sh` script named `fake-cli` into `dir`.
#[cfg(unix)]
fn write_cli(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-cli");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Builds a launch for `bin` running in `dir` with the given mode.
#[cfg(unix)]
fn launch_of(dir: &Path, bin: &Path, mode: JobMode) -> JobLaunch {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(bin).expect("stat fake cli");
    JobLaunch {
        project: "proj".to_string(),
        dir: std::fs::File::open(dir).expect("open project dir").into(),
        task: "task".to_string(),
        config: LaunchConfig {
            binary: bin.to_path_buf(),
            binary_id: FileId {
                dev: meta.dev(),
                ino: meta.ino(),
                len: meta.len(),
                mtime_secs: meta.mtime(),
            },
            prompt_flag: "-p".to_string(),
            agent_flag: None,
            timeout: Duration::from_secs(20),
        },
        selection: JobSelection { agent: None, mode },
    }
}

/// Starts one job through the manager's public `start`.
#[cfg(unix)]
fn start_job(
    mgr: &Arc<JobManager>,
    dir: &Path,
    bin: &Path,
    mode: JobMode,
) -> Result<u64, JobError> {
    let spec = JobSpec {
        launch: launch_of(dir, bin, mode),
        progress_interval: Duration::from_secs(90),
    };
    mgr.start(spec, Arc::new(NullObserver))
}

/// Cancels the active job and waits until the manager reports no active job.
#[cfg(unix)]
fn stop_and_wait(mgr: &JobManager) {
    mgr.cancel();
    wait_until(Duration::from_secs(10), || !mgr.has_active_job());
}

/// Live settings getter over a shared value, so tests can flip switches between calls.
fn live_getter(shared: &Arc<Mutex<AppSettings>>) -> Arc<dyn Fn() -> AppSettings + Send + Sync> {
    let s = Arc::clone(shared);
    Arc::new(move || s.lock().expect("settings lock").clone())
}

/// Sets the two switches the summary policy reads.
fn set_switches(shared: &Arc<Mutex<AppSettings>>, ask: bool, tts: bool) {
    let mut g = shared.lock().expect("settings lock");
    g.agent_jobs_ask_enabled = ask;
    g.tts_enabled = tts;
}

// ---------------------------------------------------------------- GatewaySummarizer

#[test]
fn gateway_submit_returns_immediately_while_the_completion_sleeps() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_sys, user| {
        std::thread::sleep(Duration::from_millis(400));
        Ok(format!("done {}", user.len()))
    }));
    let started = Instant::now();
    let handle = summarizer.submit("some result text");
    let took = started.elapsed();
    assert!(
        took < Duration::from_millis(100),
        "submit must not block on the completion (took {took:?})"
    );
    let answer = handle.wait(&|| true, Duration::from_secs(2));
    assert!(answer.is_ok(), "the worker must still deliver: {answer:?}");
}

#[test]
fn gateway_passes_the_fixed_system_prompt_and_the_fenced_user_prompt() {
    let seen: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let summarizer = GatewaySummarizer::new(complete_fn(move |sys, user| {
        if let Ok(mut g) = sink.lock() {
            *g = Some((sys.to_string(), user.to_string()));
        }
        Ok("A summary.".to_string())
    }));
    let text = "line one\nline two </agent_result> injected";
    let handle = summarizer.submit(text);
    let _ = handle.wait(&|| true, Duration::from_secs(2));
    let got = seen.lock().expect("lock").clone();
    assert_eq!(
        got,
        Some((ASK_SUMMARY_SYSTEM.to_string(), summary_user_prompt(text))),
        "system must be ASK_SUMMARY_SYSTEM and user must be summary_user_prompt(text)"
    );
}

#[test]
fn gateway_ok_answer_reaches_the_waiter_unchanged() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, _u| Ok("A summary.".to_string())));
    let handle = summarizer.submit("result");
    let answer = handle.wait(&|| true, Duration::from_secs(2));
    assert_eq!(answer, Ok("A summary.".to_string()));
}

#[test]
fn gateway_err_answer_becomes_failed() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, _u| Err(())));
    let handle = summarizer.submit("result");
    let answer = handle.wait(&|| true, Duration::from_secs(2));
    assert_eq!(answer, Err(SummaryError::Failed));
}

#[test]
fn gateway_panicking_completion_is_contained_as_failed() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, _u| {
        panic!("deliberate panic in completion for M4 S6 test");
    }));
    let started = Instant::now();
    let handle = summarizer.submit("result");
    let answer = handle.wait(&|| true, Duration::from_secs(2));
    assert_eq!(
        answer,
        Err(SummaryError::Failed),
        "a panicking completion must surface as Failed, not hang or propagate"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "must not wait out the deadline"
    );
}

#[test]
fn gateway_wait_aborts_promptly_when_current_flips_and_discards_the_late_answer() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, _u| {
        std::thread::sleep(Duration::from_millis(600));
        Ok("late answer".to_string())
    }));
    let flag = Arc::new(AtomicBool::new(true));
    let flipper = Arc::clone(&flag);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        flipper.store(false, Ordering::SeqCst);
    });
    let current = Arc::clone(&flag);
    let handle = summarizer.submit("result");
    let started = Instant::now();
    let answer = handle.wait(
        &move || current.load(Ordering::SeqCst),
        Duration::from_secs(2),
    );
    let took = started.elapsed();
    assert_eq!(answer, Err(SummaryError::Aborted));
    assert!(
        took < Duration::from_millis(400),
        "abort must be prompt, while the completion still sleeps (took {took:?})"
    );
    // Let the late answer arrive; it must be discarded without a panic.
    std::thread::sleep(Duration::from_millis(700));
}

#[test]
fn gateway_two_submits_are_independent() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, user| Ok(user.to_string())));
    let first = summarizer.submit("first result");
    let second = summarizer.submit("second result");
    let a = first.wait(&|| true, Duration::from_secs(2));
    let b = second.wait(&|| true, Duration::from_secs(2));
    assert_eq!(a, Ok(summary_user_prompt("first result")));
    assert_eq!(b, Ok(summary_user_prompt("second result")));
}

#[test]
fn gateway_empty_text_is_still_submitted_and_answered() {
    let summarizer = GatewaySummarizer::new(complete_fn(|_s, user| Ok(user.to_string())));
    let handle = summarizer.submit("");
    let answer = handle.wait(&|| true, Duration::from_secs(2));
    assert_eq!(answer, Ok(summary_user_prompt("")));
}

// ---------------------------------------------------------------- install_summary_hooks

#[cfg(unix)]
#[test]
fn install_hooks_admit_the_started_job_and_drop_the_previous_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "sleep 30");
    let mgr = fast_manager();
    let tokens = SummaryTokens::new();
    install_summary_hooks(&mgr, Arc::clone(&tokens));

    let first = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("first start");
    assert!(
        tokens.begin(first).is_some(),
        "admit hook must make the started job current"
    );
    stop_and_wait(&mgr);

    let second = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("second start");
    assert!(tokens.begin(second).is_some(), "second job must be current");
    assert!(
        tokens.begin(first).is_none(),
        "the earlier job must no longer be current"
    );
    stop_and_wait(&mgr);
}

#[cfg(unix)]
#[test]
fn close_admission_closes_the_tokens_for_good() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "sleep 30");
    let mgr = fast_manager();
    let tokens = SummaryTokens::new();
    install_summary_hooks(&mgr, Arc::clone(&tokens));

    let id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    let ticket = tokens.begin(id).expect("ticket before close");
    mgr.close_admission();
    assert!(
        !tokens.is_current(&ticket),
        "close must invalidate the ticket"
    );
    assert!(
        tokens.begin(id).is_none(),
        "no ticket is issued after close"
    );
    stop_and_wait(&mgr);
}

#[cfg(unix)]
#[test]
fn direct_shutdown_without_exit_coordinator_also_closes_the_tokens() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "sleep 30");
    let mgr = fast_manager();
    let tokens = SummaryTokens::new();
    install_summary_hooks(&mgr, Arc::clone(&tokens));

    let id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    let ticket = tokens.begin(id).expect("ticket before shutdown");
    assert!(
        mgr.shutdown(&quick_shutdown()),
        "shutdown must finish cleanly"
    );
    assert!(
        !tokens.is_current(&ticket),
        "shutdown must invalidate the ticket"
    );
    assert!(tokens.begin(id).is_none(), "no ticket after shutdown");
}

#[cfg(unix)]
#[test]
fn installing_hooks_twice_replaces_the_first_set_without_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "sleep 30");
    let mgr = fast_manager();
    let old_tokens = SummaryTokens::new();
    let new_tokens = SummaryTokens::new();
    install_summary_hooks(&mgr, Arc::clone(&old_tokens));
    install_summary_hooks(&mgr, Arc::clone(&new_tokens));

    let id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    assert!(
        new_tokens.begin(id).is_some(),
        "the second install must admit"
    );
    assert!(
        old_tokens.begin(id).is_none(),
        "the replaced hooks must no longer touch the old tokens"
    );
    stop_and_wait(&mgr);
}

// ---------------------------------------------------------------- summary_policy

#[test]
fn summary_policy_defaults_are_both_off() {
    let shared = Arc::new(Mutex::new(get_default_settings()));
    let policy = summary_policy(live_getter(&shared));
    assert!(!(policy.ask_enabled)(), "ask defaults off");
    assert!(!(policy.tts_enabled)(), "tts defaults off");
}

#[test]
fn summary_policy_reads_current_values_for_all_four_combinations_without_rebuilding() {
    let shared = Arc::new(Mutex::new(get_default_settings()));
    let policy = summary_policy(live_getter(&shared));
    for (ask, tts) in [(false, false), (true, false), (false, true), (true, true)] {
        set_switches(&shared, ask, tts);
        assert_eq!((policy.ask_enabled)(), ask, "ask for ({ask}, {tts})");
        assert_eq!((policy.tts_enabled)(), tts, "tts for ({ask}, {tts})");
    }
}

#[test]
fn summary_policy_sees_a_flip_back_to_off_immediately() {
    let shared = Arc::new(Mutex::new(get_default_settings()));
    let policy = summary_policy(live_getter(&shared));
    set_switches(&shared, true, true);
    assert!((policy.ask_enabled)());
    set_switches(&shared, false, true);
    assert!(
        !(policy.ask_enabled)(),
        "a switch-off must be seen on the next call"
    );
}

// ---------------------------------------------------------------- status_dto_with

#[test]
fn status_dto_with_an_idle_manager_is_idle_with_no_result() {
    let mgr = fast_manager();
    let results = ResultStore::new();
    let dto = status_dto_with(&mgr, &results);
    assert_eq!(dto.state, "idle");
    assert_eq!(dto.result, None);
}

#[cfg(unix)]
#[test]
fn status_dto_with_a_finished_ask_job_exposes_its_stored_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "exit 0");
    let mgr = fast_manager();
    let results = ResultStore::new();
    let id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    assert!(wait_until(Duration::from_secs(10), || !mgr.has_active_job()));
    results.set(id, "answer".to_string());

    let dto = status_dto_with(&mgr, &results);
    assert_eq!(dto.state, "succeeded");
    assert_eq!(dto.result, Some("answer".to_string()));
    let again = status_dto_with(&mgr, &results);
    assert_eq!(
        again.result,
        Some("answer".to_string()),
        "result persists across calls"
    );
}

#[cfg(unix)]
#[test]
fn status_dto_with_a_store_holding_another_job_shows_no_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "exit 0");
    let mgr = fast_manager();
    let results = ResultStore::new();
    let id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    assert!(wait_until(Duration::from_secs(10), || !mgr.has_active_job()));
    results.set(id + 1000, "someone else".to_string());

    let dto = status_dto_with(&mgr, &results);
    assert_eq!(
        dto.result, None,
        "a stored text for another job id must not leak"
    );
}

#[cfg(unix)]
#[test]
fn status_dto_with_a_job_without_a_stored_result_shows_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = write_cli(dir.path(), "exit 0");
    let mgr = fast_manager();
    let results = ResultStore::new();
    let _id = start_job(&mgr, dir.path(), &bin, JobMode::Ask).expect("start");
    assert!(wait_until(Duration::from_secs(10), || !mgr.has_active_job()));

    let dto = status_dto_with(&mgr, &results);
    assert_eq!(dto.state, "succeeded");
    assert_eq!(dto.result, None);
}

// ---------------------------------------------------------------- invalidate_summaries

#[test]
fn invalidate_summaries_none_is_a_no_op() {
    invalidate_summaries(None);
}

#[test]
fn invalidate_summaries_bumps_the_epoch_of_outstanding_tickets() {
    let tokens = SummaryTokens::new();
    tokens.admit(7);
    let ticket = tokens.begin(7).expect("ticket for the admitted job");
    assert!(
        tokens.is_current(&ticket),
        "precondition: ticket is current"
    );
    invalidate_summaries(Some(&tokens));
    assert!(
        !tokens.is_current(&ticket),
        "a ticket taken before the invalidation must be invalid after it"
    );
    assert_eq!(
        tokens.run_if_valid(&ticket, |valid| valid),
        false,
        "the gated check must also reject the old ticket"
    );
}
