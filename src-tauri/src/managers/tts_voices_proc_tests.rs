//! Unix-only process tests for `list_voices_via` (the bounded runner integration).
//! Covers DESIGN.md "Runner integration tests (unix-gated file, C1 checks)".
//! Fake programs are tiny `sh` scripts in a temp dir. Only std, tokio and
//! tempfile are used here; crate items are referenced by full path.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const CAP: usize = 64 * 1024;

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

fn read_pid(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("pid file must exist")
        .trim()
        .to_string()
}

/// True while `kill -0` succeeds. A reaped, exited process makes it fail.
fn pid_alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .status()
        .is_ok_and(|s| s.success())
}

/// Polls up to 2 s for the pid to disappear.
fn wait_until_dead(pid: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
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
    let dir = tempfile::tempdir().expect("tempdir");
    let pidfile = dir.path().join("pid");
    let script = write_script(
        dir.path(),
        &format!("echo $$ > '{}'\nexec /bin/sleep 30", pidfile.display()),
    );
    let _guard = KillOnPanic(vec![pidfile.clone()]);
    let start = Instant::now();
    let voices =
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(1))
            .await;
    let elapsed = start.elapsed();
    assert!(voices.is_empty(), "{voices:?}");
    assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
    let pid = read_pid(&pidfile);
    assert!(wait_until_dead(&pid), "child pid {pid} still alive");
}

#[tokio::test]
async fn closed_stdout_then_sleep_times_out_and_kills_the_child() {
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
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(1))
            .await;
    let elapsed = start.elapsed();
    assert!(voices.is_empty(), "{voices:?}");
    assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
    let pid = read_pid(&pidfile);
    assert!(wait_until_dead(&pid), "child pid {pid} still alive");
}

#[tokio::test]
async fn shell_exit_with_background_descendant_returns_and_kills_the_group() {
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
        crate::managers::tts_voices::list_voices_via(&script, &[], CAP, Duration::from_secs(1))
            .await;
    let elapsed = start.elapsed();
    assert!(voices.is_empty(), "{voices:?}");
    assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
    let shell_pid = read_pid(&pidfile);
    let desc_pid = read_pid(&descfile);
    assert!(wait_until_dead(&shell_pid), "shell pid {shell_pid} alive");
    assert!(
        wait_until_dead(&desc_pid),
        "descendant pid {desc_pid} alive"
    );
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
    for v in &voices {
        assert_eq!(
            crate::managers::tts::sanitize_voice(&v.name).as_deref(),
            Some(v.name.as_str()),
            "{v:?}"
        );
    }
}
