//! App wiring tests (M3 S3d, DESIGN Rev 5 R2-8, R4-2, R5-1, D6, D7). Red by design: the
//! bodies in `job_runtime.rs` are `todo!()` and the wiring markers still exist in lib.rs and
//! commands/agent_jobs.rs. Do not edit these tests to make code pass.
//!
//! AppHandle cannot be built in a unit test, so the tests cover the pure helpers, a real
//! JobManager driven by a fake `sh` CLI, and source contracts over the app's own files.

use super::{job_ui_running, ManagerStatusSource};
use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobSnapshot};
use crate::managers::agent_tools::job_status::JobStatusSource;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Builds a status DTO with the given state, for the table tests.
fn dto(state: &str, project: Option<&str>, elapsed_secs: u64) -> AgentJobStatusDto {
    AgentJobStatusDto {
        state: state.to_string(),
        project: project.map(str::to_string),
        elapsed_secs,
    }
}

#[test]
fn job_ui_running_is_true_for_the_running_state() {
    assert!(job_ui_running(&dto("running", Some("proj"), 3)));
}

#[test]
fn job_ui_running_is_false_for_idle_and_every_terminal_state() {
    for state in [
        "idle",
        "succeeded",
        "failed",
        "cancelled",
        "timed_out",
        "spawn_failed",
    ] {
        assert!(
            !job_ui_running(&dto(state, None, 0)),
            "state {state:?} must not count as running"
        );
    }
}

#[test]
fn job_ui_running_rejects_case_whitespace_and_empty_variants() {
    for state in ["Running", "RUNNING", " running", "running ", "", "runn"] {
        assert!(
            !job_ui_running(&dto(state, Some("proj"), 1)),
            "variant {state:?} must not count as running"
        );
    }
}

#[test]
fn job_ui_running_ignores_project_and_elapsed_seconds() {
    assert!(job_ui_running(&dto("running", None, 0)));
    assert!(job_ui_running(&dto("running", Some("other"), 999)));
}

/// Real-process tests: a JobManager driven by a fake `sh` CLI in a temp dir. Unix only.
#[cfg(unix)]
mod process_tests {
    use super::*;
    use crate::managers::agent_jobs::{JobSpec, JobState};
    use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
    use std::os::fd::OwnedFd;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::sync::Arc;

    /// Kills the recorded fake-CLI process group when dropped, so a failing test never leaves
    /// a `sleep` running. The fake CLI runs in its own group, so the group id is its pid.
    struct GroupGuard {
        pidfile: PathBuf,
    }

    impl Drop for GroupGuard {
        fn drop(&mut self) {
            let pid = std::fs::read_to_string(&self.pidfile)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok());
            if let Some(pid) = pid {
                // SAFETY: SIGKILL to a process group recorded by this test; ESRCH is harmless.
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
        }
    }

    /// Observer that ignores all progress and completion.
    struct NoopObserver;

    impl JobObserver for NoopObserver {
        fn on_progress(&self, _snap: &JobSnapshot) {}
        fn on_finish(&self, _snap: &JobSnapshot) {}
    }

    /// Writes an executable `sh` script named `name` into `dir` and returns its path.
    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake cli");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    /// Identity of a file, as the job launcher checks it.
    fn file_id(path: &Path) -> FileId {
        let meta = std::fs::metadata(path).expect("stat fake binary");
        FileId {
            dev: meta.dev(),
            ino: meta.ino(),
            len: meta.len(),
            mtime_secs: meta.mtime(),
        }
    }

    /// A launch for project "proj" that runs `binary` for at most `timeout`.
    fn launch(dir: &Path, binary: &Path, timeout: Duration) -> JobLaunch {
        let pinned: OwnedFd = std::fs::File::open(dir).expect("open project dir").into();
        JobLaunch {
            project: "proj".to_string(),
            dir: pinned,
            task: "summarize".to_string(),
            config: LaunchConfig {
                binary: binary.to_path_buf(),
                binary_id: file_id(binary),
                prompt_flag: "-p".to_string(),
                timeout,
            },
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

    /// Fast-timing manager so cancel finishes quickly in tests.
    fn quick_manager() -> Arc<JobManager> {
        JobManager::with_timing(Duration::from_millis(200), Duration::from_millis(200))
    }

    #[test]
    fn status_source_is_none_when_no_job_has_run() {
        let mgr = quick_manager();
        let source = ManagerStatusSource(mgr);
        assert!(source.status().is_none());
    }

    #[test]
    fn status_source_reports_the_running_job_snapshot_from_the_same_manager() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("leader.pid");
        let bin = write_script(
            dir.path(),
            "sleep-cli",
            &format!("echo $$ > '{}'; sleep 60", pidfile.display()),
        );
        let _guard = GroupGuard {
            pidfile: pidfile.clone(),
        };
        let mgr = quick_manager();
        let spec = JobSpec {
            launch: launch(dir.path(), &bin, Duration::from_secs(12)),
            progress_interval: Duration::from_secs(90),
        };
        let id = mgr
            .start(spec, Arc::new(NoopObserver))
            .expect("start must succeed for a valid fake cli");
        assert!(
            wait_until(Duration::from_secs(5), || pidfile.exists()),
            "fake cli must record its pid"
        );

        let source = ManagerStatusSource(mgr.clone());
        let snap = source
            .status()
            .expect("a running job must be reported by the status source");
        assert_eq!(
            snap.id, id,
            "status source must read the same manager's job"
        );
        assert_eq!(snap.project, "proj");
        assert_eq!(snap.state, JobState::Running);
        assert!(mgr.has_active_job());

        mgr.cancel();
    }
}

/// Source contracts: read the app's own files at test time and check the wiring shape.
/// These pass only once the S3d wiring replaces the markers and adds the exit paths.
mod source_contracts {
    use std::path::{Path, PathBuf};

    /// Returns the text before the first `#[cfg(test)]` line. Test modules sit at the end
    /// of every app file, so this is the non-test code.
    fn non_test_part(src: &str) -> &str {
        match src.find("#[cfg(test)]") {
            Some(i) => &src[..i],
            None => src,
        }
    }

    const LIB_RS: &str = include_str!("lib.rs");
    const COMMANDS_AGENT_JOBS_RS: &str = include_str!("commands/agent_jobs.rs");
    const CONVERSE_PLATFORM_RS: &str = include_str!("managers/converse_platform.rs");
    const TRAY_RS: &str = include_str!("tray.rs");

    /// Every non-test `.rs` file under src/ (test files are skipped by name).
    fn app_source_files() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.ends_with("_tests.rs") {
                        out.push(path);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    #[test]
    fn lib_declares_the_job_runtime_module() {
        assert!(
            LIB_RS.contains("mod job_runtime;"),
            "lib.rs must declare `mod job_runtime;`"
        );
    }

    #[test]
    fn no_s3d_stub_marker_remains_in_any_non_test_app_file() {
        for path in app_source_files() {
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                !non_test_part(&src).contains("S3D-STUB"),
                "{} still contains an S3D-STUB marker",
                path.display()
            );
        }
        assert!(
            !COMMANDS_AGENT_JOBS_RS.contains("S3D-STUB"),
            "commands/agent_jobs.rs still contains an S3D-STUB marker"
        );
    }

    #[test]
    fn no_wired_in_s3d_allow_mark_remains_in_any_non_test_app_file() {
        for path in app_source_files() {
            let src = std::fs::read_to_string(&path).unwrap_or_default();
            assert!(
                !non_test_part(&src).contains("wired in S3d"),
                "{} still contains a `wired in S3d` mark",
                path.display()
            );
        }
        assert!(
            !non_test_part(CONVERSE_PLATFORM_RS).contains("wired in S3d"),
            "converse_platform.rs still has the `wired in S3d` allow mark"
        );
    }

    #[test]
    fn lib_tray_quit_routes_through_request_exit_not_bare_exit_zero() {
        assert!(
            LIB_RS.contains("request_exit("),
            "the tray quit must route through ExitCoordinator::request_exit"
        );
        let bare = non_test_part(LIB_RS).matches("app.exit(0);").count();
        assert_eq!(
            bare, 0,
            "lib.rs must not call a bare `app.exit(0);` on the tray quit path"
        );
    }

    #[test]
    fn lib_handles_native_exit_requested_with_on_exit_requested() {
        assert!(
            LIB_RS.contains("RunEvent::ExitRequested"),
            "lib.rs must add a RunEvent::ExitRequested arm"
        );
        assert!(
            LIB_RS.contains("on_exit_requested("),
            "the ExitRequested arm must call ExitCoordinator::on_exit_requested"
        );
    }

    #[test]
    fn lib_run_event_exit_calls_manager_shutdown_as_a_belt() {
        assert!(
            LIB_RS.contains("shutdown(&ShutdownConfig"),
            "RunEvent::Exit must call JobManager::shutdown with ShutdownConfig budgets"
        );
    }

    #[test]
    fn exactly_one_job_manager_is_constructed_in_non_test_app_code() {
        let total: usize = app_source_files()
            .iter()
            .map(|path| {
                let src = std::fs::read_to_string(path).unwrap_or_default();
                non_test_part(&src).matches("JobManager::new(").count()
            })
            .sum();
        assert_eq!(
            total, 1,
            "exactly one JobManager::new( in non-test app code (one shared manager)"
        );
    }

    #[test]
    fn tray_cancel_job_click_arm_is_handled_in_lib() {
        assert!(
            LIB_RS.contains("\"cancel_job\""),
            "lib.rs tray click handler must handle the \"cancel_job\" arm"
        );
        assert!(
            non_test_part(TRAY_RS).contains("set_cancel_job_handler")
                || LIB_RS.contains("set_cancel_job_handler("),
            "the cancel handler must be registered via tray::set_cancel_job_handler"
        );
    }
}

/// S5 fix (minor 3): the exit fallback deadline must leave room for the SIGKILL that
/// follows the TERM grace, so a child cannot outlive an updater restart.
#[test]
fn exit_fallback_deadline_covers_term_grace_plus_margin() {
    let budgets = super::exit_fallback_budgets();
    assert!(
        budgets.deadline >= budgets.term_grace + super::EXIT_FALLBACK_MARGIN,
        "deadline {:?} must be at least term_grace {:?} + margin {:?}",
        budgets.deadline,
        budgets.term_grace,
        super::EXIT_FALLBACK_MARGIN
    );
    assert!(
        super::EXIT_FALLBACK_MARGIN >= Duration::from_millis(500),
        "margin must be at least 500 ms"
    );
}
