//! Git vetting tests (DESIGN R2-1, R3-1). Red by design: `vet_git_config`,
//! `build_git_command` and `git_status_summary` are todo!().
//! Included from git_vet.rs. Do not edit to make code pass.
//!
//! Pure grammar and layout tests run everywhere. Tests that need a real git
//! binary early-return with an eprintln when no trusted git is installed; the
//! dedicated `git_missing_*` test covers the "git is not installed" path.
#![cfg(unix)]

use super::{build_git_command, git_status_summary, vet_git_config, VetError};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

const TRUSTED_GIT: [&str; 5] = [
    "/usr/bin/git",
    "/opt/homebrew/bin/git",
    "/usr/local/bin/git",
    "/run/current-system/sw/bin/git",
    "/nix/var/nix/profiles/default/bin/git",
];

const FIXED_PATH: &str = "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin";

/// Serializes tests that mutate the process environment.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// First trusted absolute git that exists as an executable regular file (R3-1 rule).
fn trusted_git() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    TRUSTED_GIT.iter().map(PathBuf::from).find(|p| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    })
}

/// Returns the trusted git path or prints a skip note and returns None.
fn require_git(test: &str) -> Option<PathBuf> {
    let git = trusted_git();
    if git.is_none() {
        eprintln!("skipping {test}: no trusted git binary installed");
    }
    git
}

/// Runs git for fixture construction with a cleared env and a fixed PATH.
fn fixture_git(cwd: &Path, args: &[&str]) {
    let git = trusted_git().expect("fixture requires a trusted git binary");
    let out = Command::new(git)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", FIXED_PATH)
        .env("HOME", cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("spawn fixture git");
    assert!(
        out.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit_all(cwd: &Path, subject: &str) {
    fixture_git(cwd, &["add", "-A"]);
    fixture_git(
        cwd,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            subject,
        ],
    );
}

/// A real repo with one commit whose subject is `subject`.
fn new_repo_with_commit(subject: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fixture_git(dir.path(), &["-c", "init.defaultBranch=main", "init", "-q"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").expect("write a.txt");
    commit_all(dir.path(), subject);
    dir
}

/// Bare fixture: a `.git` directory with the given config bytes, no git binary needed.
fn project_with_config(config: &[u8]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
    std::fs::write(dir.path().join(".git/config"), config).expect("write config");
    dir
}

fn forbidden_key(err: Result<(), VetError>) -> Option<String> {
    match err {
        Err(VetError::ForbiddenSetting(name)) => Some(name.to_ascii_lowercase()),
        _ => None,
    }
}

fn env_map(cmd: &Command) -> BTreeMap<String, String> {
    cmd.get_envs()
        .filter_map(|(k, v): (&OsStr, Option<&OsStr>)| {
            v.map(|v| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
        })
        .collect()
}

fn args_of(cmd: &Command) -> Vec<String> {
    cmd.get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

const MINIMAL_OK: &str = "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n\tlogallrefupdates = true\n[remote \"origin\"]\n\turl = example-remote\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n[user]\n\tname = Test\n\temail = test@example.invalid\n";

// ---------------------------------------------------------------- vet: grammar accepted

#[test]
fn minimal_allowlisted_config_is_accepted() {
    let dir = project_with_config(MINIMAL_OK.as_bytes());
    assert_eq!(vet_git_config(dir.path()), Ok(()));
}

#[test]
fn comments_blank_lines_and_space_indentation_are_accepted() {
    let cfg = "# top comment\n; semicolon comment\n\n[core]\n    bare = false\n\t; inner\n[init]\n\tdefaultBranch = main\n";
    let dir = project_with_config(cfg.as_bytes());
    assert_eq!(vet_git_config(dir.path()), Ok(()));
}

#[test]
fn config_exactly_500_lines_is_accepted() {
    let cfg = "#\n".repeat(500);
    let dir = project_with_config(cfg.as_bytes());
    assert_eq!(vet_git_config(dir.path()), Ok(()));
}

#[test]
fn config_exactly_64_kib_single_line_is_accepted() {
    let mut cfg = String::from("#");
    cfg.push_str(&"a".repeat(65536 - 2));
    cfg.push('\n');
    assert_eq!(cfg.len(), 65536);
    let dir = project_with_config(cfg.as_bytes());
    assert_eq!(vet_git_config(dir.path()), Ok(()));
}

// ---------------------------------------------------------------- vet: forbidden keys named

#[test]
fn every_forbidden_section_and_key_is_refused_with_its_name() {
    let cases: &[(&str, &str)] = &[
        ("[core]\n\tfsmonitor = touch m\n", "fsmonitor"),
        ("[core]\n\tsshCommand = x\n", "sshcommand"),
        ("[core]\n\thooksPath = x\n", "hookspath"),
        ("[core]\n\tpager = x\n", "pager"),
        ("[core]\n\teditor = x\n", "editor"),
        ("[filter \"x\"]\n\tclean = touch m\n", "filter"),
        ("[filter \"lfs\"]\n\tprocess = x\n", "filter"),
        ("[include]\n\tpath = /tmp/x\n", "include"),
        (
            "[includeIf \"gitdir:/tmp/\"]\n\tpath = /tmp/x\n",
            "includeif",
        ),
        ("[alias]\n\tst = status\n", "alias"),
        ("[credential]\n\thelper = x\n", "credential"),
        ("[diff]\n\texternal = x\n", "external"),
        ("[diff \"x\"]\n\ttextconv = x\n", "textconv"),
        ("[diff \"x\"]\n\tcommand = x\n", "command"),
        ("[merge \"x\"]\n\tdriver = x\n", "driver"),
        ("[uploadpack]\n\tpackObjectsHook = x\n", "packobjectshook"),
        ("[remote \"origin\"]\n\tvcs = x\n", "vcs"),
        ("[remote \"origin\"]\n\treceivepack = x\n", "receivepack"),
        ("[remote \"origin\"]\n\tuploadpack = x\n", "uploadpack"),
        ("[remote \"origin\"]\n\tproxy = x\n", "proxy"),
        ("[url \"x\"]\n\tinsteadOf = y\n", "insteadof"),
        ("[gpg]\n\tprogram = x\n", "program"),
        ("[protocol \"file\"]\n\tallow = always\n", "protocol"),
        ("[extensions]\n\tworktreeConfig = true\n", "extensions"),
    ];
    for (cfg, expected) in cases {
        let dir = project_with_config(cfg.as_bytes());
        let name = forbidden_key(vet_git_config(dir.path()));
        let name = name.unwrap_or_else(|| panic!("expected ForbiddenSetting for {cfg:?}"));
        assert!(
            name.contains(expected),
            "error {name:?} should name {expected:?} for {cfg:?}"
        );
    }
}

#[test]
fn case_variant_section_and_key_are_refused() {
    let dir = project_with_config(b"[CORE]\n\tFsMonitor = x\n");
    let name = forbidden_key(vet_git_config(dir.path()));
    assert!(name.is_some_and(|n| n.contains("fsmonitor")));
}

#[test]
fn mixed_case_filter_header_is_refused() {
    let dir = project_with_config(b"[Filter \"x\"]\n\tClean = x\n");
    let name = forbidden_key(vet_git_config(dir.path()));
    assert!(name.is_some_and(|n| n.contains("filter")));
}

#[test]
fn forbidden_key_after_an_allowed_duplicate_section_is_refused() {
    let dir = project_with_config(b"[core]\n\tbare = false\n[core]\n\tfsmonitor = x\n");
    let name = forbidden_key(vet_git_config(dir.path()));
    assert!(
        name.is_some_and(|n| n.contains("fsmonitor")),
        "every occurrence must be checked"
    );
}

#[test]
fn error_names_the_key_but_never_its_value() {
    let dir = project_with_config(b"[core]\n\tfsmonitor = SECRET-VALUE-XYZ\n");
    let err = vet_git_config(dir.path());
    let debug = format!("{err:?}");
    assert!(!debug.contains("SECRET-VALUE-XYZ"), "value leaked: {debug}");
    assert!(debug.to_ascii_lowercase().contains("fsmonitor"));
}

// ---------------------------------------------------------------- vet: fail-closed grammar

#[test]
fn grammar_violations_refuse_the_whole_config() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        (
            "backslash continuation",
            b"[core]\n\tbare = false \\\n\tfsmonitor = x\n".to_vec(),
        ),
        (
            "legacy remote header",
            b"[remote.origin]\n\turl = example-remote\n".to_vec(),
        ),
        (
            "legacy filter header",
            b"[filter.x]\n\tclean = x\n".to_vec(),
        ),
        ("key on header line", b"[core] bare = false\n".to_vec()),
        (
            "trailing text after header",
            b"[core] # trailing\n".to_vec(),
        ),
        ("boolean shorthand", b"[core]\n\tbare\n".to_vec()),
        ("backslash in value", b"[user]\n\tname = a\\b\n".to_vec()),
        ("double quote in value", b"[user]\n\tname = a\"b\n".to_vec()),
        ("NUL byte", b"[user]\n\tname = a\0b\n".to_vec()),
        ("non-UTF-8", b"[user]\n\tname = \xff\n".to_vec()),
        ("501 lines", "#\n".repeat(501).into_bytes()),
        ("over 64 KiB", {
            let mut s = String::from("#");
            s.push_str(&"a".repeat(65536));
            s.push('\n');
            s.into_bytes()
        }),
    ];
    for (label, cfg) in cases {
        let dir = project_with_config(&cfg);
        assert!(
            vet_git_config(dir.path()).is_err(),
            "{label}: config must be refused before spawn"
        );
    }
}

#[test]
fn continuation_hiding_a_forbidden_key_is_refused() {
    let dir = project_with_config(b"[core]\n\tbare = false\\\n\nfsmonitor = touch m\n");
    assert!(vet_git_config(dir.path()).is_err());
}

#[test]
fn missing_git_config_file_is_a_malformed_or_missing_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
    assert!(vet_git_config(dir.path()).is_err());
}

// ---------------------------------------------------------------- vet: layout

#[test]
fn plain_directory_without_git_is_not_a_repo() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert_eq!(vet_git_config(dir.path()), Err(VetError::NotARepo));
}

#[test]
fn dot_git_file_pointer_is_refused_as_unsupported_layout() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(".git"), "gitdir: /tmp/elsewhere/.git\n").expect("write");
    assert!(matches!(
        vet_git_config(dir.path()),
        Err(VetError::UnsupportedLayout(_))
    ));
}

#[test]
fn dot_git_symlink_is_refused_as_unsupported_layout() {
    let outside = project_with_config(MINIMAL_OK.as_bytes());
    let dir = tempfile::tempdir().expect("tempdir");
    std::os::unix::fs::symlink(outside.path().join(".git"), dir.path().join(".git"))
        .expect("symlink");
    assert!(matches!(
        vet_git_config(dir.path()),
        Err(VetError::UnsupportedLayout(_))
    ));
}

#[test]
fn commondir_is_refused_as_unsupported_layout() {
    let dir = project_with_config(MINIMAL_OK.as_bytes());
    std::fs::write(dir.path().join(".git/commondir"), "../other\n").expect("write");
    assert!(matches!(
        vet_git_config(dir.path()),
        Err(VetError::UnsupportedLayout(_))
    ));
}

#[test]
fn config_worktree_is_refused_as_unsupported_layout() {
    let dir = project_with_config(MINIMAL_OK.as_bytes());
    std::fs::write(
        dir.path().join(".git/config.worktree"),
        "[core]\n\tbare = false\n",
    )
    .expect("write");
    assert!(matches!(
        vet_git_config(dir.path()),
        Err(VetError::UnsupportedLayout(_))
    ));
}

#[test]
fn alternates_file_is_refused_as_unsupported_layout() {
    let dir = project_with_config(MINIMAL_OK.as_bytes());
    std::fs::create_dir_all(dir.path().join(".git/objects/info")).expect("mkdir");
    std::fs::write(
        dir.path().join(".git/objects/info/alternates"),
        "/tmp/other\n",
    )
    .expect("write");
    assert!(matches!(
        vet_git_config(dir.path()),
        Err(VetError::UnsupportedLayout(_))
    ));
}

// ---------------------------------------------------------------- argv and env (pure)

#[test]
fn git_missing_message_when_no_trusted_binary_exists() {
    // Dedicated test for the 'git is not installed' path. This machine has git,
    // so the refusal branch can only be asserted when no trusted binary exists.
    let dir = tempfile::tempdir().expect("tempdir");
    match trusted_git() {
        Some(_) => eprintln!("trusted git present: git-missing branch not reachable here"),
        None => {
            assert_eq!(
                build_git_command(dir.path(), &["status"]).map(|_| ()),
                Err(VetError::GitMissing)
            );
            let msg = git_status_summary("proj", dir.path()).await_blocking();
            assert!(msg.is_err_and(|m| m.contains("git is not installed")));
        }
    }
}

trait BlockingOnce<T> {
    fn await_blocking(self) -> T;
}

impl<F: std::future::Future> BlockingOnce<F::Output> for F {
    fn await_blocking(self) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(self)
    }
}

#[test]
fn status_argv_is_exact_and_pins_git_dir_and_work_tree() {
    if require_git("status_argv_is_exact_and_pins_git_dir_and_work_tree").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let d = dir.path().display().to_string();
    let cmd = build_git_command(
        dir.path(),
        &[
            "status",
            "--porcelain=v1",
            "--branch",
            "--no-renames",
            "--ignore-submodules=all",
        ],
    )
    .expect("build");
    let expected: Vec<String> = [
        "--no-optional-locks",
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([
        format!("--git-dir={d}/.git"),
        format!("--work-tree={d}"),
        "status".to_string(),
        "--porcelain=v1".to_string(),
        "--branch".to_string(),
        "--no-renames".to_string(),
        "--ignore-submodules=all".to_string(),
    ])
    .collect();
    assert_eq!(args_of(&cmd), expected);
}

#[test]
fn log_argv_never_recurses_into_submodules() {
    if require_git("log_argv_never_recurses_into_submodules").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let cmd = build_git_command(dir.path(), &["log", "-n", "5", "--format=%h %s"]).expect("build");
    let args = args_of(&cmd);
    assert_eq!(args[args.len() - 4..], ["log", "-n", "5", "--format=%h %s"]);
    assert!(args.iter().all(|a| !a.contains("recurse")));
    assert!(args.iter().any(|a| a.starts_with("--git-dir=")));
    assert!(args.iter().any(|a| a.starts_with("--work-tree=")));
}

#[test]
fn command_runs_in_the_project_directory() {
    if require_git("command_runs_in_the_project_directory").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let cmd = build_git_command(dir.path(), &["status"]).expect("build");
    assert_eq!(cmd.get_current_dir(), Some(dir.path()));
}

#[test]
fn program_is_the_first_existing_trusted_absolute_git() {
    if require_git("program_is_the_first_existing_trusted_absolute_git").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let cmd = build_git_command(dir.path(), &["status"]).expect("build");
    let program = PathBuf::from(cmd.get_program());
    assert!(
        program.is_absolute(),
        "git must never be resolved through PATH"
    );
    assert_eq!(Some(program), trusted_git());
}

#[test]
fn child_env_is_exactly_the_fixed_set_with_fixed_path() {
    if require_git("child_env_is_exactly_the_fixed_set_with_fixed_path").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let cmd = build_git_command(dir.path(), &["status"]).expect("build");
    let expected: BTreeMap<String, String> = [
        ("PATH", FIXED_PATH),
        ("HOME", "/nonexistent"),
        ("LC_ALL", "C"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_ATTR_NOSYSTEM", "1"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_PAGER", "cat"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(env_map(&cmd), expected);
}

#[test]
fn inherited_git_env_of_the_test_process_is_not_passed_on() {
    if require_git("inherited_git_env_of_the_test_process_is_not_passed_on").is_none() {
        return;
    }
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var("GIT_DIR", "/tmp/vox-foreign/.git");
    std::env::set_var("GIT_WORK_TREE", "/tmp/vox-foreign");
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var("GIT_CONFIG_KEY_0", "core.fsmonitor");
    std::env::set_var("GIT_CONFIG_VALUE_0", "touch /tmp/vox-never");
    let cmd = build_git_command(dir.path(), &["status"]);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
    ] {
        std::env::remove_var(key);
    }
    let cmd = cmd.expect("build");
    let env = env_map(&cmd);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
    ] {
        assert!(!env.contains_key(key), "{key} leaked into the git child");
    }
}

#[test]
fn parent_path_with_relative_entries_is_never_used() {
    if require_git("parent_path_with_relative_entries_is_never_used").is_none() {
        return;
    }
    let _guard = env_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let saved = std::env::var_os("PATH");
    std::env::set_var("PATH", ".:..");
    let cmd = build_git_command(dir.path(), &["status"]);
    match saved {
        Some(v) => std::env::set_var("PATH", v),
        None => std::env::remove_var("PATH"),
    }
    let cmd = cmd.expect("build");
    let path = env_map(&cmd).get("PATH").cloned().unwrap_or_default();
    assert_eq!(path, FIXED_PATH);
    assert!(!path.split(':').any(|e| e == "." || e == ".."));
}

// ---------------------------------------------------------------- end-to-end with real git

#[test]
fn clean_filter_in_repo_config_is_refused_and_marker_absent() {
    if require_git("clean_filter_in_repo_config_is_refused_and_marker_absent").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    let marker = dir.path().join("MARKER");
    let mut cfg = std::fs::read_to_string(dir.path().join(".git/config")).expect("read");
    cfg.push_str(&format!(
        "[filter \"x\"]\n\tclean = touch {}\n",
        marker.display()
    ));
    std::fs::write(dir.path().join(".git/config"), cfg).expect("write");
    std::fs::write(dir.path().join(".gitattributes"), "* filter=x\n").expect("write");
    std::fs::write(dir.path().join("a.txt"), "changed\n").expect("write");
    let result = git_status_summary("proj", dir.path()).await_blocking();
    assert!(result.is_err(), "filter config must be refused");
    assert!(!marker.exists(), "clean filter ran");
}

#[test]
fn core_fsmonitor_marker_in_repo_config_is_refused_and_marker_absent() {
    if require_git("core_fsmonitor_marker_in_repo_config_is_refused_and_marker_absent").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    let marker = dir.path().join("MARKER");
    let mut cfg = std::fs::read_to_string(dir.path().join(".git/config")).expect("read");
    cfg.push_str(&format!(
        "[core]\n\tfsmonitor = touch {}\n",
        marker.display()
    ));
    std::fs::write(dir.path().join(".git/config"), cfg).expect("write");
    assert!(git_status_summary("proj", dir.path())
        .await_blocking()
        .is_err());
    assert!(!marker.exists());
}

#[test]
fn forbidden_keys_in_tricky_syntax_are_refused_before_spawn() {
    if require_git("forbidden_keys_in_tricky_syntax_are_refused_before_spawn").is_none() {
        return;
    }
    let variants = [
        "[CORE]\n\tFsMonitor = touch {m}\n",
        "[core]\n\tbare = false \\\n\tfsmonitor = touch {m}\n",
        "[core.fsmonitor]\n\tx = y\n[core]\n\tfsmonitor = touch {m}\n",
        "[core] fsmonitor = touch {m}\n",
        "[core]\n\tfsmonitor\n",
    ];
    for (i, template) in variants.iter().enumerate() {
        let dir = new_repo_with_commit("first");
        let marker = dir.path().join(format!("MARKER{i}"));
        let mut cfg = std::fs::read_to_string(dir.path().join(".git/config")).expect("read");
        cfg.push_str(&template.replace("{m}", &marker.display().to_string()));
        std::fs::write(dir.path().join(".git/config"), cfg).expect("write");
        assert!(
            git_status_summary("proj", dir.path())
                .await_blocking()
                .is_err(),
            "variant {i} must be refused"
        );
        assert!(!marker.exists(), "variant {i} spawned git");
    }
}

#[test]
fn include_of_external_file_is_refused() {
    if require_git("include_of_external_file_is_refused").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    let outside = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("MARKER");
    let ext = outside.path().join("evil.gitconfig");
    std::fs::write(
        &ext,
        format!("[core]\n\tfsmonitor = touch {}\n", marker.display()),
    )
    .expect("write");
    let mut cfg = std::fs::read_to_string(dir.path().join(".git/config")).expect("read");
    cfg.push_str(&format!("[include]\n\tpath = {}\n", ext.display()));
    std::fs::write(dir.path().join(".git/config"), cfg).expect("write");
    assert!(git_status_summary("proj", dir.path())
        .await_blocking()
        .is_err());
    assert!(!marker.exists());
}

#[test]
fn dot_git_pointer_to_external_gitdir_is_refused() {
    if require_git("dot_git_pointer_to_external_gitdir_is_refused").is_none() {
        return;
    }
    let external = new_repo_with_commit("external subject");
    let marker = external.path().join("MARKER");
    let mut cfg = std::fs::read_to_string(external.path().join(".git/config")).expect("read");
    cfg.push_str(&format!(
        "[core]\n\tfsmonitor = touch {}\n",
        marker.display()
    ));
    std::fs::write(external.path().join(".git/config"), cfg).expect("write");
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join(".git"),
        format!("gitdir: {}\n", external.path().join(".git").display()),
    )
    .expect("write");
    let result = git_status_summary("proj", dir.path()).await_blocking();
    assert!(result.is_err());
    assert!(!marker.exists());
    assert!(result
        .err()
        .is_some_and(|m| !m.contains("external subject")));
}

#[test]
fn inherited_git_env_in_test_process_does_not_change_the_result() {
    if require_git("inherited_git_env_in_test_process_does_not_change_the_result").is_none() {
        return;
    }
    let _guard = env_lock();
    let own = new_repo_with_commit("own subject");
    let foreign = new_repo_with_commit("foreign subject");
    let marker = own.path().join("MARKER");
    std::env::set_var("GIT_DIR", foreign.path().join(".git"));
    std::env::set_var("GIT_CONFIG_COUNT", "1");
    std::env::set_var("GIT_CONFIG_KEY_0", "core.fsmonitor");
    std::env::set_var("GIT_CONFIG_VALUE_0", format!("touch {}", marker.display()));
    let result = git_status_summary("proj", own.path()).await_blocking();
    for key in [
        "GIT_DIR",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_0",
        "GIT_CONFIG_VALUE_0",
    ] {
        std::env::remove_var(key);
    }
    let out = result.expect("own clean repo must succeed");
    assert!(out.contains("own subject"));
    assert!(!out.contains("foreign subject"));
    assert!(!marker.exists());
}

#[test]
fn project_local_executable_named_git_is_never_run() {
    if require_git("project_local_executable_named_git_is_never_run").is_none() {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let _guard = env_lock();
    let dir = new_repo_with_commit("first");
    let marker = dir.path().join("MARKER");
    let fake = dir.path().join("git");
    std::fs::write(&fake, format!("#!/bin/sh\ntouch {}\n", marker.display())).expect("write");
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let saved = std::env::var_os("PATH");
    std::env::set_var("PATH", ".:..");
    let result = git_status_summary("proj", dir.path()).await_blocking();
    match saved {
        Some(v) => std::env::set_var("PATH", v),
        None => std::env::remove_var("PATH"),
    }
    assert!(result.is_ok(), "real git must still run: {result:?}");
    assert!(!marker.exists(), "project-local git was executed");
}

#[test]
fn submodule_with_its_own_clean_filter_is_never_entered() {
    if require_git("submodule_with_its_own_clean_filter_is_never_entered").is_none() {
        return;
    }
    let sub_src = new_repo_with_commit("sub subject");
    let outer = new_repo_with_commit("outer subject");
    fixture_git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            &sub_src.path().display().to_string(),
            "subm",
        ],
    );
    commit_all(outer.path(), "add submodule");
    let marker = outer.path().join("MARKER");
    let modules_cfg = outer.path().join(".git/modules/subm/config");
    let mut cfg = std::fs::read_to_string(&modules_cfg).expect("read submodule config");
    cfg.push_str(&format!(
        "[filter \"x\"]\n\tclean = touch {}\n",
        marker.display()
    ));
    std::fs::write(&modules_cfg, cfg).expect("write");
    let result = git_status_summary("proj", outer.path()).await_blocking();
    assert!(result.is_ok(), "outer clean repo must succeed: {result:?}");
    assert!(!marker.exists(), "submodule was entered");
}

#[test]
fn clean_repo_reports_branch_and_last_commit_subject_without_upstream() {
    if require_git("clean_repo_reports_branch_and_last_commit_subject_without_upstream").is_none() {
        return;
    }
    let dir = new_repo_with_commit("initial subject");
    let out = git_status_summary("proj", dir.path())
        .await_blocking()
        .expect("status");
    assert!(out.contains("main"), "branch missing: {out}");
    assert!(
        out.contains("initial subject"),
        "last commit subject missing: {out}"
    );
    assert!(!out.contains("ahead"), "no upstream means no ahead count");
    assert!(!out.contains("behind"), "no upstream means no behind count");
    assert!(!out.contains("fatal"));
}

#[test]
fn dirty_repo_reports_modified_staged_and_untracked() {
    if require_git("dirty_repo_reports_modified_staged_and_untracked").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    std::fs::write(dir.path().join("a.txt"), "changed\n").expect("write");
    std::fs::write(dir.path().join("staged.txt"), "s\n").expect("write");
    fixture_git(dir.path(), &["add", "staged.txt"]);
    std::fs::write(dir.path().join("new.txt"), "n\n").expect("write");
    let out = git_status_summary("proj", dir.path())
        .await_blocking()
        .expect("status");
    assert!(out.contains("M a.txt"), "modified missing: {out}");
    assert!(out.contains("A  staged.txt"), "staged missing: {out}");
    assert!(out.contains("?? new.txt"), "untracked missing: {out}");
}

#[test]
fn repo_with_zero_commits_reports_status_without_fatal_error() {
    if require_git("repo_with_zero_commits_reports_status_without_fatal_error").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    fixture_git(dir.path(), &["-c", "init.defaultBranch=main", "init", "-q"]);
    std::fs::write(dir.path().join("a.txt"), "x\n").expect("write");
    let out = git_status_summary("proj", dir.path()).await_blocking();
    let out = out.expect("zero-commit repo must not be an error");
    assert!(out.contains("main"));
    assert!(!out.contains("fatal"));
}

#[test]
fn detached_head_is_reported_as_no_branch() {
    if require_git("detached_head_is_reported_as_no_branch").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    std::fs::write(dir.path().join("a.txt"), "two\n").expect("write");
    commit_all(dir.path(), "second");
    fixture_git(dir.path(), &["checkout", "-q", "--detach"]);
    let out = git_status_summary("proj", dir.path())
        .await_blocking()
        .expect("status");
    assert!(
        out.contains("HEAD (no branch)"),
        "detached head missing: {out}"
    );
}

#[test]
fn huge_status_is_capped_with_a_more_marker() {
    if require_git("huge_status_is_capped_with_a_more_marker").is_none() {
        return;
    }
    let dir = new_repo_with_commit("first");
    for i in 0..3000 {
        std::fs::write(dir.path().join(format!("f{i:05}.txt")), "u\n").expect("write");
    }
    let out = git_status_summary("proj", dir.path())
        .await_blocking()
        .expect("status");
    let untracked_lines = out.lines().filter(|l| l.contains("?? ")).count();
    assert!(
        untracked_lines <= 40,
        "status not capped: {untracked_lines} lines"
    );
    assert!(out.contains("more)"), "missing '(N more)' marker");
    assert!(out.len() < 64 * 1024);
}

#[test]
fn non_repo_directory_gets_a_clear_message() {
    if require_git("non_repo_directory_gets_a_clear_message").is_none() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let err = git_status_summary("proj", dir.path())
        .await_blocking()
        .expect_err("must fail");
    assert!(err.contains("proj"), "name missing: {err}");
    assert!(err.contains("is not a git repository"), "message: {err}");
}
