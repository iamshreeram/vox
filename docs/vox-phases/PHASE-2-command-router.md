# Phase 2 — Command Router + Safety Confirmation Gate

**Branch:** `phase/2-command-router` · **Worktree:** `.worktrees/phase-2-command-router/`
**Depends on:** nothing · **Blocks:** nothing (Phase 3 will later wire into it)

## 1. Goal

Deterministic, zero-LLM voice-command matching ("open iTerm", "open
Downloads") plus a confirmation gate for phrases that sound consequential
("delete", "send", "push") — mirroring vox's
`src/vox/commands/router.py` + `src/vox/safety/policy.py`.

## 2. Non-goals

- No actual command _execution_ integration with the dictation hotkey flow
  yet — that's a later wiring phase. This phase delivers a `CommandRouter`
  and `SafetyPolicy` that take a transcript string and return a decision;
  proving the decision logic is correct is the whole job here.
- No arbitrary shell execution. The action set is a small, fixed,
  deterministic list (open app, open path/URL) — never "run this shell
  command the user said out loud."
- No fuzzy ML-based intent classification. Matching is deterministic string
  matching (exact phrase list + installed-app/known-folder lookup), same
  as vox.

## 3. Design

New files: `src-tauri/src/managers/command_router.rs`,
`src-tauri/src/managers/safety_policy.rs`.

### Data model

```rust
pub enum RouteDecision {
    /// Matched a known deterministic action.
    Matched { action: CommandAction },
    /// No deterministic match — caller (a later phase) should escalate
    /// to the agent bridge or do nothing, router doesn't decide that.
    NoMatch,
}

pub enum CommandAction {
    OpenApp { name: String, resolved_path: PathBuf },
    OpenPath { path: PathBuf },
    OpenUrl { url: String },
}

pub enum SafetyDecision {
    /// Not consequential — proceed immediately.
    Allow,
    /// Consequential — caller must get a "confirm"/"cancel" response
    /// before proceeding. Carries the original text so the caller can
    /// re-check it after confirmation.
    RequireConfirmation { original_text: String },
}
```

### `CommandRouter`

- Reuse `strsim`/`natural` (already dependencies) for fuzzy-but-bounded
  matching the same way vox does "launch Visual Studio Code" matching
  against an actually-installed app list — **do not guess an app that
  isn't installed.** If there's no confident match, return `NoMatch`;
  never execute a lower-confidence guess.
- App discovery is platform-specific:
  - macOS: scan `/Applications` and `~/Applications` for `.app` bundles.
  - Windows/Linux: out of scope for this phase (return `NoMatch` for app-open
    commands on non-macOS; the router itself must still compile and its
    unit tests must still pass on all platforms using fakes/fixtures, per
    the platform-gating ground rule — only the _real_ filesystem scan is
    macOS-only).
- Known-folder lookup (Downloads, Documents, Desktop, Home, Pictures,
  Movies, Music) works cross-platform via the `dirs`-style standard paths
  Rust already has conventions for elsewhere in this codebase — check
  `utils.rs` for any existing platform-path helpers before adding new ones.

### `SafetyPolicy`

```rust
impl SafetyPolicy {
    /// Deterministic keyword gate, NOT an LLM call. Mirrors vox's
    /// `policy.py`: a configurable list of trigger words/phrases
    /// (default: "delete", "remove", "send", "push", "deploy", "rm ",
    /// "format", "uninstall") checked case-insensitively as whole-word
    /// matches against the text.
    pub fn decision_for_text(&self, text: &str) -> SafetyDecision;
}
```

- Fail-open toward safety: on any internal error/ambiguity, the gate must
  return `RequireConfirmation`, never silently `Allow` (mirrors vox's
  documented "fail-open gates" pattern — uncertain means "do the safer
  thing").
- The trigger word list must be configurable via settings
  (`consequential_action_keywords: Vec<String>`), with the defaults above
  baked in so an empty/fresh install still protects against the common
  cases.

## 4. Functional requirements

- FR1: `CommandRouter::route(text: &str) -> RouteDecision` never panics on
  any input, including empty string, pure whitespace, non-ASCII/unicode
  text, and extremely long strings (10,000+ chars).
- FR2: App-name matching is case-insensitive and tolerant of minor
  phrasing variance ("open iterm" / "launch iTerm" / "open i term" with a
  stray space should all either match or all not match consistently based
  on a defined similarity threshold — pick one via `strsim` and document it
  in a code comment, then the test cases in §6 lock that threshold in).
- FR3: A command phrase that doesn't resemble ANY known action/app/folder
  returns `NoMatch` — the router must never pick the "closest" app above
  the similarity threshold if that closest match is still below threshold.
- FR4: `SafetyPolicy::decision_for_text` matches trigger keywords as whole
  words only ("sendai" the city must NOT trigger on "send"; "please send
  this" MUST trigger on "send").
- FR5: Matching is case-insensitive ("DELETE this file" triggers the same
  as "delete this file").
- FR6: An empty trigger-keyword list (user cleared settings) still falls
  back to `Allow` for everything — this is a deliberate, explicit
  configuration choice by the user, not a bug; document this clearly so a
  future phase doesn't "fix" it into the fail-open default by mistake.

## 5. Non-functional requirements

- NFR1: No network calls anywhere in this phase.
- NFR2: App directory scan is cached/debounced — must not re-scan
  `/Applications` on every single `route()` call (vox's own router does
  dynamic app lookup but caches it; check `src/vox/commands/app_registry.py`
  in the vox repo for the exact caching behavior to mirror, e.g. TTL or
  invalidate-on-settings-change).
- NFR3: No new third-party crate without checking `strsim`/`natural`
  first — they almost certainly cover what's needed here.

## 6. Test cases (write FIRST, red→green→refactor)

### `CommandRouter` — open app

| #   | Given installed apps           | Input                                                   | Expect                                                                                                                                                                                                                                                                                            |
| --- | ------------------------------ | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| T1  | `["iTerm.app", "Safari.app"]`  | `"open iterm"`                                          | `Matched { OpenApp { name: "iTerm", .. } }`                                                                                                                                                                                                                                                       |
| T2  | `["iTerm.app"]`                | `"open ITERM"`                                          | Matched (case-insensitive)                                                                                                                                                                                                                                                                        |
| T3  | `["Visual Studio Code.app"]`   | `"launch Visual Studio Code"`                           | Matched                                                                                                                                                                                                                                                                                           |
| T4  | `["Visual Studio Code.app"]`   | `"launch VS Code"`                                      | Define and assert one consistent outcome (either matched via an alias table, or `NoMatch` if no alias exists yet) — do not leave this ambiguous; pick `NoMatch` for this phase (no alias table yet) and assert it explicitly so a future phase changing this is a deliberate, visible test change |
| T5  | `[]` (no apps installed/found) | `"open iterm"`                                          | `NoMatch`                                                                                                                                                                                                                                                                                         |
| T6  | `["iTerm.app"]`                | `"open xcodebuilder"` (no such app, not similar enough) | `NoMatch`                                                                                                                                                                                                                                                                                         |
| T7  | `["iTerm.app"]`                | `""` (empty)                                            | `NoMatch`, no panic                                                                                                                                                                                                                                                                               |
| T8  | `["iTerm.app"]`                | `""` (emoji/unicode garbage)                            | `NoMatch`, no panic                                                                                                                                                                                                                                                                               |
| T9  | `["iTerm.app"]`                | 10,000-character string of random words                 | `NoMatch` (or matched if it happens to contain "open iterm" — the point of this test is "doesn't panic/hang", assert it returns within a reasonable time, e.g. under 100ms)                                                                                                                       |

### `CommandRouter` — open path/folder

| #   | Input                        | Expect                                                    |
| --- | ---------------------------- | --------------------------------------------------------- |
| T10 | `"open downloads"`           | `Matched { OpenPath { path: <platform Downloads dir> } }` |
| T11 | `"open my downloads folder"` | Matched (same as T10 — trailing/filler words tolerated)   |
| T12 | `"open the moon"`            | `NoMatch`                                                 |

### `SafetyPolicy`

| #   | Trigger list               | Input                      | Expect                                                                  |
| --- | -------------------------- | -------------------------- | ----------------------------------------------------------------------- |
| T13 | default                    | `"delete the file"`        | `RequireConfirmation`                                                   |
| T14 | default                    | `"DELETE the file"`        | `RequireConfirmation` (case-insensitive)                                |
| T15 | default                    | `"I live in Sendai"`       | `Allow` (whole-word match only — "send" must not match inside "Sendai") |
| T16 | default                    | `"please send this email"` | `RequireConfirmation`                                                   |
| T17 | default                    | `"what's the weather"`     | `Allow`                                                                 |
| T18 | `[]` (empty, user-cleared) | `"delete everything"`      | `Allow` (explicit opt-out, see FR6)                                     |
| T19 | custom `["banana"]`        | `"please banana the repo"` | `RequireConfirmation`                                                   |
| T20 | default                    | `""`                       | `Allow` (empty text is not consequential)                               |
| T21 | default                    | `"rm -rf /"`               | `RequireConfirmation` (matches `"rm "` trigger)                         |

### Integration sanity

| #   | Scenario                                                                   | Expect                                                                                                              |
| --- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| T22 | `route()` called from 2 threads concurrently with a shared router instance | No panic, no data race (run under `cargo test` normally; if a cache is added per NFR2, specifically stress it here) |

## 7. Acceptance criteria

- [ ] Every test in §6 exists, watched failing, then passing.
- [ ] `cargo clippy -- -D warnings` clean.
- [ ] Works correctly on macOS (real app scan) and compiles + passes all
      unit tests (with fakes standing in for the filesystem scan) on
      Windows/Linux CI.
- [ ] No arbitrary shell/process execution exists anywhere in this phase's
      code — `grep -r "Command::new" src-tauri/src/managers/command_router.rs`
      should only ever show platform "open this app/path" launchers
      (`open` on macOS, equivalent elsewhere), never a user-supplied
      string passed to a shell.
- [ ] `SafetyPolicy` default trigger list matches §3's documented defaults
      exactly.
