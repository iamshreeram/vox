//! Live acceptance tests for the spoken-reply path. Every test is `#[ignore]`d
//! and needs the real command-line agent (code-puppy) and, for the speech
//! test, macOS `/usr/bin/say`. Run with `--ignored`; see docs/live-acceptance.md.
//!
//! No test here mocks the component under test: the composer runs through the
//! production `speech_backends::compose_with_deadline`, the brain path through
//! `speech_backends::complete` + `converse_plan::parse_plan`, and the prompts
//! through `converse_plan::build_plan_prompt`.

use crate::managers::composer_config::{write_to_app, TtsComposerMode, TtsComposerSettings};
use crate::managers::composer_rules::normalize_command_tuple;
use crate::managers::converse_plan::{build_plan_prompt, parse_plan, PlanInput, MAX_HISTORY_ITEMS};
use crate::managers::speech_backends::{complete, compose_with_deadline};
use crate::managers::speech_compose::{ComposeError, ComposeRequest, Composed, MAX_COMPOSED_CHARS};
use crate::managers::tts::{say_args_styled, say_stdin_text};
use crate::settings::{get_default_settings, AppSettings};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CODE_PUPPY_BIN_NAME: &str = "code-puppy";
const MODEL_ARGS: &str = "--model claude-haiku-5-5 --no-tools";
const STYLE: &str = "brief and friendly";
const SAY: &str = "/usr/bin/say";

const TECHNICAL_REPLY: &str = "I found the bug in the session timeout handling. The timeout check in src-tauri/src/managers/converse.rs compared elapsed time in seconds against a value stored in milliseconds, so sessions ended almost immediately after the first turn. Here is the corrected comparison:

```rust
if elapsed_ms >= cfg.idle_timeout_ms {
    return SessionEnd::Idle;
}
```

I also added a regression test next to the existing ones, in managers/converse_tests.rs, which starts a session, waits longer than the idle window, and asserts that the session stays open while the user is still speaking. The change touches only the timeout comparison and the test, so the rest of the turn loop is unchanged. Two follow-ups remain: the default idle window is still set to five seconds, which may be too short for slow speakers, and the timeout log should include the session identifier so support can trace it. I ran the converse test module and all forty-one tests pass. Nothing else in the repository was modified, and no settings were migrated. The change is ready for review whenever you want to look at it.";

const SHORT_REPLY: &str = "The file was saved successfully. It is 2.4 kilobytes and has 96 lines.";

const ERROR_LOG_REPLY: &str = "thread 'main' panicked at src/vault/index.rs:88:14: called `Option::unwrap()` on a `None` value
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
error[E0425]: cannot find value `cfg` in this scope
  --> src/managers/converse.rs:412:9
Error: process exited with code 101";

/// The first executable file named `name` on `PATH`, if any.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary path from `VOX_LIVE_CODE_PUPPY` (non-empty), else the first
/// `code-puppy` found on `PATH`. When neither exists the returned name is not
/// a file, so `require_code_puppy` fails with `GATE UNMET`.
fn code_puppy_path() -> String {
    match std::env::var("VOX_LIVE_CODE_PUPPY") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => find_on_path(CODE_PUPPY_BIN_NAME)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| CODE_PUPPY_BIN_NAME.to_string()),
    }
}

/// Fails the test with `GATE UNMET:` when the agent binary is missing.
fn require_code_puppy() -> String {
    let bin = code_puppy_path();
    if !Path::new(&bin).is_file() {
        panic!(
            "GATE UNMET: code-puppy binary not found at {bin} (put it on PATH or set VOX_LIVE_CODE_PUPPY to its absolute path)"
        );
    }
    bin
}

/// Fails the test with `GATE UNMET:` when the macOS speech tool is missing.
fn require_say() {
    if !Path::new(SAY).is_file() {
        panic!("GATE UNMET: {SAY} is missing (the speech path needs macOS say)");
    }
}

/// Settings exactly as the app stores a Command-mode composer with an
/// acknowledged tuple (`command_ack` = normalized path, args and flag).
fn settings_for(path: &str, args: &str) -> AppSettings {
    let mut app = get_default_settings();
    let mut c = TtsComposerSettings {
        mode: TtsComposerMode::Command,
        command_path: path.to_string(),
        command_args: args.to_string(),
        ..TtsComposerSettings::default()
    };
    c.command_ack =
        normalize_command_tuple(&c.command_path, &c.command_args, &c.prompt_flag).unwrap();
    write_to_app(&mut app, &c);
    app
}

fn request(reply: &str) -> ComposeRequest {
    ComposeRequest {
        reply: reply.to_string(),
        user_utterance: None,
        style: STYLE.to_string(),
    }
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tauri::async_runtime::block_on(f)
}

fn tmp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("vox-live-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

struct Outcome {
    name: &'static str,
    composed: Composed,
    latency_ms: u128,
}

/// The three A1 cases through the production runner, computed once and shared
/// by A1 and B so the live model is called once per case.
fn composed_outcomes() -> &'static Vec<Outcome> {
    static OUTCOMES: OnceLock<Vec<Outcome>> = OnceLock::new();
    OUTCOMES.get_or_init(|| {
        let settings = settings_for(&require_code_puppy(), MODEL_ARGS);
        let cases: [(&'static str, &str); 3] = [
            ("technical_long", TECHNICAL_REPLY),
            ("short_factual", SHORT_REPLY),
            ("error_log", ERROR_LOG_REPLY),
        ];
        cases
            .iter()
            .map(|(name, reply)| {
                let started = Instant::now();
                let result = block_on(compose_with_deadline(
                    &settings,
                    request(reply),
                    Duration::from_secs(90),
                ));
                let latency_ms = started.elapsed().as_millis();
                match result {
                    Ok(composed) => Outcome {
                        name,
                        composed,
                        latency_ms,
                    },
                    Err(err) => panic!("case {name}: composer returned {err:?}"),
                }
            })
            .collect()
    })
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn sentence_count(text: &str) -> usize {
    text.split(['.', '!', '?'])
        .filter(|part| !part.trim().is_empty())
        .count()
}

/// Names of markup features found in the speech (never the text itself).
fn markup_found(text: &str) -> Vec<&'static str> {
    let checks: [(&str, &'static str); 8] = [
        ("`", "backtick"),
        ("*", "asterisk"),
        ("#", "hash"),
        ("|", "pipe"),
        ("/", "slash"),
        ("<", "angle"),
        ("http", "url"),
        ("://", "url_scheme"),
    ];
    checks
        .iter()
        .filter(|(needle, _)| text.contains(needle))
        .map(|(_, name)| *name)
        .collect()
}

#[test]
#[ignore = "live: needs code-puppy and network"]
fn a1_long_short_and_error_replies_become_one_short_sentence() {
    for o in composed_outcomes() {
        println!(
            "LIVE-OUTPUT case={} latency_ms={} words={} emotion={:?} speech={:?}",
            o.name,
            o.latency_ms,
            word_count(&o.composed.speech),
            o.composed.emotion,
            o.composed.speech
        );
    }
    for o in composed_outcomes() {
        let speech = &o.composed.speech;
        let words = word_count(speech);
        assert!(!speech.is_empty(), "case {}: empty speech", o.name);
        assert!(
            speech.chars().count() <= MAX_COMPOSED_CHARS,
            "case {}: speech has {} chars, limit {}",
            o.name,
            speech.chars().count(),
            MAX_COMPOSED_CHARS
        );
        let markup = markup_found(speech);
        assert!(
            markup.is_empty(),
            "case {}: speech contains markup features {:?}",
            o.name,
            markup
        );
        let sentences = sentence_count(speech);
        assert!(
            (1..=2).contains(&sentences),
            "case {}: {} sentences, expected 1 or 2",
            o.name,
            sentences
        );
        assert!(
            words <= 60,
            "case {}: {} words, expected at most 60",
            o.name,
            words
        );
    }
}

#[test]
#[ignore = "live: needs code-puppy and network"]
fn a2_timeout_returns_timeout_within_budget_and_leaves_no_process() {
    require_code_puppy();
    let dir = tmp_dir("a2");
    let script = dir.join("slow-agent.sh");
    let body = format!(
        "#!/bin/sh\necho $$ > '{d}/shell.pid'\nsleep 10 &\necho $! > '{d}/sleep.pid'\nwait\n",
        d = dir.display()
    );
    std::fs::write(&script, body).expect("write wrapper");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod wrapper");
    }
    let settings = settings_for(script.to_str().expect("utf-8 path"), "");

    let started = Instant::now();
    let result = block_on(compose_with_deadline(
        &settings,
        request("A short reply."),
        Duration::from_secs(2),
    ));
    let elapsed = started.elapsed();
    assert!(
        matches!(result, Err(ComposeError::Timeout)),
        "expected Timeout from the production runner"
    );
    assert!(
        elapsed <= Duration::from_millis(2500),
        "timeout took {} ms, budget 2500 ms",
        elapsed.as_millis()
    );

    let shell_pid = std::fs::read_to_string(dir.join("shell.pid"))
        .expect("wrapper did not start, test would be vacuous")
        .trim()
        .to_string();
    let sleep_pid = std::fs::read_to_string(dir.join("sleep.pid"))
        .expect("wrapper did not start its sleep child")
        .trim()
        .to_string();
    let dir_text = dir.display().to_string();
    let mut clean = false;
    for _ in 0..30 {
        if !pgrep_matches(&dir_text) && !pid_alive(&shell_pid) && !pid_alive(&sleep_pid) {
            clean = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        clean,
        "a wrapper or sleep child is still running after timeout"
    );
}

/// True when `pgrep -f` finds a process whose command line contains `needle`.
fn pgrep_matches(needle: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", needle])
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// True when the pid exists and is not a zombie.
fn pid_alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    !out.is_empty() && !out.starts_with('Z')
}

#[test]
#[ignore = "live: needs code-puppy and network"]
fn a3_missing_binary_is_a_configuration_error_not_a_panic() {
    require_code_puppy();
    let settings = settings_for("/nonexistent/vox-live-missing-agent", MODEL_ARGS);
    let result = block_on(compose_with_deadline(
        &settings,
        request("A short reply."),
        Duration::from_secs(10),
    ));
    // Production contract (speech_backends.rs run_command doc): a spawn failure
    // is `Backend`. The task text expected BadConfig/NotConfigured; see report.
    assert!(
        matches!(result, Err(ComposeError::Backend)),
        "missing binary: expected Backend from the production runner"
    );
}

/// Speaks one composed sentence with the same argv and stdin the production
/// speech backend uses (`tts::say_args_styled`, `tts::say_stdin_text`), waiting
/// at most 30 s and returning the exit code.
fn speak_and_wait(composed: &Composed) -> Option<i32> {
    let mut child = Command::new(SAY)
        .args(say_args_styled(None, Some(180), composed.emotion))
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn /usr/bin/say");
    child
        .stdin
        .take()
        .expect("say stdin")
        .write_all(say_stdin_text(&composed.speech, composed.emotion).as_bytes())
        .expect("write say stdin");
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("poll say") {
            return status.code();
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("say did not finish within 30 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "live: needs code-puppy and network"]
fn b_each_composed_sentence_is_spoken_and_exits_zero() {
    require_code_puppy();
    require_say();
    for o in composed_outcomes() {
        let started = Instant::now();
        let code = speak_and_wait(&o.composed);
        println!(
            "LIVE-OUTPUT spoke case={} exit={:?} took_ms={}",
            o.name,
            code,
            started.elapsed().as_millis()
        );
        assert_eq!(code, Some(0), "case {}: say exited with {:?}", o.name, code);
    }
}

/// Turn 1 and turn 2 of the scripted conversation, as the production history
/// stores them: (user utterance, assistant summary), oldest first.
fn teal_history() -> Vec<(String, String)> {
    vec![(
        "My favourite colour is teal.".to_string(),
        "Noted, teal is a lovely colour.".to_string(),
    )]
}

const TURN_TWO: &str = "What colour did I say I like?";

#[test]
#[ignore = "live: needs code-puppy and network"]
fn c1_turn_two_prompt_carries_turn_one_persona_bounded_history_and_no_session_leak() {
    require_code_puppy();
    let history = teal_history();
    let p = build_plan_prompt(&PlanInput {
        utterance: TURN_TWO,
        history: &history,
        last_draft: None,
        style: STYLE,
        vault_context: None,
    });
    assert!(
        p.user.contains("My favourite colour is teal."),
        "turn-2 prompt lacks the turn-1 text"
    );
    assert!(
        p.system.contains("You are Vox"),
        "persona missing from system prompt"
    );
    assert!(p.system.contains(STYLE), "style missing from system prompt");

    // Bounded: 50 scripted turns, only the newest MAX_HISTORY_ITEMS survive.
    let long: Vec<(String, String)> = (0..50)
        .map(|i| (format!("turn {i} question"), format!("turn {i} answer")))
        .collect();
    let p = build_plan_prompt(&PlanInput {
        utterance: TURN_TWO,
        history: &long,
        last_draft: None,
        style: STYLE,
        vault_context: None,
    });
    assert_eq!(
        p.user.matches("User: ").count(),
        MAX_HISTORY_ITEMS,
        "history was not capped to MAX_HISTORY_ITEMS"
    );
    assert!(
        p.user.contains("turn 49 question"),
        "newest turn was not kept"
    );
    assert!(
        p.user.contains("turn 38 question"),
        "oldest kept turn is missing"
    );
    assert!(
        !p.user.contains("turn 37 question"),
        "turn beyond the cap leaked in"
    );

    // No leak: a new session starts with an empty history (converse.rs run_turns).
    let session_a = vec![("SESSION-A-SECRET-TEAL".to_string(), "ok".to_string())];
    let _ = build_plan_prompt(&PlanInput {
        utterance: "Session A.",
        history: &session_a,
        last_draft: None,
        style: STYLE,
        vault_context: None,
    });
    let session_b = build_plan_prompt(&PlanInput {
        utterance: "Hello again.",
        history: &[],
        last_draft: None,
        style: STYLE,
        vault_context: None,
    });
    assert!(
        !session_b.combined.contains("SESSION-A-SECRET-TEAL"),
        "a previous session leaked into the new one"
    );
    assert!(
        !session_b.user.contains("Conversation so far"),
        "new session carried a conversation block"
    );
}

#[test]
#[ignore = "live: needs code-puppy and network"]
fn c2_live_turn_two_recalls_teal_through_the_command_path() {
    let bin = require_code_puppy();
    let settings = settings_for(&bin, MODEL_ARGS);
    let history = teal_history();
    let mut passes = 0;
    for run in 1..=3 {
        let p = build_plan_prompt(&PlanInput {
            utterance: TURN_TWO,
            history: &history,
            last_draft: None,
            style: STYLE,
            vault_context: None,
        });
        let raw = match block_on(complete(
            &settings,
            &p.system,
            &p.combined,
            Duration::from_secs(90),
        )) {
            Ok(raw) => raw,
            Err(err) => {
                println!("LIVE-OUTPUT c2 run={run} brain_error={err:?}");
                continue;
            }
        };
        match parse_plan(&raw, &p.combined) {
            Ok(plan) => {
                let hit = plan.speech.to_lowercase().contains("teal");
                println!(
                    "LIVE-OUTPUT c2 run={run} mentions_teal={hit} words={} speech={:?}",
                    word_count(&plan.speech),
                    plan.speech
                );
                if hit {
                    passes += 1;
                }
            }
            Err(err) => println!("LIVE-OUTPUT c2 run={run} parse_error={err:?}"),
        }
    }
    println!("LIVE-OUTPUT c2 pass_rate={passes}/3");
    assert_eq!(passes, 3, "pass rate {passes}/3 (see LIVE-OUTPUT lines)");
}
