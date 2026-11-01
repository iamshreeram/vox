//! Unit tests for `source_update_runner`. Written before the implementation.
//! Wired from the runner module with `#[path]`; `use super::*` brings in the
//! module under test.

use super::*;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

const RUN: &str = "0123456789abcdef";
const FULL: &str = "0123456789abcdef0123456789abcdef01234567";
const NOW: u64 = 1_800_000_000;

fn status_json(phase: &str, pct: u32, msg: &str, ts: u64) -> String {
    format!(
        r#"{{"v":1,"run_id":"{RUN}","phase":"{phase}","percent":{pct},"target":"0123456","target_full":"{FULL}","message":"{msg}","method":"exact","ts":{ts}}}"#
    )
}

fn parse(text: &str, prev: Option<&RunStatus>) -> Result<RunStatus, StatusError> {
    parse_status(text.as_bytes(), RUN, prev, NOW)
}

fn prev_status(phase: RunPhase, pct: u8) -> RunStatus {
    RunStatus {
        run_id: RUN.to_string(),
        phase,
        percent: pct,
        target: "0123456".to_string(),
        target_full: FULL.to_string(),
        message: "previous".to_string(),
        method: StatusMethod::Exact,
        ts: NOW,
    }
}

// ---- RunPhase / progress types -------------------------------------------

#[test]
fn phase_terminal_only_for_done_and_failed() {
    assert!(RunPhase::Done.is_terminal());
    assert!(RunPhase::Failed.is_terminal());
    for phase in [
        RunPhase::Preparing,
        RunPhase::Dependencies,
        RunPhase::Compiling,
        RunPhase::Bundling,
        RunPhase::Installing,
        RunPhase::Restarting,
    ] {
        assert!(!phase.is_terminal(), "{phase:?} must not be terminal");
    }
}

#[test]
fn phase_serialises_lowercase() {
    assert_eq!(
        serde_json::to_string(&RunPhase::Dependencies)
            .ok()
            .as_deref(),
        Some("\"dependencies\"")
    );
    assert_eq!(
        serde_json::to_string(&StatusMethod::Estimated)
            .ok()
            .as_deref(),
        Some("\"estimated\"")
    );
}

#[test]
fn update_progress_from_status_maps_method_to_estimated() {
    let mut status = prev_status(RunPhase::Compiling, 42);
    status.method = StatusMethod::Estimated;
    let progress = UpdateProgress::from(&status);
    assert_eq!(progress.run_id, RUN);
    assert_eq!(progress.phase, RunPhase::Compiling);
    assert_eq!(progress.percent, 42);
    assert_eq!(progress.target, "0123456");
    assert_eq!(progress.message, "previous");
    assert!(progress.estimated);
    status.method = StatusMethod::Exact;
    assert!(!UpdateProgress::from(&status).estimated);
}

// ---- parse_status: accepted input ----------------------------------------

#[test]
fn parse_accepts_a_valid_compiling_status() {
    let status = parse(
        &status_json("compiling", 55, "Compiling 5/9 crates", NOW),
        None,
    );
    let status = match status {
        Ok(s) => s,
        Err(e) => panic!("valid status rejected: {e}"),
    };
    assert_eq!(status.phase, RunPhase::Compiling);
    assert_eq!(status.percent, 55);
    assert_eq!(status.target_full, FULL);
    assert_eq!(status.method, StatusMethod::Exact);
    assert_eq!(status.ts, NOW);
}

#[test]
fn parse_accepts_done_at_exactly_100() {
    assert!(parse(&status_json("done", 100, "ok", NOW), None).is_ok());
}

#[test]
fn parse_accepts_failed_with_any_percent() {
    assert!(parse(&status_json("failed", 40, "boom", NOW), None).is_ok());
}

#[test]
fn parse_accepts_empty_target_full_with_short_target() {
    let text = format!(
        r#"{{"v":1,"run_id":"{RUN}","phase":"preparing","percent":0,"target":"abcdef0","target_full":"","message":"","method":"estimated","ts":{NOW}}}"#
    );
    let status = parse(&text, None).expect("empty target_full is allowed");
    assert_eq!(status.target_full, "");
    assert_eq!(status.method, StatusMethod::Estimated);
}

#[test]
fn parse_accepts_message_of_exactly_200_chars() {
    let msg = "a".repeat(200);
    assert!(parse(&status_json("preparing", 1, &msg, NOW), None).is_ok());
}

#[test]
fn parse_accepts_ts_exactly_one_day_ahead() {
    assert!(parse(&status_json("preparing", 1, "x", NOW + 86_400), None).is_ok());
}

// ---- parse_status: rejected input ----------------------------------------

#[test]
fn parse_rejects_oversize_input_before_anything_else() {
    let big = vec![b' '; MAX_STATUS_BYTES + 1];
    assert_eq!(
        parse_status(&big, RUN, None, NOW),
        Err(StatusError::TooLarge)
    );
}

#[test]
fn parse_rejects_invalid_utf8() {
    let bytes = [0xff, 0xfe, 0x00];
    assert_eq!(
        parse_status(&bytes, RUN, None, NOW),
        Err(StatusError::InvalidUtf8)
    );
}

#[test]
fn parse_rejects_non_json() {
    assert_eq!(
        parse("not json at all", None),
        Err(StatusError::InvalidJson)
    );
}

#[test]
fn parse_rejects_json_that_is_not_an_object() {
    assert_eq!(parse("[1,2,3]", None), Err(StatusError::NotAnObject));
}

#[test]
fn parse_rejects_wrong_version() {
    let text = status_json("preparing", 1, "x", NOW).replace("\"v\":1", "\"v\":2");
    assert_eq!(parse(&text, None), Err(StatusError::BadVersion));
}

#[test]
fn parse_rejects_unknown_phase() {
    assert_eq!(
        parse(&status_json("bogus", 1, "x", NOW), None),
        Err(StatusError::UnknownPhase)
    );
}

#[test]
fn parse_rejects_wrong_run_id() {
    let text = status_json("preparing", 1, "x", NOW).replace(RUN, "fedcba9876543210");
    assert_eq!(parse(&text, None), Err(StatusError::BadRunId));
}

#[test]
fn parse_rejects_uppercase_run_id() {
    let text = status_json("preparing", 1, "x", NOW).replace(RUN, "0123456789ABCDEF");
    assert_eq!(parse(&text, None), Err(StatusError::BadRunId));
}

#[test]
fn parse_rejects_percent_above_100() {
    assert_eq!(
        parse(&status_json("compiling", 101, "x", NOW), None),
        Err(StatusError::PercentOutOfRange)
    );
}

#[test]
fn parse_rejects_non_terminal_phase_at_100() {
    assert_eq!(
        parse(&status_json("compiling", 100, "x", NOW), None),
        Err(StatusError::PercentNotAllowed)
    );
}

#[test]
fn parse_rejects_done_below_100() {
    assert_eq!(
        parse(&status_json("done", 99, "x", NOW), None),
        Err(StatusError::PercentNotAllowed)
    );
}

#[test]
fn parse_rejects_uppercase_target() {
    let text = status_json("preparing", 1, "x", NOW).replace("\"0123456\"", "\"ABCDEF0\"");
    assert_eq!(parse(&text, None), Err(StatusError::BadTarget));
}

#[test]
fn parse_rejects_target_that_is_not_the_prefix_of_target_full() {
    let text = status_json("preparing", 1, "x", NOW).replace("\"0123456\"", "\"abcdef0\"");
    assert_eq!(parse(&text, None), Err(StatusError::BadTarget));
}

#[test]
fn parse_rejects_short_target_full() {
    let text = status_json("preparing", 1, "x", NOW).replace(FULL, &FULL[..39]);
    assert_eq!(parse(&text, None), Err(StatusError::BadTargetFull));
}

#[test]
fn parse_rejects_uppercase_target_full() {
    let upper = FULL.to_uppercase();
    let text = status_json("preparing", 1, "x", NOW).replace(FULL, &upper);
    assert_eq!(parse(&text, None), Err(StatusError::BadTargetFull));
}

#[test]
fn parse_rejects_message_over_200_chars() {
    let msg = "a".repeat(201);
    assert_eq!(
        parse(&status_json("preparing", 1, &msg, NOW), None),
        Err(StatusError::MessageTooLong)
    );
}

#[test]
fn parse_rejects_control_character_in_message_instead_of_sanitising() {
    let text = status_json("preparing", 1, r"bad\u0007bell", NOW);
    assert_eq!(parse(&text, None), Err(StatusError::MessageControlChar));
    let newline = status_json("preparing", 1, r"line\nbreak", NOW);
    assert_eq!(parse(&newline, None), Err(StatusError::MessageControlChar));
}

#[test]
fn parse_rejects_timestamp_more_than_a_day_ahead() {
    assert_eq!(
        parse(&status_json("preparing", 1, "x", NOW + 86_401), None),
        Err(StatusError::FutureTimestamp)
    );
}

#[test]
fn parse_rejects_unknown_top_level_field() {
    let text = status_json("preparing", 1, "x", NOW).replace('}', r#","extra":1}"#);
    assert!(matches!(parse(&text, None), Err(StatusError::BadField(_))));
}

#[test]
fn parse_rejects_missing_field() {
    let text = format!(
        r#"{{"v":1,"run_id":"{RUN}","phase":"preparing","percent":0,"target":"0123456","target_full":"{FULL}","message":"x","ts":{NOW}}}"#
    );
    assert!(matches!(parse(&text, None), Err(StatusError::BadField(_))));
}

#[test]
fn parse_rejects_unknown_method() {
    let text = status_json("preparing", 1, "x", NOW).replace("\"exact\"", "\"guess\"");
    assert!(matches!(parse(&text, None), Err(StatusError::BadField(_))));
}

#[test]
fn parse_rejects_non_integer_percent() {
    let text = status_json("preparing", 1, "x", NOW).replace("\"percent\":1", "\"percent\":\"1\"");
    assert!(matches!(parse(&text, None), Err(StatusError::BadField(_))));
}

#[test]
fn status_error_display_never_echoes_raw_input() {
    let secret = "SECRET_VALUE_SHOULD_NOT_APPEAR";
    let text = status_json("preparing", 1, "x", NOW).replace(RUN, secret);
    let err = parse(&text, None).expect_err("must be rejected");
    let shown = format!("{err}");
    assert!(!shown.is_empty());
    assert!(!shown.contains("SECRET"));
    let unknown = status_json(secret, 1, "x", NOW);
    let err = parse(&unknown, None).expect_err("unknown phase rejected");
    assert!(!format!("{err}").contains("SECRET"));
}

// ---- parse_status: monotonic rules ---------------------------------------

#[test]
fn parse_rejects_percent_regression_within_a_run() {
    let prev = prev_status(RunPhase::Compiling, 60);
    assert_eq!(
        parse(&status_json("compiling", 50, "x", NOW), Some(&prev)),
        Err(StatusError::Regress)
    );
}

#[test]
fn parse_accepts_equal_percent_within_a_run() {
    let prev = prev_status(RunPhase::Compiling, 60);
    assert!(parse(&status_json("compiling", 60, "x", NOW), Some(&prev)).is_ok());
}

#[test]
fn parse_ignores_prev_from_another_run() {
    let mut prev = prev_status(RunPhase::Compiling, 90);
    prev.run_id = "fedcba9876543210".to_string();
    assert!(parse(&status_json("compiling", 10, "x", NOW), Some(&prev)).is_ok());
}

#[test]
fn parse_rejects_phase_going_backwards() {
    let prev = prev_status(RunPhase::Compiling, 30);
    assert_eq!(
        parse(&status_json("preparing", 31, "x", NOW), Some(&prev)),
        Err(StatusError::PhaseRegress)
    );
}

#[test]
fn parse_allows_failed_from_any_non_terminal_phase() {
    let prev = prev_status(RunPhase::Installing, 93);
    assert!(parse(&status_json("failed", 93, "x", NOW), Some(&prev)).is_ok());
}

#[test]
fn parse_rejects_leaving_a_terminal_phase() {
    let prev = prev_status(RunPhase::Done, 100);
    assert_eq!(
        parse(&status_json("failed", 100, "x", NOW), Some(&prev)),
        Err(StatusError::PhaseRegress)
    );
    let prev = prev_status(RunPhase::Failed, 40);
    assert_eq!(
        parse(&status_json("done", 100, "x", NOW), Some(&prev)),
        Err(StatusError::PhaseRegress)
    );
}

#[test]
fn parse_allows_forward_phase_move() {
    let prev = prev_status(RunPhase::Dependencies, 5);
    assert!(parse(&status_json("compiling", 10, "x", NOW), Some(&prev)).is_ok());
}

// ---- state directory ------------------------------------------------------

#[test]
fn update_state_dir_is_app_data_update() {
    let dir = update_state_dir(Path::new("/data/app"));
    assert_eq!(dir, Path::new("/data/app/update"));
}

#[test]
fn status_log_and_plist_paths_are_inside_the_state_dir() {
    let dir = Path::new("/data/app/update");
    assert_eq!(status_path(dir), dir.join("status.json"));
    assert_eq!(log_path(dir, RUN), dir.join(format!("run-{RUN}.log")));
    assert_eq!(plist_path(dir, RUN), dir.join(format!("run-{RUN}.plist")));
}

#[cfg(unix)]
#[test]
fn ensure_state_dir_creates_private_directory() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("update");
    ensure_state_dir(&dir).expect("create");
    let mode = fs::metadata(&dir).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    // Idempotent on an existing private directory.
    ensure_state_dir(&dir).expect("second call");
}

#[cfg(unix)]
#[test]
fn ensure_state_dir_refuses_symlink() {
    let root = tempfile::tempdir().expect("tempdir");
    let real = root.path().join("real");
    fs::create_dir(&real).expect("mkdir");
    let link = root.path().join("update");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    assert!(ensure_state_dir(&link).is_err());
}

#[cfg(unix)]
#[test]
fn ensure_state_dir_refuses_group_or_world_writable() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().expect("tempdir");
    let dir = root.path().join("update");
    fs::create_dir(&dir).expect("mkdir");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o770)).expect("chmod");
    assert!(ensure_state_dir(&dir).is_err());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o707)).expect("chmod");
    assert!(ensure_state_dir(&dir).is_err());
}

#[test]
fn ensure_state_dir_refuses_a_regular_file() {
    let root = tempfile::tempdir().expect("tempdir");
    let file = root.path().join("update");
    fs::write(&file, b"x").expect("write");
    assert!(ensure_state_dir(&file).is_err());
}

fn touch_at(path: &Path, secs_ago: u64) {
    fs::write(path, b"x").expect("write");
    let when = SystemTime::now() - Duration::from_secs(secs_ago);
    let file = fs::OpenOptions::new().write(true).open(path).expect("open");
    file.set_modified(when).expect("set mtime");
}

#[test]
fn rotate_logs_keeps_newest_three_runs_and_leaves_other_files() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = dir.path();
    let ids = [
        "1111111111111111",
        "2222222222222222",
        "3333333333333333",
        "4444444444444444",
        "5555555555555555",
    ];
    // Index 0 is the oldest, index 4 the newest.
    for (age, id) in ids.iter().enumerate() {
        let secs_ago = (ids.len() - age) as u64 * 100;
        touch_at(&base.join(format!("run-{id}.log")), secs_ago);
        touch_at(&base.join(format!("run-{id}.plist")), secs_ago);
    }
    touch_at(&base.join("status.json"), 10_000);
    touch_at(&base.join("notes.txt"), 10_000);
    touch_at(&base.join("run-XYZ.log"), 10_000);

    rotate_logs(base, 3).expect("rotate");

    for id in &ids[..2] {
        assert!(
            !base.join(format!("run-{id}.log")).exists(),
            "{id} log kept"
        );
        assert!(
            !base.join(format!("run-{id}.plist")).exists(),
            "{id} plist kept"
        );
    }
    for id in &ids[2..] {
        assert!(
            base.join(format!("run-{id}.log")).exists(),
            "{id} log removed"
        );
        assert!(
            base.join(format!("run-{id}.plist")).exists(),
            "{id} plist removed"
        );
    }
    assert!(base.join("status.json").exists());
    assert!(base.join("notes.txt").exists());
    assert!(base.join("run-XYZ.log").exists());
}

#[test]
fn rotate_logs_on_missing_dir_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(rotate_logs(&dir.path().join("absent"), 3).is_err());
}

// ---- run ids and labels ---------------------------------------------------

#[test]
fn new_run_id_is_16_lowercase_hex_and_varies() {
    let a = new_run_id();
    let b = new_run_id();
    assert!(is_valid_run_id(&a), "bad id {a}");
    assert!(is_valid_run_id(&b), "bad id {b}");
    assert_ne!(a, b);
}

#[test]
fn is_valid_run_id_accepts_only_16_lowercase_hex() {
    assert!(is_valid_run_id("0123456789abcdef"));
    assert!(!is_valid_run_id("0123456789ABCDEF"));
    assert!(!is_valid_run_id("0123456789abcde"));
    assert!(!is_valid_run_id("0123456789abcdef0"));
    assert!(!is_valid_run_id("0123456789abcdeg"));
    assert!(!is_valid_run_id(""));
    assert!(!is_valid_run_id("0123456789abcde\n"));
}

#[test]
fn job_label_has_the_fixed_prefix() {
    assert_eq!(
        job_label(RUN),
        "com.iamshreeram.vox.update.0123456789abcdef"
    );
}

// ---- job plist -------------------------------------------------------------

fn sample_params() -> JobParams {
    JobParams {
        run_id: RUN.to_string(),
        source_dir: PathBuf::from("/Users/dev/vox"),
        status_file: PathBuf::from("/Users/dev/Library/App/update/status.json"),
        state_dir: PathBuf::from("/Users/dev/Library/App/update"),
        log_file: PathBuf::from("/Users/dev/Library/App/update/run-0123456789abcdef.log"),
        target_full: FULL.to_string(),
        home: PathBuf::from("/Users/dev"),
        tmpdir: PathBuf::from("/var/folders/xy/T"),
        lock_dir: PathBuf::from("/var/folders/xy/T/vox-update.lock"),
        proxy_env: Vec::new(),
    }
}

/// Extracts the `<key>` names of one `<dict>` in order (top level only when
/// `depth_limit` is 1, the env dict is parsed separately).
fn keys_in(xml: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<key>") {
        let after = &rest[start + 5..];
        let end = after.find("</key>").expect("closing key");
        keys.push(after[..end].to_string());
        rest = &after[end..];
    }
    keys
}

fn env_section(xml: &str) -> &str {
    let start = xml
        .find("<key>EnvironmentVariables</key>")
        .expect("env key present");
    let tail = &xml[start..];
    let open = tail.find("<dict>").expect("env dict");
    let close = tail.find("</dict>").expect("env dict end");
    &tail[open..close]
}

const TOP_KEYS: [&str; 11] = [
    "Label",
    "ProgramArguments",
    "WorkingDirectory",
    "RunAtLoad",
    "ProcessType",
    "LowPriorityIO",
    "Nice",
    "AbandonProcessGroup",
    "StandardOutPath",
    "StandardErrorPath",
    "EnvironmentVariables",
];

const ENV_KEYS: [&str; 17] = [
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "PATH",
    "LANG",
    "GIT_TERMINAL_PROMPT",
    "GIT_SSH_COMMAND",
    "VOX_SOURCE_UPDATE",
    "VOX_UPDATE_RUN_ID",
    "VOX_UPDATE_STATUS_FILE",
    "VOX_UPDATE_STATE_DIR",
    "VOX_UPDATE_TARGET_SHA",
    "VOX_UPDATE_NONINTERACTIVE",
    "VOX_UPDATE_LOCK_DIR",
    "CARGO_TERM_PROGRESS_WHEN",
    "CARGO_TERM_PROGRESS_WIDTH",
];

#[test]
fn plist_has_exactly_the_expected_top_level_keys() {
    let xml = build_job_plist(&sample_params()).expect("valid params");
    assert_eq!(keys_in(&xml).len(), TOP_KEYS.len() + ENV_KEYS.len());
    let top: Vec<String> = keys_in(&xml).into_iter().take(TOP_KEYS.len()).collect();
    assert_eq!(top, TOP_KEYS);
}

#[test]
fn plist_environment_is_the_allow_list_only() {
    let xml = build_job_plist(&sample_params()).expect("valid params");
    let env = keys_in(env_section(&xml));
    assert_eq!(env, ENV_KEYS);
}

#[test]
fn plist_program_arguments_and_values_are_exact() {
    let xml = build_job_plist(&sample_params()).expect("valid params");
    assert!(xml.contains(
        "<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/bin/bash</string>\n\t\t<string>/Users/dev/vox/scripts/update-macos.sh</string>\n\t</array>"
    ), "{xml}");
    assert!(xml.contains("<key>WorkingDirectory</key>\n\t<string>/Users/dev/vox</string>"));
    assert!(xml.contains("<key>ProcessType</key>\n\t<string>Background</string>"));
    assert!(xml.contains("<key>Nice</key>\n\t<integer>10</integer>"));
    assert!(xml.contains("<key>RunAtLoad</key>\n\t<true/>"));
    assert!(xml.contains("<key>AbandonProcessGroup</key>\n\t<true/>"));
    assert!(xml.contains("<key>LowPriorityIO</key>\n\t<true/>"));
    assert!(xml.contains(
        "<key>StandardOutPath</key>\n\t<string>/Users/dev/Library/App/update/run-0123456789abcdef.log</string>"
    ));
    assert!(xml.contains(
        "<key>StandardErrorPath</key>\n\t<string>/Users/dev/Library/App/update/run-0123456789abcdef.log</string>"
    ));
    assert!(xml.contains(
        "<key>Label</key>\n\t<string>com.iamshreeram.vox.update.0123456789abcdef</string>"
    ));
}

#[test]
fn plist_environment_values_are_exact() {
    let xml = build_job_plist(&sample_params()).expect("valid params");
    let env = env_section(&xml);
    let expect = [
        ("HOME", "/Users/dev"),
        ("TMPDIR", "/var/folders/xy/T"),
        (
            "PATH",
            "/Users/dev/.cargo/bin:/Users/dev/.local/bin:/Users/dev/.bun/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        ),
        ("LANG", "en_US.UTF-8"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
        ("VOX_SOURCE_UPDATE", "1"),
        ("VOX_UPDATE_RUN_ID", RUN),
        ("VOX_UPDATE_TARGET_SHA", FULL),
        ("VOX_UPDATE_NONINTERACTIVE", "1"),
        ("VOX_UPDATE_LOCK_DIR", "/var/folders/xy/T/vox-update.lock"),
        ("CARGO_TERM_PROGRESS_WHEN", "always"),
        ("CARGO_TERM_PROGRESS_WIDTH", "100"),
    ];
    for (key, value) in expect {
        let needle = format!("<key>{key}</key>\n\t\t<string>{value}</string>");
        assert!(env.contains(&needle), "missing {key}={value}");
    }
}

#[test]
fn plist_forwards_only_allowed_proxy_variables() {
    let mut params = sample_params();
    params.proxy_env = vec![("HTTPS_PROXY".into(), "http://proxy.example:8080".into())];
    let xml = build_job_plist(&params).expect("valid proxy");
    let env = keys_in(env_section(&xml));
    assert_eq!(env.len(), ENV_KEYS.len() + 1);
    assert!(env.contains(&"HTTPS_PROXY".to_string()));

    let mut bad = sample_params();
    bad.proxy_env = vec![("AWS_SECRET_ACCESS_KEY".into(), "x".into())];
    assert!(build_job_plist(&bad).is_err());
}

#[test]
fn plist_xml_escapes_hostile_proxy_value() {
    let mut params = sample_params();
    params.proxy_env = vec![(
        "NO_PROXY".into(),
        "a</string><key>Evil</key><string>b&c".into(),
    )];
    let xml = build_job_plist(&params).expect("printable value accepted");
    assert!(xml.contains("a&lt;/string&gt;&lt;key&gt;Evil&lt;/key&gt;&lt;string&gt;b&amp;c"));
    assert_eq!(keys_in(&xml).len(), TOP_KEYS.len() + ENV_KEYS.len() + 1);
    assert!(!keys_in(&xml).contains(&"Evil".to_string()));
}

#[test]
fn plist_escapes_ampersand_in_paths() {
    let mut params = sample_params();
    params.home = PathBuf::from("/Users/a&b");
    params.source_dir = PathBuf::from("/Users/a&b/vox");
    let xml = build_job_plist(&params).expect("valid path with ampersand");
    assert!(xml.contains("/Users/a&amp;b/vox/scripts/update-macos.sh"));
    assert!(!xml.contains("a&b"));
}

#[test]
fn plist_rejects_newline_and_nul_and_control_in_every_path_field() {
    type Mutate = fn(&mut JobParams);
    let cases: [(&str, Mutate); 9] = [
        ("source_dir", |p| {
            p.source_dir = PathBuf::from("/Users/dev/vox\n")
        }),
        ("status_file", |p| p.status_file = PathBuf::from("/x\ny")),
        ("state_dir", |p| p.state_dir = PathBuf::from("/x\0y")),
        ("log_file", |p| p.log_file = PathBuf::from("/x\ty")),
        ("home", |p| p.home = PathBuf::from("/Users/\u{7}dev")),
        ("tmpdir", |p| p.tmpdir = PathBuf::from("/tmp\r")),
        ("lock_dir", |p| {
            p.lock_dir = PathBuf::from("/tmp/\u{7f}lock")
        }),
        ("run_id", |p| p.run_id = "0123456789abcde\n".into()),
        ("target_full", |p| p.target_full = format!("{FULL}\n")),
    ];
    for (name, mutate) in cases {
        let mut params = sample_params();
        mutate(&mut params);
        assert!(build_job_plist(&params).is_err(), "{name} accepted");
    }
}

#[test]
fn plist_rejects_relative_paths() {
    type Mutate = fn(&mut JobParams);
    let cases: [(&str, Mutate); 6] = [
        ("source_dir", |p| p.source_dir = PathBuf::from("vox")),
        ("status_file", |p| {
            p.status_file = PathBuf::from("status.json")
        }),
        ("state_dir", |p| p.state_dir = PathBuf::from("update")),
        ("log_file", |p| p.log_file = PathBuf::from("run.log")),
        ("home", |p| p.home = PathBuf::from("dev")),
        ("lock_dir", |p| p.lock_dir = PathBuf::from("lock")),
    ];
    for (name, mutate) in cases {
        let mut params = sample_params();
        mutate(&mut params);
        assert!(
            build_job_plist(&params).is_err(),
            "{name} relative accepted"
        );
    }
}

#[test]
fn plist_rejects_bad_run_id_and_target() {
    let mut params = sample_params();
    params.run_id = "../../etc".into();
    assert!(build_job_plist(&params).is_err());

    let mut params = sample_params();
    params.target_full = FULL.to_uppercase();
    assert!(build_job_plist(&params).is_err());

    let mut params = sample_params();
    params.target_full = "abc".into();
    assert!(build_job_plist(&params).is_err());
}

#[test]
fn plist_rejects_control_characters_in_proxy_value() {
    let mut params = sample_params();
    params.proxy_env = vec![("HTTP_PROXY".into(), "http://x\n<key>Y</key>".into())];
    assert!(build_job_plist(&params).is_err());
}

#[test]
fn plist_rejects_duplicate_proxy_names() {
    let mut params = sample_params();
    params.proxy_env = vec![
        ("HTTP_PROXY".into(), "a".into()),
        ("HTTP_PROXY".into(), "b".into()),
    ];
    assert!(build_job_plist(&params).is_err());
}

#[test]
fn plist_does_not_contain_any_unlisted_key_even_when_valid() {
    // Every <key> in the output must be one of the expected names.
    let xml = build_job_plist(&sample_params()).expect("valid");
    for key in keys_in(&xml) {
        assert!(
            TOP_KEYS.contains(&key.as_str()) || ENV_KEYS.contains(&key.as_str()),
            "unexpected key {key}"
        );
    }
}

#[test]
fn write_plist_file_is_private_and_atomic_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("run-{RUN}.plist"));
    write_plist_file(&path, "<plist/>").expect("write");
    assert_eq!(fs::read_to_string(&path).expect("read"), "<plist/>");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn validate_source_dir_accepts_checkout_with_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("scripts")).expect("mkdir");
    fs::write(dir.path().join("scripts/update-macos.sh"), b"#!/bin/bash\n").expect("write");
    assert!(validate_source_dir(dir.path()).is_ok());
}

#[test]
fn validate_source_dir_refuses_missing_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(matches!(
        validate_source_dir(dir.path()),
        Err(RunnerError::ScriptMissing)
    ));
}

#[test]
fn validate_source_dir_refuses_relative_and_control_paths() {
    assert!(validate_source_dir(Path::new("vox")).is_err());
    assert!(validate_source_dir(Path::new("/tmp/bad\npath")).is_err());
}

#[cfg(unix)]
#[test]
fn validate_source_dir_refuses_symlinked_script() {
    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().join("real.sh");
    fs::write(&real, b"#!/bin/bash\n").expect("write");
    fs::create_dir_all(dir.path().join("scripts")).expect("mkdir");
    std::os::unix::fs::symlink(&real, dir.path().join("scripts/update-macos.sh")).expect("ln");
    assert!(matches!(
        validate_source_dir(dir.path()),
        Err(RunnerError::ScriptMissing)
    ));
}

// ---- launchctl ------------------------------------------------------------

#[test]
fn parse_launchctl_print_running_with_pid() {
    let text = "gui/501/com.iamshreeram.vox.update.0123456789abcdef = {\n\tactive count = 1\n\tpath = /Users/dev/Library/LaunchAgents/x.plist\n\tstate = running\n\n\tprogram = /bin/bash\n\tpid = 1234\n\tlast exit code = 0\n}\n";
    assert_eq!(
        parse_launchctl_print(text),
        JobProbe::Registered {
            pid: Some(1234),
            last_exit: Some(0)
        }
    );
    assert!(parse_launchctl_print(text).is_live());
}

#[test]
fn parse_launchctl_print_not_running_has_no_pid() {
    let text = "\tstate = not running\n\tlast exit code = 0\n";
    let state = parse_launchctl_print(text);
    assert_eq!(
        state,
        JobProbe::Registered {
            pid: None,
            last_exit: Some(0)
        }
    );
    assert!(!state.is_live());
}

#[test]
fn parse_launchctl_print_never_exited_and_negative_exit() {
    let never = "\tstate = waiting\n\tlast exit code = (never exited)\n";
    assert_eq!(
        parse_launchctl_print(never),
        JobProbe::Registered {
            pid: None,
            last_exit: None
        }
    );
    let killed = "\tstate = not running\n\tlast exit code = -9\n";
    assert_eq!(
        parse_launchctl_print(killed),
        JobProbe::Registered {
            pid: None,
            last_exit: Some(-9)
        }
    );
}

#[test]
fn parse_launchctl_print_unrecognised_output_is_unknown() {
    assert_eq!(parse_launchctl_print("hello world"), JobProbe::Unknown);
    assert_eq!(parse_launchctl_print(""), JobProbe::Unknown);
}

#[test]
fn launchctl_argv_is_fixed_and_scoped_to_gui_uid() {
    assert_eq!(
        print_args(501, RUN_LABEL),
        vec!["print".to_string(), format!("gui/501/{RUN_LABEL}")]
    );
    assert_eq!(
        bootout_args(501, RUN_LABEL),
        vec!["bootout".to_string(), format!("gui/501/{RUN_LABEL}")]
    );
    let plist = Path::new("/Users/dev/Library/App/update/run-0123456789abcdef.plist");
    assert_eq!(
        bootstrap_args(501, plist).expect("absolute plist"),
        vec![
            "bootstrap".to_string(),
            "gui/501".to_string(),
            plist.display().to_string()
        ]
    );
    assert!(bootstrap_args(501, Path::new("relative.plist")).is_err());
}

const RUN_LABEL: &str = "com.iamshreeram.vox.update.0123456789abcdef";

#[test]
fn validate_job_label_accepts_only_the_update_prefix_and_run_id() {
    assert!(validate_job_label(RUN_LABEL).is_ok());
    assert!(validate_job_label("com.apple.something").is_err());
    assert!(validate_job_label("com.iamshreeram.vox.update.0123456789ABCDEF").is_err());
    assert!(validate_job_label("com.iamshreeram.vox.update.0123456789abcdef.extra").is_err());
    assert!(validate_job_label("com.iamshreeram.vox.agent").is_err());
}

#[test]
fn launchctl_state_refuses_foreign_labels_without_running() {
    let control = LaunchctlJobControl::current();
    assert_eq!(control.probe("com.apple.Finder"), JobProbe::Unknown);
}

/// In-memory job control used by the reconciliation and cleanup tests.
pub(crate) struct FakeJobControl {
    pub state: std::sync::Mutex<JobProbe>,
    pub bootouts: std::sync::Mutex<Vec<String>>,
}

impl FakeJobControl {
    pub fn with_state(state: JobProbe) -> Self {
        Self {
            state: std::sync::Mutex::new(state),
            bootouts: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl JobControl for FakeJobControl {
    fn probe(&self, _label: &str) -> JobProbe {
        match self.state.lock() {
            Ok(guard) => *guard,
            Err(_) => JobProbe::Absent,
        }
    }

    fn bootstrap(&self, _plist: &Path) -> Result<(), RunnerError> {
        if let Ok(mut guard) = self.state.lock() {
            *guard = JobProbe::Registered {
                pid: None,
                last_exit: None,
            };
        }
        Ok(())
    }

    fn bootout(&self, label: &str) -> Result<(), RunnerError> {
        if let Ok(mut log) = self.bootouts.lock() {
            log.push(label.to_string());
        }
        if let Ok(mut guard) = self.state.lock() {
            *guard = JobProbe::Absent;
        }
        Ok(())
    }
}

#[test]
fn fake_job_control_reflects_bootstrap_and_bootout() {
    let fake = FakeJobControl::with_state(JobProbe::Absent);
    assert_eq!(fake.probe(RUN_LABEL), JobProbe::Absent);
    fake.bootstrap(Path::new("/tmp/x.plist"))
        .expect("bootstrap");
    assert!(matches!(fake.probe(RUN_LABEL), JobProbe::Registered { .. }));
    fake.bootout(RUN_LABEL).expect("bootout");
    assert_eq!(fake.probe(RUN_LABEL), JobProbe::Absent);
}

// ---- lock_state -----------------------------------------------------------

#[test]
fn lock_state_free_when_directory_absent() {
    let root = tempfile::tempdir().expect("tempdir");
    let lock = root.path().join("vox-update.lock");
    assert_eq!(lock_state(&lock, &|_| true), LockState::Free);
}

#[test]
fn lock_state_reads_holder_pid_from_owner_token() {
    let root = tempfile::tempdir().expect("tempdir");
    let lock = root.path().join("vox-update.lock");
    fs::create_dir(&lock).expect("mkdir");
    fs::write(lock.join("owner"), "4242-1800000000-1234567\n").expect("owner");
    assert_eq!(lock_state(&lock, &|pid| pid == 4242), LockState::HeldAlive);
    assert_eq!(lock_state(&lock, &|_| false), LockState::HeldDead);
    assert!(lock.is_dir(), "lock must never be removed");
}

#[test]
fn lock_state_without_readable_owner_is_held_alive() {
    let root = tempfile::tempdir().expect("tempdir");
    let lock = root.path().join("vox-update.lock");
    fs::create_dir(&lock).expect("mkdir");
    assert_eq!(lock_state(&lock, &|_| false), LockState::HeldAlive);
    fs::write(lock.join("owner"), "not-a-token\n").expect("owner");
    assert_eq!(lock_state(&lock, &|_| false), LockState::HeldAlive);
    fs::write(lock.join("owner"), "0-1-2\n").expect("owner");
    assert_eq!(lock_state(&lock, &|_| false), LockState::HeldAlive);
    assert!(lock.is_dir());
}

#[test]
fn process_alive_sees_self_and_not_an_impossible_pid() {
    assert!(process_alive(std::process::id()));
    assert!(!process_alive(2_000_000_000));
    assert!(!process_alive(0));
}

// ---- reconcile (R3-1 decision table) -------------------------------------

const SHA: &str = FULL;
const OTHER_SHA: &str = "fedcba9876543210fedcba9876543210fedcba98";

fn input(
    status: Option<RunStatus>,
    age: u64,
    job: JobProbe,
    lock: LockState,
    published: bool,
) -> ReconcileInput<'static> {
    ReconcileInput {
        status,
        status_age_secs: age,
        job,
        lock,
        running_build_sha: SHA,
        published,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
        pending_built: false,
    }
}

fn registered(pid: Option<u32>, last_exit: Option<i32>) -> JobProbe {
    JobProbe::Registered { pid, last_exit }
}

fn done_status() -> RunStatus {
    prev_status(RunPhase::Done, 100)
}

fn failed_status(message: &str) -> RunStatus {
    let mut status = prev_status(RunPhase::Failed, 40);
    status.message = message.to_string();
    status
}

fn compiling_status() -> RunStatus {
    prev_status(RunPhase::Compiling, 50)
}

#[test]
fn reconcile_live_job_is_always_adopted() {
    let live = registered(Some(77), None);
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            live,
            LockState::HeldAlive,
            false
        )),
        Decision::Adopt
    );
    assert_eq!(
        reconcile(&input(
            Some(failed_status("x")),
            0,
            live,
            LockState::Free,
            true
        )),
        Decision::Adopt
    );
    assert_eq!(
        reconcile(&input(None, 0, live, LockState::Free, false)),
        Decision::Adopt
    );
    assert_eq!(
        reconcile(&input(
            Some(compiling_status()),
            9999,
            live,
            LockState::HeldDead,
            false
        )),
        Decision::Adopt
    );
}

#[test]
fn reconcile_done_matching_target_publishes_done() {
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            JobProbe::Absent,
            LockState::Free,
            false
        )),
        Decision::PublishDone
    );
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            JobProbe::Absent,
            LockState::HeldDead,
            false
        )),
        Decision::PublishDone
    );
}

#[test]
fn reconcile_done_with_mismatched_running_build_publishes_failure() {
    let mut status = done_status();
    status.target_full = OTHER_SHA.to_string();
    match reconcile(&input(
        Some(status),
        0,
        JobProbe::Absent,
        LockState::Free,
        false,
    )) {
        Decision::PublishFailed(text) => {
            assert!(text.contains("does not match"), "{text}");
        }
        other => panic!("expected PublishFailed, got {other:?}"),
    }
}

#[test]
fn reconcile_done_mismatch_with_empty_running_sha_publishes_done() {
    let mut status = done_status();
    status.target_full = OTHER_SHA.to_string();
    let mut inp = input(Some(status), 0, JobProbe::Absent, LockState::Free, false);
    inp.running_build_sha = "";
    assert_eq!(reconcile(&inp), Decision::PublishDone);
}

#[test]
fn reconcile_failed_unpublished_publishes_message() {
    assert_eq!(
        reconcile(&input(
            Some(failed_status("compile error")),
            0,
            JobProbe::Absent,
            LockState::Free,
            false
        )),
        Decision::PublishFailed("compile error".to_string())
    );
}

#[test]
fn reconcile_truncates_long_failure_message_to_120_chars() {
    let long = "x".repeat(200);
    match reconcile(&input(
        Some(failed_status(&long)),
        0,
        JobProbe::Absent,
        LockState::Free,
        false,
    )) {
        Decision::PublishFailed(text) => assert!(text.chars().count() <= 120),
        other => panic!("expected PublishFailed, got {other:?}"),
    }
}

#[test]
fn reconcile_published_terminal_with_no_process_is_cleaned_up() {
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            JobProbe::Absent,
            LockState::Free,
            true
        )),
        Decision::CleanupFinished
    );
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            registered(None, Some(0)),
            LockState::HeldDead,
            true
        )),
        Decision::CleanupFinished
    );
    assert_eq!(
        reconcile(&input(
            Some(failed_status("x")),
            0,
            JobProbe::Absent,
            LockState::Free,
            true
        )),
        Decision::CleanupFinished
    );
}

#[test]
fn reconcile_never_cleans_up_while_lock_is_held_alive() {
    assert_eq!(
        reconcile(&input(
            Some(done_status()),
            0,
            registered(None, Some(0)),
            LockState::HeldAlive,
            true
        )),
        Decision::None
    );
}

#[test]
fn reconcile_compiling_recent_without_job_waits() {
    assert_eq!(
        reconcile(&input(
            Some(compiling_status()),
            10,
            JobProbe::Absent,
            LockState::Free,
            false
        )),
        Decision::None
    );
}

#[test]
fn reconcile_compiling_old_without_job_is_interrupted() {
    match reconcile(&input(
        Some(compiling_status()),
        INTERRUPTED_AFTER_SECS,
        JobProbe::Absent,
        LockState::Free,
        false,
    )) {
        Decision::MarkInterrupted(text) => assert!(text.contains("interrupted")),
        other => panic!("expected MarkInterrupted, got {other:?}"),
    }
}

#[test]
fn reconcile_interrupted_names_last_exit_code_when_known() {
    match reconcile(&input(
        Some(compiling_status()),
        5000,
        registered(None, Some(-9)),
        LockState::HeldDead,
        false,
    )) {
        Decision::MarkInterrupted(text) => assert!(text.contains("-9"), "{text}"),
        other => panic!("expected MarkInterrupted, got {other:?}"),
    }
}

#[test]
fn reconcile_old_status_with_dead_lock_is_interrupted_and_lock_untouched() {
    assert!(matches!(
        reconcile(&input(
            Some(compiling_status()),
            5000,
            JobProbe::Absent,
            LockState::HeldDead,
            false
        )),
        Decision::MarkInterrupted(_)
    ));
}

#[test]
fn reconcile_no_status_and_registration_without_pid_is_cleaned_only_when_old() {
    assert_eq!(
        reconcile(&input(
            None,
            0,
            registered(None, None),
            LockState::Free,
            false
        )),
        Decision::None,
        "a job still being registered must not be declared dead"
    );
    // Old and silent: the stop is published first (A5 fix 2), cleanup follows.
    assert_eq!(
        reconcile(&input(
            None,
            INTERRUPTED_AFTER_SECS,
            registered(None, None),
            LockState::Free,
            false
        )),
        Decision::PublishFailed("The update stopped unexpectedly.".to_string())
    );
    assert_eq!(
        reconcile(&input(
            None,
            INTERRUPTED_AFTER_SECS,
            registered(None, None),
            LockState::Free,
            true
        )),
        Decision::CleanupFinished
    );
}

#[test]
fn reconcile_nothing_known_is_none() {
    assert_eq!(
        reconcile(&input(None, 0, JobProbe::Absent, LockState::Free, false)),
        Decision::None
    );
    assert_eq!(
        reconcile(&input(
            None,
            99_999,
            JobProbe::Absent,
            LockState::HeldDead,
            false
        )),
        Decision::None
    );
}

#[test]
fn reconcile_no_decision_removes_a_lock() {
    // Exhaustive on purpose: adding a variant forces a review of this rule.
    fn audit(decision: Decision) {
        match decision {
            Decision::None
            | Decision::Adopt
            | Decision::PublishDone
            | Decision::PublishFailed(_)
            | Decision::MarkInterrupted(_)
            | Decision::CleanupFinished
            | Decision::Rest => {}
        }
    }
    audit(reconcile(&input(
        Some(done_status()),
        0,
        JobProbe::Absent,
        LockState::HeldDead,
        true,
    )));
    audit(reconcile(&input(
        Some(compiling_status()),
        9999,
        JobProbe::Absent,
        LockState::HeldDead,
        false,
    )));
}

#[test]
fn reconcile_a_job_that_exited_without_a_status_publishes_the_stop_before_cleanup() {
    let exited = registered(None, Some(1));
    assert_eq!(
        reconcile(&input(None, 0, exited, LockState::Free, false)),
        Decision::PublishFailed("The update stopped unexpectedly (exit 1).".to_string()),
        "the exit code is known, so no waiting for the threshold"
    );
    assert_eq!(
        reconcile(&input(None, 0, exited, LockState::Free, true)),
        Decision::CleanupFinished
    );
    assert_eq!(
        reconcile(&input(None, 0, exited, LockState::HeldAlive, false)),
        Decision::None,
        "a live lock keeps everything in place"
    );
}

#[test]
fn reconcile_silent_exit_without_a_status_waits_for_the_threshold() {
    let silent = registered(None, None);
    assert_eq!(
        reconcile(&input(None, 0, silent, LockState::Free, false)),
        Decision::None,
        "a fresh plist must not clean up before the job had time to start"
    );
    assert_eq!(
        reconcile(&input(
            None,
            INTERRUPTED_AFTER_SECS - 1,
            silent,
            LockState::Free,
            false
        )),
        Decision::None
    );
    assert_eq!(
        reconcile(&input(
            None,
            INTERRUPTED_AFTER_SECS,
            silent,
            LockState::Free,
            false
        )),
        Decision::PublishFailed("The update stopped unexpectedly.".to_string())
    );
    assert_eq!(
        reconcile(&input(
            None,
            INTERRUPTED_AFTER_SECS,
            silent,
            LockState::Free,
            true
        )),
        Decision::CleanupFinished
    );
}

// B6 minor 7: launchctl calls are bounded. A hanging call is killed and reported as an error,
// and a timed-out probe is Unknown, never Absent or Registered.
#[test]
fn b6_a_hanging_launchctl_call_is_bounded_and_killed() {
    let started = Instant::now();
    let result = run_bounded(
        std::ffi::OsStr::new("/bin/sleep"),
        &["5".to_string()],
        Duration::from_millis(200),
    );
    let error = result.err().expect("a call past its limit is an error");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the wait is bounded, not the child's own 5 s"
    );
}

#[test]
fn b6_a_quick_launchctl_call_returns_its_output() {
    let output = run_bounded(
        std::ffi::OsStr::new("/bin/echo"),
        &["hi".to_string()],
        Duration::from_secs(5),
    )
    .expect("quick call");
    assert!(output.status.success());
    assert_eq!(output.stdout, b"hi\n".to_vec());
}

#[test]
fn b6_a_timed_out_probe_is_unknown() {
    let timed_out = Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "launchctl timed out",
    ));
    assert_eq!(probe_from_run(timed_out), JobProbe::Unknown);
}
