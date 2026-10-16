//! Unix-only process tests for `list_voices_via` (the bounded runner integration).
//! Covers DESIGN.md "Runner integration tests (unix-gated file, C1 checks)".
//! Fake programs are tiny `sh` scripts in a temp dir. Only std, tokio and
//! tempfile are used here; crate items are referenced by full path.
#![cfg(unix)]

use std::future::Future;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const CAP: usize = 64 * 1024;

/// Attempts for the pid-dependent tests. Under heavy load a script can be
/// slower to start than the runner timeout, so it may never write its pid.
const PID_ATTEMPTS: usize = 3;

/// Writes `#!/bin/sh` + `body` to an executable script named `fake-say.sh`.
fn write_script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("fake-say.sh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fake script");
    let mut perms = std::fs::metadata(&path).expect("stat script").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod script");
    path
}

fn names(voices: &[crate::managers::tts_voices::TtsVoice]) -> Vec<String> {
    voices.iter().map(|v| v.name.clone()).collect()
}

/// Waits up to 3 s (polling every 50 ms) for the fake script to write its pid.
/// Returns `None` when the file never appears with non-empty content, which
/// means the child never ran far enough to write it.
fn read_pid(path: &Path) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let pid = trimmed
                    .parse::<u32>()
                    .expect("pid file must hold a numeric pid");
                return Some(pid);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// True while `kill -0` succeeds. A reaped, exited process makes it fail.
fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

/// Polls up to 3 s for the pid to disappear.
fn wait_until_dead(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if !pid_alive(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs `attempt` up to `PID_ATTEMPTS` times, each with a fresh tempdir and
/// script (the closure creates them). `attempt` returns `true` when the child
/// started, its pid(s) were seen and verified dead. `false` means no pid file
/// appeared in that attempt, which proves nothing about cleanup, so the next
/// attempt runs. Always-true assertions (empty result, elapsed bound) live
/// inside the closure and run on every attempt.
async fn retry_until_child_started<F, Fut>(mut attempt: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..PID_ATTEMPTS {
        if attempt().await {
            return;
        }
    }
    panic!("child never started in {PID_ATTEMPTS} attempts; cannot verify cleanup");
}

/// If a test panics, kills the pids recorded in these pid files so no
/// `sleep 30` is left behind. Pids are never killed on the success path.
struct KillOnPanic(Vec<PathBuf>);

impl Drop for KillOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            for file in &self.0 {
                if let Ok(text) = std::fs::read_to_string(file) {
                    let _ = Command::new("kill").args(["-9", text.trim()]).status();
                }
            }
        }
    }
}

#[tokio::test]
async fn two_valid_lines_are_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "printf 'Albert en_US # Hi\\nAlice it_IT # Ciao\\n'",
    );
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    assert_eq!(names(&voices), vec!["Albert", "Alice"]);
}

#[tokio::test]
async fn closed_stdout_with_exit_zero_still_returns_voices() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "printf 'Albert en_US # Hi\\nAlice it_IT # Ciao\\n'; exec 1>&-; /bin/sleep 0.3; exit 0",
    );
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    assert_eq!(names(&voices), vec!["Albert", "Alice"]);
}

#[tokio::test]
async fn nonzero_exit_keeps_valid_lines_already_printed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "printf 'Albert en_US # Hi\\n'; exit 1");
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    assert_eq!(names(&voices), vec!["Albert"]);
}

#[tokio::test]
async fn nonzero_exit_without_output_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(dir.path(), "exit 1");
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    assert!(voices.is_empty(), "{voices:?}");
}

#[tokio::test]
async fn output_over_the_cap_is_empty_and_returns_promptly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "/usr/bin/yes 'Albert en_US # x' | /usr/bin/head -c 400000",
    );
    let start = Instant::now();
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    let elapsed = start.elapsed();
    assert!(voices.is_empty(), "truncated listing must be empty");
    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
}

#[tokio::test]
async fn timeout_returns_empty_and_kills_the_child() {
    retry_until_child_started(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("pid");
        let script = write_script(
            dir.path(),
            &format!("echo $$ > '{}'\nexec /bin/sleep 30", pidfile.display()),
        );
        let _guard = KillOnPanic(vec![pidfile.clone()]);
        let start = Instant::now();
        let voices =
            crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(4))
                .await;
        let elapsed = start.elapsed();
        assert!(voices.is_empty(), "{voices:?}");
        assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
        match read_pid(&pidfile) {
            Some(pid) => {
                assert!(wait_until_dead(pid), "child pid {pid} still alive");
                true
            }
            None => false,
        }
    })
    .await;
}

/// Closing stdout makes the runner take the end-of-output path immediately
/// (it does not wait for the timeout), so the empty result comes back
/// promptly while the child, still sleeping, must be killed.
#[tokio::test]
async fn closed_stdout_then_sleep_returns_promptly_and_kills_the_child() {
    retry_until_child_started(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("pid");
        let script = write_script(
            dir.path(),
            &format!(
                "echo $$ > '{}'\nexec 1>&-\nexec /bin/sleep 30",
                pidfile.display()
            ),
        );
        let _guard = KillOnPanic(vec![pidfile.clone()]);
        let start = Instant::now();
        let voices =
            crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(4))
                .await;
        let elapsed = start.elapsed();
        assert!(voices.is_empty(), "{voices:?}");
        assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
        match read_pid(&pidfile) {
            Some(pid) => {
                assert!(wait_until_dead(pid), "child pid {pid} still alive");
                true
            }
            None => false,
        }
    })
    .await;
}

#[tokio::test]
async fn shell_exit_with_background_descendant_returns_and_kills_the_group() {
    retry_until_child_started(|| async {
        let dir = tempfile::tempdir().expect("tempdir");
        let pidfile = dir.path().join("pid");
        let descfile = dir.path().join("desc-pid");
        let script = write_script(
            dir.path(),
            &format!(
                "/bin/sleep 30 &\necho $! > '{}'\necho $$ > '{}'\nexit 0",
                descfile.display(),
                pidfile.display()
            ),
        );
        let _guard = KillOnPanic(vec![pidfile.clone(), descfile.clone()]);
        let start = Instant::now();
        let voices =
            crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(4))
                .await;
        let elapsed = start.elapsed();
        assert!(voices.is_empty(), "{voices:?}");
        assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
        let Some(shell_pid) = read_pid(&pidfile) else {
            return false;
        };
        let Some(desc_pid) = read_pid(&descfile) else {
            return false;
        };
        assert!(wait_until_dead(shell_pid), "shell pid {shell_pid} alive");
        assert!(wait_until_dead(desc_pid), "descendant pid {desc_pid} alive");
        true
    })
    .await;
}

#[tokio::test]
async fn missing_program_path_is_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("missing-say");
    let voices =
        crate::managers::tts_voices::list_voices_via(&missing, &[], CAP, Duration::from_secs(1))
            .await;
    assert!(voices.is_empty(), "{voices:?}");
}

#[tokio::test]
async fn non_utf8_bytes_do_not_panic_and_yield_only_sanitized_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = write_script(
        dir.path(),
        "printf '\\377\\376Albert en_US # x\\n'; printf 'Alice it_IT # y\\n'",
    );
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(5))
            .await;
    // The invalid bytes decode to U+FFFD, so the "Albert" line is refused by
    // sanitize_voice; only the valid "Alice" line survives lossy decoding.
    assert_eq!(names(&voices), vec!["Alice"]);
    for v in &voices {
        assert_eq!(
            crate::managers::tts::sanitize_voice(&v.name).as_deref(),
            Some(v.name.as_str()),
            "{v:?}"
        );
    }
}
