//! Test-only helper (cfg(test)). Runs an ignored inner test in a CHILD copy of the
//! test binary so that environment variables needed by a test never touch the
//! parent test process, where they would race with unrelated parallel tests.
//! Included by bounded_cmd.rs and git_vet.rs with #[path] mod lines.

use std::process::Command;

/// Prefix of the line an inner test prints once all of its assertions passed.
const CHILD_OK_PREFIX: &str = "VOX_CHILD_OK";

/// Runs the ignored inner test `inner` (a function in the module whose
/// `module_path!()` is `module_path`) in a child process with `env` added on top of
/// the inherited environment. Fails unless the child exits successfully AND prints
/// its sentinel, so a child that ran zero tests cannot pass vacuously.
pub(crate) fn run_in_child(module_path: &str, inner: &str, env: &[(&str, &str)]) {
    // module_path!() starts with the crate name; libtest test names do not.
    let relative = module_path
        .split_once("::")
        .map_or(module_path, |(_, rest)| rest);
    let full_name = format!("{relative}::{inner}");
    let exe = std::env::current_exe().expect("current test binary");
    let mut cmd = Command::new(exe);
    cmd.args([
        "--exact",
        full_name.as_str(),
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("spawn child test binary");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "child test {full_name} failed ({:?})\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status
    );
    let sentinel = format!("{CHILD_OK_PREFIX} {inner}");
    assert!(
        stdout.contains(&sentinel),
        "child test {full_name} did not reach its sentinel (not run?)\n--- stdout ---\n{stdout}"
    );
}

/// Called by an inner test as its last statement, after every assertion passed.
pub(crate) fn child_ok(inner: &str) {
    println!("{CHILD_OK_PREFIX} {inner}");
}
