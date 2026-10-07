# Vox Build Phases — Index

This directory is the single source of truth for turning **Vox** (formerly
Handy, rebranded per the fork-must-rebrand clause in the project's
[LICENSE notice](../../README.md#license)) into the full local voice-assistant
platform originally prototyped in the Python `vox` project
(`/Users/s0v06yb/ram/projects/python/vox`), reimplemented natively in Rust on
top of Handy's existing dictation core.

**Read this file first.** Then read [`STATUS.md`](./STATUS.md) to see what's
claimed/done, then read the one phase file you're building.

## Why phases, and why this structure

Handy's dictation pipeline (STT, VAD, paste, model management) is already
best-in-class and **is not touched by any phase below** — it is the stable
foundation every phase builds on top of, using the existing `managers/`
pattern (`src-tauri/src/managers/{audio,model,transcription}.rs`). Each phase
adds exactly one new manager/subsystem, independently testable, independently
mergeable, and (critically) **independently assignable to a different model
in its own git worktree** so phases can be built in parallel without stepping
on each other's files.

See the feasibility/scoping conversation that produced this plan for the
full feature-by-feature rationale. Short version: this is a multi-week,
phased engineering project, not a single patch — treat each phase as its
own mini-project with its own branch, its own tests, and its own PR.

## Ground rules (apply to every phase, no exceptions)

1. **TDD, strictly.** See the `test-driven-development` skill. No production
   code without a failing test first. Red → Green → Refactor. If you wrote
   code before the test, delete it and start over — no exceptions, no
   "I'll backfill the test."
2. **One phase = one git worktree = one branch.** See
   [`WORKTREE-CONVENTION.md`](./WORKTREE-CONVENTION.md). Never build two
   phases in the same working tree; never build on `main` directly.
3. **A phase isn't done until:**
   - Every test case listed in that phase's doc exists, was watched failing,
     then watched passing.
   - `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test`
     are all clean for the whole workspace (not just the new module).
   - `bun run format:check` and `bun run lint` are clean, if the phase
     touched any frontend code.
   - The phase's own "Acceptance Criteria" checklist is fully checked.
   - `STATUS.md` is updated and the branch is pushed.
4. **Platform gating.** Every phase below is macOS-first (vox's own scope
   was macOS-only). Gate new code behind `cfg(target_os = "macos")`
   following the existing pattern (`tauri-nspanel`, `objc2`, Metal backend in
   `Cargo.toml`). Windows/Linux builds must keep compiling and passing their
   existing tests untouched.
5. **Reuse what's already a dependency before adding a new one.** Handy's
   `Cargo.toml` already carries `rusqlite` (bundled), `objc2`/`objc2-app-kit`/
   `objc2-foundation` (macOS FFI), `rdev` (hotkeys), `vad-rs` + `earshot`
   (VAD), `rustfft` (spectral/audio math), `strsim` + `natural` (fuzzy text
   matching), `transcribe-rs`/`ort` (ONNX inference), `reqwest`, `tokio`.
   Check this list before reaching for a new crate.
6. **No feature flag left half-wired.** Every new subsystem must be fully
   off-by-default and a no-op when its setting is disabled, mirroring vox's
   own `None`-when-disabled pattern — zero behavior change for users who
   don't opt in.

## Phase order & dependency graph

```
Phase 1: Memory (SQLite facts + recall)         [no deps]
Phase 2: Command Router + Safety Gate           [no deps]
Phase 3: Agent Bridge (external CLI agent subprocess)  [no deps]
Phase 4: Native TTS (macOS AVSpeechSynthesizer) [no deps]
Phase 5: Wake Word (ONNX, openWakeWord-style)   [depends on: existing VAD]
Phase 6: Ambient Mode (rolling transcript)      [depends on: Phase 5]
Phase 7: Screen OCR (Apple Vision)              [no deps, lowest priority]
```

Phases 1-4 and 7 have **zero cross-dependencies** — five different models
in five different worktrees can build all of them simultaneously right now.
Phase 6 must start after Phase 5 is merged to `main` (or at minimum branched
from Phase 5's branch — see `STATUS.md` for current branch heads).

## Files in this directory

| File | Purpose |
|---|---|
| `STATUS.md` | Live phase claim/progress table. Read before claiming work, update when claiming/finishing. |
| `WORKTREE-CONVENTION.md` | Exact directory/branch naming + setup/teardown commands. |
| `PHASE-1-memory.md` | SQLite memory manager: facts, recall gate. |
| `PHASE-2-command-router.md` | Deterministic command routing + consequential-action confirmation gate. |
| `PHASE-3-agent-bridge.md` | Subprocess bridge to a user-configured external coding-agent CLI for escalated/delegated requests. |
| `PHASE-4-tts.md` | Native macOS text-to-speech output manager. |
| `PHASE-5-wakeword.md` | Opt-in "Hey Vox"-style wake word detection. |
| `PHASE-6-ambient-mode.md` | Rolling-transcript ambient listening + Engagement Judge. |
| `PHASE-7-vision-ocr.md` | Opt-in "what's on my screen" OCR voice command. |

## How to actually build one

Run the `/voxrs-goals` slash command (see
`.agents/commands/voxrs-goals.md` at the repo root). It automates: claiming
a phase in `STATUS.md`, creating the worktree, loading the right phase doc,
and running the TDD loop end-to-end. You can hand that command to a cheap/
"dumb" model — the phase docs are written to leave as little to judgment
as possible.
