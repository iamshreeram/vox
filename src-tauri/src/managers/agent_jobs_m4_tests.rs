//! M4 S2 RED tests (DESIGN Revision 4: D4 argv, D5 result capture and on_result
//! delivery, D6 admission and close hooks). Written before the implementation:
//! they must compile against the stubs and fail at runtime. Do not edit these
//! tests to make code pass.
#![cfg(unix)]

use super::{
    ExitConfig, ExitCoordinator, JobError, JobHooks, JobManager, JobObserver, JobResult,
    JobSnapshot, JobSpec, JobState, ShutdownConfig,
};
use crate::managers::agent_jobs_output::ResultCapture;
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
use crate::managers::converse_plan::{JobMode, JobSelection};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// Hard cap for any single wait in this file.
const GUARD: Duration = Duration::from_secs(12);

// ---- shared harness ---------------------------------------------------------

/// Ordered event log shared by observers and hooks.
#[derive(Default)]
struct Log {
    events: Mutex<Vec<String>>,
}

impl Log {
    fn push(&self, event: String) {
        if let Ok(mut v) = self.events.lock() {
            v.push(event);
        }
    }
    fn all(&self) -> Vec<String> {
        self.events.lock().map(|v| v.clone()).unwrap_or_default()
    }
    fn index_of(&self, prefix: &str) -> Option<usize> {
        self.all().iter().position(|e| e.starts_with(prefix))
    }
    fn count(&self, prefix: &str) -> usize {
        self.all().iter().filter(|e| e.starts_with(prefix)).count()
    }
    /// Texts of all `result:<id>:<text>` events, in delivery order.
    fn result_texts(&self) -> Vec<String> {
        self.all()
            .iter()
            .filter_map(|e| e.strip_prefix("result:"))
            .filter_map(|r| r.split_once(':').map(|(_, t)| t.to_string()))
            .collect()
    }
}

/// Records progress, result and finish events into a shared log.
struct LogObserver {
    log: Arc<Log>,
}

impl JobObserver for LogObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {
        self.log.push("progress".to_string());
    }
    fn on_finish(&self, snap: &JobSnapshot) {
        self.log.push(format!("finish:{}", snap.id));
    }
    fn on_result(&self, r: &JobResult) {
        self.log.push(format!("result:{}:{}", r.id, r.text));
    }
}

/// Records the attempt, then panics inside on_result.
struct PanicResult {
    log: Arc<Log>,
}

impl JobObserver for PanicResult {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, snap: &JobSnapshot) {
        self.log.push(format!("finish:{}", snap.id));
    }
    fn on_result(&self, _r: &JobResult) {
        self.log.push("result-attempt".to_string());
        panic!("deliberate on_result panic for M4 S2 test");
    }
}

/// Signals entry into on_result, then blocks until released.
struct BlockingResult {
    log: Arc<Log>,
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl JobObserver for BlockingResult {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, snap: &JobSnapshot) {
        self.log.push(format!("finish:{}", snap.id));
    }
    fn on_result(&self, r: &JobResult) {
        if let Ok(tx) = self.entered.lock() {
            let _ = tx.send(());
        }
        if let Ok(rx) = self.release.lock() {
            let _ = rx.recv_timeout(GUARD);
        }
        self.log.push(format!("result:{}:{}", r.id, r.text));
    }
}

fn file_id(path: &Path) -> FileId {
    // A missing binary gets a placeholder id so start() itself decides the refusal.
    match std::fs::metadata(path) {
        Ok(meta) => FileId {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime_secs: meta.mtime(),
        },
        Err(_) => FileId {
            dev: 0,
            ino: 0,
            len: 0,
            mtime_secs: 0,
        },
    }
}

/// Writes an executable `sh` script named `name` into `dir`.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn pinned_dir(dir: &Path) -> OwnedFd {
    std::fs::File::open(dir).expect("open project dir").into()
}

fn launch(dir: &Path, binary: &Path, task: &str, timeout: Duration) -> JobLaunch {
    JobLaunch {
        project: "proj".to_string(),
        dir: pinned_dir(dir),
        task: task.to_string(),
        config: LaunchConfig {
            binary: binary.to_path_buf(),
            binary_id: file_id(binary),
            prompt_flag: "-p".to_string(),
            agent_flag: None,
            timeout,
        },
        selection: JobSelection::default(),
    }
}

fn ask_launch(dir: &Path, binary: &Path, timeout: Duration) -> JobLaunch {
    let mut l = launch(dir, binary, "question", timeout);
    l.selection = JobSelection {
        agent: None,
        mode: JobMode::Ask,
    };
    l
}

fn spec_of(launch: JobLaunch) -> JobSpec {
    JobSpec {
        launch,
        progress_interval: Duration::from_secs(90),
    }
}

/// Polls `cond` until true or `limit` elapses. Returns the final value.
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

fn wait_terminal(mgr: &JobManager, limit: Duration) -> Option<JobSnapshot> {
    let mut last = None;
    wait_until(limit, || {
        last = mgr.snapshot();
        last.as_ref()
            .is_some_and(|s| !matches!(s.state, JobState::Running))
    });
    last
}

fn quick_shutdown() -> ShutdownConfig {
    ShutdownConfig {
        term_grace: Duration::from_millis(200),
        drain: Duration::from_millis(200),
        deadline: Duration::from_secs(5),
    }
}

/// Installs hooks that record admissions into `log` and count closes.
fn install_hooks(mgr: &JobManager, log: &Arc<Log>) -> (Arc<Mutex<Vec<u64>>>, Arc<AtomicUsize>) {
    let admits = Arc::new(Mutex::new(Vec::<u64>::new()));
    let closes = Arc::new(AtomicUsize::new(0));
    let (a, c, l, l2) = (
        Arc::clone(&admits),
        Arc::clone(&closes),
        Arc::clone(log),
        Arc::clone(log),
    );
    mgr.set_hooks(JobHooks {
        on_admit: Arc::new(move |id| {
            if let Ok(mut v) = a.lock() {
                v.push(id);
            }
            l.push(format!("admit:{id}"));
        }),
        on_close: Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
            l2.push("close".to_string());
        }),
    });
    (admits, closes)
}

// ---- ResultCapture unit tests (D5 bounded capture) --------------------------

#[test]
fn result_capture_empty_yields_empty_text() {
    let cap = ResultCapture::new();
    assert_eq!(cap.text(), "");
}

#[test]
fn result_capture_one_line_is_returned_as_is() {
    let mut cap = ResultCapture::new();
    cap.push_line("only line");
    assert_eq!(cap.text(), "only line");
}

#[test]
fn result_capture_joins_lines_with_newline_and_no_trailing_newline() {
    let mut cap = ResultCapture::new();
    cap.push_line("a");
    cap.push_line("b");
    cap.push_line("c");
    assert_eq!(cap.text(), "a\nb\nc");
}

#[test]
fn result_capture_total_exactly_8192_is_kept_whole() {
    let mut cap = ResultCapture::new();
    let first = "x".repeat(4095);
    let second = "y".repeat(4096);
    cap.push_line(&first);
    cap.push_line(&second);
    let text = cap.text();
    assert_eq!(text.len(), 8192);
    assert_eq!(text, format!("{first}\n{second}"));
}

#[test]
fn result_capture_total_8193_drops_oldest_whole_line() {
    let mut cap = ResultCapture::new();
    let first = "x".repeat(4096);
    let second = "y".repeat(4096);
    cap.push_line(&first);
    cap.push_line(&second);
    assert_eq!(cap.text(), second, "oldest whole line must be dropped");
}

#[test]
fn result_capture_single_10000_byte_line_keeps_last_8192_bytes() {
    let mut cap = ResultCapture::new();
    let line: String = (0..10_000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    cap.push_line(&line);
    let text = cap.text();
    assert_eq!(text.len(), 8192);
    assert_eq!(text, line[line.len() - 8192..]);
}

#[test]
fn result_capture_three_byte_chars_straddling_cut_stay_valid_utf8() {
    let mut cap = ResultCapture::new();
    let line = format!("a{}", "\u{20AC}".repeat(3333));
    assert_eq!(line.len(), 10_000);
    cap.push_line(&line);
    let text = cap.text();
    assert!(text.len() <= 8192, "len {} over cap", text.len());
    assert!(text.ends_with('\u{20AC}'));
    assert!(std::str::from_utf8(text.as_bytes()).is_ok());
}

#[test]
fn result_capture_thousand_lines_keeps_only_newest_and_ends_with_last() {
    let mut cap = ResultCapture::new();
    for i in 1..=1000 {
        let line = format!("{i:03}{}", "-".repeat(97));
        assert_eq!(line.len(), 100);
        cap.push_line(&line);
    }
    let text = cap.text();
    assert!(text.len() <= 8192);
    assert!(text.ends_with(&format!("1000{}", "-".repeat(96))));
    assert!(!text.contains("001---"), "oldest line must be gone");
    assert!(
        text.lines().count() >= 80,
        "keeps roughly the newest 81 lines"
    );
}

// ---- D4 argv ----------------------------------------------------------------

/// Runs a fake CLI that writes its argv NUL-separated, returns the args.
fn argv_for(selection: JobSelection, agent_flag: Option<&str>, task: &str) -> Option<Vec<String>> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out = tmp.path().join("argv.bin");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!("printf '%s\\0' \"$@\" > '{}'", out.display()),
    );
    let mut l = launch(tmp.path(), &bin, task, Duration::from_secs(20));
    l.selection = selection;
    l.config.agent_flag = agent_flag.map(str::to_string);
    let mgr = JobManager::new();
    let obs = Arc::new(Log::default());
    let started = mgr.start(
        spec_of(l),
        Arc::new(LogObserver {
            log: Arc::clone(&obs),
        }),
    );
    if started.is_err() {
        return None;
    }
    let _ = wait_terminal(&mgr, GUARD);
    let bytes = std::fs::read(&out).unwrap_or_default();
    let mut args: Vec<String> = bytes
        .split(|b| *b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    // The script ends every argument with NUL, so the trailing empty piece is not an argument.
    if args.last().is_some_and(|s| s.is_empty()) {
        args.pop();
    }
    Some(args)
}

fn sel(agent: Option<&str>) -> JobSelection {
    JobSelection {
        agent: agent.map(str::to_string),
        mode: JobMode::Task,
    }
}

#[test]
fn argv_without_agent_or_flag_is_exactly_prompt_flag_and_task() {
    let args = argv_for(sel(None), None, "fix the bug");
    assert_eq!(
        args,
        Some(vec!["-p".to_string(), "fix the bug".to_string()])
    );
}

#[test]
fn argv_with_agent_and_default_flag_is_flag_agent_prompt_task() {
    let args = argv_for(sel(Some("coder-one")), Some("--agent"), "task");
    assert_eq!(
        args,
        Some(vec![
            "--agent".to_string(),
            "coder-one".to_string(),
            "-p".to_string(),
            "task".to_string()
        ])
    );
}

#[test]
fn argv_with_custom_short_flag_uses_that_flag() {
    let args = argv_for(sel(Some("name")), Some("-a"), "task");
    assert_eq!(
        args,
        Some(vec![
            "-a".to_string(),
            "name".to_string(),
            "-p".to_string(),
            "task".to_string()
        ])
    );
}

#[test]
fn argv_task_with_spaces_quotes_dollar_backticks_and_newlines_is_one_argument() {
    let task = "say \"hi\" $HOME `id` it's\nsecond line; rm -rf /";
    let args = argv_for(sel(None), None, task);
    assert_eq!(args, Some(vec!["-p".to_string(), task.to_string()]));
}

#[test]
fn argv_agent_name_with_allowed_charset_is_passed_unchanged() {
    let args = argv_for(sel(Some("ram.ai_v-2")), Some("--agent"), "task");
    assert_eq!(
        args.and_then(|a| a.get(1).cloned()),
        Some("ram.ai_v-2".to_string())
    );
}

#[test]
fn agent_without_flag_is_refused_and_no_child_runs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let marker = tmp.path().join("ran.marker");
    let bin = write_script(tmp.path(), "cli", &format!("touch '{}'", marker.display()));
    let mut l = launch(tmp.path(), &bin, "task", Duration::from_secs(20));
    l.selection = sel(Some("coder-one"));
    l.config.agent_flag = None;
    let mgr = JobManager::new();
    let result = mgr.start(
        spec_of(l),
        Arc::new(LogObserver {
            log: Arc::new(Log::default()),
        }),
    );
    assert!(
        result.is_err(),
        "agent without a flag must be refused, got {result:?}"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !marker.exists(),
        "no child may run for a refused agent start"
    );
}

// ---- D5 on_result delivery --------------------------------------------------

#[test]
fn ask_job_delivers_one_result_before_finish_with_printed_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "echo first-answer-line; echo second-line",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let id = mgr
        .start(
            spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
            Arc::new(LogObserver {
                log: Arc::clone(&log),
            }),
        )
        .expect("ask job starts");
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(
        log.count("result:"),
        1,
        "exactly one on_result for an ask job"
    );
    let text = log.result_texts().pop().unwrap_or_default();
    assert_eq!(text, "first-answer-line\nsecond-line");
    let r = log.index_of("result:").expect("result index");
    let f = log.index_of("finish:").expect("finish index");
    assert!(r < f, "on_result must precede on_finish");
    assert!(log.all()[r].starts_with(&format!("result:{id}:")));
}

#[test]
fn task_job_with_same_output_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo first-answer-line");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    must_start_task(&mgr, tmp.path(), &bin, Arc::clone(&log));
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0, "task jobs never deliver a result");
}

fn must_start_task(mgr: &Arc<JobManager>, dir: &Path, bin: &Path, log: Arc<Log>) -> u64 {
    match mgr.start(
        spec_of(launch(dir, bin, "task", Duration::from_secs(20))),
        Arc::new(LogObserver { log }),
    ) {
        Ok(id) => id,
        Err(_) => panic!("task start must succeed"),
    }
}

#[test]
fn ask_job_non_zero_exit_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo partial-answer; exit 3");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_job_cancelled_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo partial-answer; sleep 60");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || mgr
        .snapshot()
        .is_some_and(|s| !s.tail.is_empty())));
    assert!(mgr.cancel());
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_job_timed_out_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo partial-answer; sleep 60");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_millis(400))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_job_spawn_failure_delivers_no_result_and_no_finish() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("no-such-cli");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let started = mgr.start(
        spec_of(ask_launch(tmp.path(), &missing, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(started.is_err());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_job_with_empty_output_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_job_with_whitespace_only_output_delivers_no_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo '   '; echo '\t'");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn ask_result_over_8_kib_is_bounded_and_ends_with_last_line() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "i=0; while [ $i -lt 2000 ]; do echo line-$i-padding-text-here; i=$((i+1)); done; echo final-marker-line",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(text.len() <= 8192, "len {} over 8192", text.len());
    assert!(text.ends_with("final-marker-line"));
}

#[test]
fn ask_result_fake_token_printed_whole_is_absent_from_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let token = concat!("xox", "b-", "1234567890-abcdefghij");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!("echo 'before {token} after'; echo visible-line"),
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(
        !text.contains("1234567890-abcdefghij"),
        "token leaked: {text}"
    );
    assert!(text.contains("visible-line"));
}

#[test]
fn ask_result_token_split_across_writes_is_absent_from_result() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "printf 'xoxb-'; sleep 0.3; printf '1234567890-abcdefghij\\n'; echo done-line",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(
        !text.contains("1234567890-abcdefghij"),
        "split token leaked: {text}"
    );
    assert!(text.contains("done-line"));
}

#[test]
fn ask_result_private_key_block_across_writes_has_no_body_lines() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let begin = concat!("-----BEGIN ", "PRIVATE KEY-----");
    let end = concat!("-----END ", "PRIVATE KEY-----");
    let bin = write_script(
        tmp.path(),
        "cli",
        &format!(
            "echo '{begin}'; echo 'MIIBOgIBAAJBAKj34GkxFhD90vcNLYE'; sleep 0.2; echo 'QWxhZGRpbjpvcGVuIHNlc2FtZQ=='; echo '{end}'; echo after-key-line"
        ),
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(!text.contains("MIIBOgIBAAJBAKj34GkxFhD90vcNLYE"));
    assert!(!text.contains("QWxhZGRpbjpvcGVuIHNlc2FtZQ=="));
    assert!(text.contains("after-key-line"));
}

#[test]
fn ask_result_invalid_utf8_and_control_chars_yield_clean_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "printf 'ok\\377\\001 bad\\033[31m red\\n'; echo tail-line",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(
        !text.chars().any(|c| c.is_control() && c != '\n'),
        "control characters survived: {text:?}"
    );
    assert!(text.contains("tail-line"));
}

#[test]
fn ask_result_50_000_byte_line_is_bounded() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "head -c 50000 /dev/zero | tr '\\0' 'a'; echo; echo tail-after-long",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    let text = log.result_texts().pop().unwrap_or_default();
    assert!(text.len() <= 8192, "len {} over 8192", text.len());
    assert!(text.ends_with("tail-after-long"));
}

#[test]
fn panicking_on_result_is_isolated_and_manager_accepts_next_job() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo answer-line");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(PanicResult {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(
        log.count("result-attempt"),
        1,
        "on_result must be attempted"
    );
    let next = mgr.start(
        spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::new(Log::default()),
        }),
    );
    assert!(next.is_ok(), "manager must accept a new job after a panic");
}

#[test]
fn blocking_on_result_does_not_block_cancel_or_admission_and_finish_stays_ordered() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ask_bin = write_script(tmp.path(), "ask-cli", "echo answer-line");
    let sleep_bin = write_script(tmp.path(), "sleep-cli", "sleep 60");
    let mgr = JobManager::new();
    let log1 = Arc::new(Log::default());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let id1 = mgr
        .start(
            spec_of(ask_launch(tmp.path(), &ask_bin, Duration::from_secs(20))),
            Arc::new(BlockingResult {
                log: Arc::clone(&log1),
                entered: Mutex::new(entered_tx),
                release: Mutex::new(release_rx),
            }),
        )
        .expect("ask job starts");
    assert!(
        entered_rx.recv_timeout(GUARD).is_ok(),
        "on_result must be entered"
    );
    let log2 = Arc::new(Log::default());
    let id2 = mgr
        .start(
            spec_of(launch(
                tmp.path(),
                &sleep_bin,
                "task",
                Duration::from_secs(20),
            )),
            Arc::new(LogObserver {
                log: Arc::clone(&log2),
            }),
        )
        .expect("second job admitted while on_result is blocked");
    assert!(id2 > id1);
    let t = Instant::now();
    assert!(mgr.cancel(), "cancel must stop the second job");
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "cancel must not wait on delivery"
    );
    release_tx.send(()).expect("release on_result");
    assert!(wait_until(GUARD, || log1.count("finish:") == 1));
    let r = log1.index_of("result:").expect("result delivered");
    let f = log1.index_of("finish:").expect("finish delivered");
    assert!(r < f, "result must precede finish after release");
}

#[test]
fn on_result_is_delivered_at_most_once_under_many_output_chunks() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(
        tmp.path(),
        "cli",
        "i=0; while [ $i -lt 300 ]; do echo chunk-$i; i=$((i+1)); done",
    );
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(ask_launch(tmp.path(), &bin, Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 1, "at most one on_result per job");
}

// ---- D6 hooks ---------------------------------------------------------------

#[test]
fn on_admit_is_called_once_per_admitted_job_with_returned_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    let id = mgr
        .start(
            spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
            Arc::new(LogObserver {
                log: Arc::clone(&log),
            }),
        )
        .expect("start");
    let got = admits.lock().map(|v| v.clone()).unwrap_or_default();
    assert_eq!(got, vec![id], "on_admit once with the returned id");
}

#[test]
fn on_admit_precedes_every_observer_event_of_the_job_over_30_races() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    for round in 0..30 {
        let mgr = JobManager::new();
        let log = Arc::new(Log::default());
        let _ = install_hooks(&mgr, &log);
        let id = mgr
            .start(
                spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
                Arc::new(LogObserver {
                    log: Arc::clone(&log),
                }),
            )
            .expect("start");
        assert!(
            wait_until(GUARD, || log.count("finish:") == 1),
            "round {round}"
        );
        let admit = log.index_of(&format!("admit:{id}"));
        let first = log.all().iter().position(|e| !e.starts_with("admit:"));
        assert!(admit.is_some(), "round {round}: admit missing");
        assert_eq!(
            admit,
            Some(0),
            "round {round}: admit must be first, log {:?}",
            log.all()
        );
        assert!(first.is_some_and(|f| f > 0));
    }
}

#[test]
fn rejected_busy_start_does_not_call_on_admit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sleep_bin = write_script(tmp.path(), "sleep-cli", "sleep 60");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    let _first = mgr
        .start(
            spec_of(launch(
                tmp.path(),
                &sleep_bin,
                "task",
                Duration::from_secs(20),
            )),
            Arc::new(LogObserver {
                log: Arc::clone(&log),
            }),
        )
        .expect("first start");
    let second = mgr.start(
        spec_of(launch(
            tmp.path(),
            &sleep_bin,
            "task",
            Duration::from_secs(20),
        )),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(matches!(second, Err(JobError::Busy)));
    let got = admits.lock().map(|v| v.len()).unwrap_or(0);
    assert_eq!(got, 1, "busy start must not call on_admit");
    mgr.cancel();
}

#[test]
fn rejected_bad_binary_start_does_not_call_on_admit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    let bad = launch(
        tmp.path(),
        &PathBuf::from("no-such-cli"),
        "task",
        Duration::from_secs(20),
    );
    let r = mgr.start(
        spec_of(bad),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(matches!(r, Err(JobError::BadBinary)));
    assert_eq!(admits.lock().map(|v| v.len()).unwrap_or(9), 0);
}

#[test]
fn rejected_bad_dir_start_does_not_call_on_admit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let file_as_dir = tmp.path().join("plain-file");
    std::fs::write(&file_as_dir, "x").expect("write file");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    let r = mgr.start(
        spec_of(JobLaunch {
            dir: std::fs::File::open(&file_as_dir).expect("open").into(),
            ..launch(tmp.path(), &bin, "task", Duration::from_secs(20))
        }),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(matches!(r, Err(JobError::BadDir)));
    assert_eq!(admits.lock().map(|v| v.len()).unwrap_or(9), 0);
}

#[test]
fn rejected_shutting_down_start_does_not_call_on_admit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    mgr.close_admission();
    let r = mgr.start(
        spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(matches!(r, Err(JobError::ShuttingDown)));
    assert_eq!(admits.lock().map(|v| v.len()).unwrap_or(9), 0);
}

#[test]
fn job_ids_increase_and_match_on_admit_values() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (admits, _) = install_hooks(&mgr, &log);
    let mut ids = Vec::new();
    for _ in 0..2 {
        let id = mgr
            .start(
                spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
                Arc::new(LogObserver {
                    log: Arc::clone(&log),
                }),
            )
            .expect("start");
        assert!(wait_terminal(&mgr, GUARD).is_some());
        ids.push(id);
    }
    assert!(ids[1] > ids[0], "ids must increase");
    let got = admits.lock().map(|v| v.clone()).unwrap_or_default();
    assert_eq!(got, ids, "on_admit ids equal the ids start returned");
}

#[test]
fn on_close_is_called_after_close_admission() {
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (_, closes) = install_hooks(&mgr, &log);
    mgr.close_admission();
    assert!(
        closes.load(Ordering::SeqCst) >= 1,
        "on_close must run on close_admission"
    );
}

#[test]
fn on_close_runs_before_direct_shutdown_kills_a_running_child() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sleep_bin = write_script(tmp.path(), "sleep-cli", "sleep 60");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (_, closes) = install_hooks(&mgr, &log);
    let _ = mgr.start(
        spec_of(launch(
            tmp.path(),
            &sleep_bin,
            "task",
            Duration::from_secs(60),
        )),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(wait_until(GUARD, || mgr.has_active_job()));
    let clean = mgr.shutdown(&quick_shutdown());
    assert!(clean, "shutdown must stop the child");
    assert!(
        closes.load(Ordering::SeqCst) >= 1,
        "on_close must run on shutdown"
    );
    let close_at = log.index_of("close");
    let finish_at = log.index_of("finish:");
    assert!(
        close_at.is_some(),
        "on_close must be logged at shutdown start"
    );
    assert!(
        close_at < finish_at,
        "on_close must precede the kill and finish, log {:?}",
        log.all()
    );
}

#[test]
fn exit_coordinator_request_path_fires_on_close() {
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let (_, closes) = install_hooks(&mgr, &log);
    let (done_tx, done_rx) = mpsc::channel();
    ExitCoordinator::new().request_exit(
        &mgr,
        move |_r| {
            let _ = done_tx.send(());
        },
        &ExitConfig::default(),
    );
    assert!(
        done_rx.recv_timeout(GUARD).is_ok(),
        "exit cleanup must finish"
    );
    assert!(
        closes.load(Ordering::SeqCst) >= 1,
        "exit path must fire on_close"
    );
}

#[test]
fn no_hooks_installed_leaves_job_behaviour_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "echo plain-output");
    let mgr = JobManager::new();
    let log = Arc::new(Log::default());
    let _ = mgr.start(
        spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    let snap = wait_terminal(&mgr, GUARD).expect("snapshot");
    assert!(matches!(snap.state, JobState::Succeeded));
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
    assert_eq!(log.count("result:"), 0);
}

#[test]
fn panicking_on_admit_is_isolated_and_start_still_returns_id() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "true");
    let mgr = JobManager::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    mgr.set_hooks(JobHooks {
        on_admit: Arc::new(move |_id| {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("deliberate on_admit panic for M4 S2 test");
        }),
        on_close: Arc::new(|| {}),
    });
    let log = Arc::new(Log::default());
    let r = mgr.start(
        spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20))),
        Arc::new(LogObserver {
            log: Arc::clone(&log),
        }),
    );
    assert!(r.is_ok(), "start must still return the id");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "on_admit must have been called"
    );
    assert!(wait_until(GUARD, || log.count("finish:") == 1));
}

#[test]
fn panicking_on_close_is_isolated_and_shutdown_completes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let sleep_bin = write_script(tmp.path(), "sleep-cli", "sleep 60");
    let mgr = JobManager::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    mgr.set_hooks(JobHooks {
        on_admit: Arc::new(|_id| {}),
        on_close: Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
            panic!("deliberate on_close panic for M4 S2 test");
        }),
    });
    let _ = mgr.start(
        spec_of(launch(
            tmp.path(),
            &sleep_bin,
            "task",
            Duration::from_secs(60),
        )),
        Arc::new(LogObserver {
            log: Arc::new(Log::default()),
        }),
    );
    assert!(wait_until(GUARD, || mgr.has_active_job()));
    assert!(mgr.shutdown(&quick_shutdown()), "shutdown must complete");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "on_close must have been called"
    );
}

// Owner ruling (design D6): hooks run synchronously on the thread that calls
// start(), under the manager's own lock, and MUST NOT call back into the
// manager. Production hooks only touch the summary tokens. This test pins the
// part that is observable: the admit hook has returned before start() returns.
#[test]
fn on_admit_runs_synchronously_on_the_starting_thread_before_start_returns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = write_script(tmp.path(), "cli", "sleep 60");
    let mgr = JobManager::new();
    let hook_thread = Arc::new(Mutex::new(None));
    let h = Arc::clone(&hook_thread);
    mgr.set_hooks(JobHooks {
        on_admit: Arc::new(move |_id| {
            if let Ok(mut g) = h.lock() {
                *g = Some(std::thread::current().id());
            }
        }),
        on_close: Arc::new(|| {}),
    });
    let spec = spec_of(launch(tmp.path(), &bin, "task", Duration::from_secs(20)));
    let res = mgr.start(
        spec,
        Arc::new(LogObserver {
            log: Arc::new(Log::default()),
        }),
    );
    assert!(res.is_ok(), "start must succeed");
    let seen = hook_thread.lock().expect("lock").take();
    assert_eq!(
        seen,
        Some(std::thread::current().id()),
        "on_admit must have run on the thread that called start(), before it returned"
    );
    let _ = mgr.cancel();
}
