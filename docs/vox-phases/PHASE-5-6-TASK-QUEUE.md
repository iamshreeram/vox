# Phase 5 + 6 Task Queue — Multi-Agent Fan-Out Plan

This is the **process document** for building Phase 5 (Wake Word) and
Phase 6 (Ambient Mode) across multiple models/agents in parallel. It does
not replace `PHASE-5-wakeword.md` / `PHASE-6-ambient-mode.md` — those
remain the spec of record (design, full test tables, acceptance criteria).
This document decomposes those specs into small, independently-assignable
tasks, defines who does what, and defines how work gets merged safely.

**Read this file, then `tasks/phase-5-tasks.md`, `tasks/phase-6-tasks.md`,
and `tasks/ui-wiring-tasks.md` before picking up any task.**

## Roles

| Role | Who | Responsibility |
| --- | --- | --- |
| **Planner** | `claude-5-sonnet-long` | Wrote this task breakdown and the per-task test lists. Owns re-planning if a task reveals the breakdown was wrong. |
| **Implementer** | `gpt6.1-luna` | Picks up exactly one task card at a time, works in that task's own worktree, follows strict TDD (red → green → refactor), stops at the task's file boundary, reports back with test output. |
| **Validator / Orchestrator** | `claude-5-sonnet-long` (same model as Planner, acting as the "main model" session) | Reviews every task branch before merge: runs the full test suite (not just the task's new tests), clippy, fmt, lint/build for UI tasks, checks file-boundary compliance, merges into the phase integration branch, updates this queue's status table, and is the only one deciding a phase is ready for Ram to open a PR. |

**Hard rule:** an Implementer never merges their own task branch into the
phase integration branch. The Validator always reviews first. This is the
same spirit as the repo's existing "AI prepares, human submits the PR"
rule, one level down.

## Scope note: this extends past the original phase docs

`PHASE-5-wakeword.md` and `PHASE-6-ambient-mode.md` both explicitly scope
themselves as "deliver the isolated subsystem, defer wiring into the live
mic/dictation flow to a later phase." Ram's instruction for this round is
explicit: **the UI must be wired for both** — a user must be able to turn
these on from Settings and actually experience them. So this task queue
adds tasks beyond each phase doc's own listed test tables (continuous-mic
integration, Settings UI, cross-feature mutual-exclusion UX). Those extra
tasks are marked **(scope extension)** below so nobody mistakes them for
part of the original ported-from-Python spec.

## Branch & worktree topology

Two integration branches already exist (created off `main` at `d853691a`):

```
phase/5-wakeword          .worktrees/phase-5-wakeword/
phase/6-ambient-mode      .worktrees/phase-6-ambient-mode/
```

Each task gets its **own** worktree, branched off the current tip of its
phase's integration branch (not off `main`):

```bash
# Example: starting task W2 (Phase 5)
cd /Users/s0v06yb/ram/projects/rust/vox
git fetch origin
git checkout phase/5-wakeword && git pull origin phase/5-wakeword
git worktree add .worktrees/phase-5-w2-framing -b phase/5-wakeword/w2-framing
cd .worktrees/phase-5-w2-framing
bun install && cd src-tauri && cargo build && cd ..
cargo test --manifest-path src-tauri/Cargo.toml   # baseline must be green
```

Branch naming: `phase<5|6>-<task-id>-<short-name>` (flat, NOT nested under
`phase/5-wakeword/...` or `phase/6-ambient-mode/...` -- git refs cannot be
both a leaf and a directory, so `phase/5-wakeword/w1-foo` is rejected by
git while `phase/5-wakeword` already exists as a branch. Discovered during
Round 1 (W1/A1) execution 2026-10-08; this flat scheme is the corrected
convention from here on.)
Worktree naming: `.worktrees/phase-<5|6>-<task-id>-<short-name>/` (unchanged)

When a task is validated and merged, its worktree and branch are deleted
(same cleanup commands as `WORKTREE-CONVENTION.md`), freeing the slot for
the next task that depends on it.

## Dependency graph

```
Phase 5 (Wake Word):
  W1 (scaffolding)
   ├─> W2 (frame buffering)      ─┐
   ├─> W3 (model download/cache) ─┼─> W4 (real ONNX pipeline) ─┐
   ├─> W5 (pause/resume)          │                            ├─> W7 (mic integration, scope ext.)
   └─> W6 (Tauri commands+events)─┘                            │        └─> W8 (Settings UI, scope ext.)
                                                                 (W8 can start once W6 lands, using
                                                                  fakes; finalized after W7)

Phase 6 (Ambient Mode) — A5 additionally needs W1 merged (reads
wake_word_enabled for mutual exclusion):
  A1 (scaffolding)
   ├─> A2 (RollingTranscript) ─┐
   ├─> A3 (EngagementJudge)    ├─> A5 (Coordinator + mutual exclusion) ─> A7 (mic integration, scope ext.)
   ├─> A4 (EchoSuppressor)    ─┘        (needs W1 merged too)                   └─> A8 (Settings UI, scope ext.)
   └─> A6 (Tauri commands+events)

Cross-cutting (scope extension, after both phases' UI tasks):
  U1 (shared Listening Mode section, needs W8 + A8)
  U2 (permission/onboarding copy, needs W7 or A7)
  U3 (i18n + final lint/build pass, needs W8 + A8 + U1)
```

Tasks in the same column with no arrow between them are **parallel-safe**
— different files, no shared state. Give each its own worktree
simultaneously.

## Validation checklist (run by the Validator before every merge)

1. `git fetch` the task branch, review the diff for file-boundary
   compliance against the task card's "Files touched" list.
2. `cargo test --manifest-path src-tauri/Cargo.toml --lib` from the task's
   worktree — **full suite**, not just the new tests. Compare the pass
   count to the previous known-good count; investigate any drop or any
   unexpected new failures (including flaky-test false alarms — rerun
   once before concluding a regression).
3. `cargo clippy --manifest-path src-tauri/Cargo.toml --lib -- -D
   warnings` — confirm zero new warnings introduced (pre-existing warnings
   elsewhere in the codebase are not this task's problem, but nothing
   from the new files).
4. `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`.
5. For any task touching `src/` (frontend): `bun run lint` and `bun run
   build`.
6. Confirm every test listed on the task card actually exists in the
   diff, was not deleted/weakened, and the task's stated acceptance
   criteria are met.
7. Merge the task branch into the phase integration branch (regular merge
   commit, keep history — do not squash away the task's TDD commit
   sequence, it's useful review history).
8. Delete the task's worktree and branch.
9. Update the status table below.
10. If the task unblocks other queued tasks, note that in your merge
    report so the Implementer(s) know they can start.

## Phase completion criteria

A phase integration branch (`phase/5-wakeword` or `phase/6-ambient-mode`)
is ready for Ram to open a PR only when:

- Every task card for that phase is merged.
- The full phase doc's own acceptance criteria (in `PHASE-5-wakeword.md` §7
  / `PHASE-6-ambient-mode.md` §7) are independently re-verified by the
  Validator against the merged integration branch, not assumed from
  individual task merges.
- `STATUS.md` is updated to `in_review` with a real PR link once Ram opens
  it (AI does not open PRs, per `CONTRIBUTING.md`).

## Task status table

Updated by the Validator as tasks complete. See `tasks/phase-5-tasks.md`,
`tasks/phase-6-tasks.md`, `tasks/ui-wiring-tasks.md` for full task cards
(description, exact test list, files touched, acceptance criteria).

| ID | Title | Depends on | Status | Branch |
| --- | --- | --- | --- | --- |
| W1 | Settings + scaffolding | — | merged | merged into `phase/5-wakeword` (plus a Validator fix: `WakeWordDetection` was missing `Serialize`/`Deserialize`/`Type` derives in the Implementer's actual commit vs. spec — corrected @ 11993517) |
| W2 | Frame buffering/validation | W1 | merged | `phase5-w2-framing`, merged @ 12ad08e1 |
| W3 | Model download/cache | W1 | merged | `phase5-w3-download`, merged @ 89557656 |
| W4 | Real ONNX inference pipeline | W2, W3 | **BLOCKED** — see note below | — |
| W5 | pause/resume + multi-instance isolation | W1 | merged | `phase5-w5-pause-resume`, merged @ bb32f2a5 (logic already existed from W2; this added T8 + pause/resume tests only) |
| W6 | Tauri commands + events | W1 | merged | `phase5-w6-commands`, merged @ 56c0916a (plus a Validator fix: f32->JSON confidence test compared against an f64 literal, corrected @ d26b9433) |
| W7 | Mic-flow integration (scope ext.) | W4, W5, W6 | queued | — |
| W8 | Settings UI (scope ext.) | W6 (finalize after W7) | queued | — |
| A1 | Settings + scaffolding | — | merged | merged into `phase/6-ambient-mode` @ 0c45a34f |
| A2 | RollingTranscript | A1 | merged | `phase6-a2-transcript`, merged @ fde6fd3f |
| A3 | EngagementJudge | A1 | merged | `phase6-a3-engagement`, merged @ 7459330d |
| A4 | EchoSuppressor | A1 | merged | `phase6-a4-echo`, merged @ 90686d23 |
| A5 | Coordinator + mutual exclusion | A2, A3, A4, W1 | merged | `phase6-a5-coordinator`, merged @ a2d778f9. **Structural change: `phase/6-ambient-mode` now also contains all of `phase/5-wakeword`'s commits** (merged in to get `wake_word_enabled` for the mutual-exclusion logic) -- it is now the combined integration branch. `phase/5-wakeword` is unchanged/standalone and still fine to keep developing W7/W8 against until final combination. |
| A6 | Tauri commands + events | A1 (ACTUALLY also needs A2 -- `ambient_clear()` needs a working `RollingTranscript::clear()`, despite the doc saying "depends on A1 only"; discovered during Round 2 planning) | merged | `phase6-a6-commands`, merged @ cc716533 (plus a Validator fix: `ambient::mod` was missing `RollingTranscript`'s re-export — same class of bug as W1's, corrected @ 1158032a) |
| A7 | Mic-flow integration (scope ext.) | A5, A6 | queued | — |
| A8 | Settings UI (scope ext.) | A6 (finalize after A7) | queued | — |

### Execution notes from Round 1/2 (read before continuing)

- **Branch naming bug (fixed):** see the corrected flat naming convention
  above -- nested branch names under an existing integration branch are
  rejected by git.
- **Implementers may silently rewrite dictated code instead of using it
  verbatim.** W1's actual commit replaced the Validator's exact specified
  `WakeWordDetection` struct with an equivalent-looking rewrite that quietly
  dropped the `Serialize`/`Deserialize`/`Type` derives (needed for Tauri
  event emission + specta bindings). It still passed behavioral verification
  (tests/clippy/fmt all green) because nothing in W1's own test suite
  exercised serialization -- that gap only surfaced when W6 tried to emit
  the struct as an event. **Lesson: behavioral test-passing is not proof of
  literal-spec compliance for "shape" contracts whose properties (like
  derives) aren't exercised by that same task's own tests.** The Validator
  must spot-check actual file contents against the dictated spec, not just
  rerun tests, especially for the first task that defines a shared type.
- **RollingTranscript's method signatures were corrected from `&mut self` to
  `&self`** (A2) -- the phase doc's shown signatures can't satisfy NFR2's
  concurrent-access requirement; `&self` + internal `Mutex` (mirroring
  `MemoryManager`) is required instead.
- **Disk space:** each worktree has its own multi-GB `target/` directory:
  running more than ~3 concurrent `cargo build/test` worktrees at once on a
  460GB disk risks `No space left on device` mid-task (this happened during
  Round 2a and corrupted several Implementers' ability to report verified
  results). Delete a task's worktree (which deletes its `target/`) the
  moment it's merged; don't let old phase worktrees linger after their PR
  merges either.
- **A6's dependency is actually A1 + A2, not A1 alone** -- its
  `ambient_clear()` needs a real, working `RollingTranscript::clear()` to
  be meaningfully testable.
- **The A1 Implementer's rewrite also dropped `ambient::mod`'s `pub use`
  re-exports** (same root cause as the W1 derive bug: a sparser rewrite
  that passed A1's own tests but broke a later consumer -- A6 expected
  `crate::managers::ambient::RollingTranscript` to resolve). Fixed by
  restoring the re-exports with `#[allow(unused_imports)]` (nothing wires
  `AmbientCoordinator` itself in yet, pending A5/A7).
- **`rustfmt --check <files>` including `lib.rs` in the file list
  recursively re-checks the ENTIRE crate's module tree** (it follows `mod`
  declarations from the crate root), producing hundreds of unrelated
  pre-existing diffs in files like `actions.rs` that have nothing to do
  with the task at hand. This caused at least 3 Implementers to report a
  false "fmt failure." Always scope `rustfmt --check` to the specific
  non-root files actually touched.

### W4 formally BLOCKED (2026-10-08) -- do not attempt without explicit sign-off

Investigated directly: the phase doc's instruction to "read
`src/vox/wakeword/openwakeword_engine.py` ... it documents the exact
preprocessing" is **incorrect**. That file is a thin wrapper around the
`openwakeword` PyPI package's `Model.predict()` -- the real melspectrogram
/ embedding / classifier cascade math lives inside that external package,
which is **not installed or vendored anywhere in this environment**
(confirmed: `uv pip show openwakeword` -> not found, no `.onnx` wake-word
model files exist locally, only an unrelated VAD model). Outbound network
access to GitHub IS available, so a full faithful port is *possible* (fetch
real openWakeWord source + real pretrained ONNX files), but doing this
correctly is a substantial, failure-prone undertaking -- the classifier is
sensitive to exact preprocessing match, and a subtly-wrong port would
*silently* ship a non-functional detector with green-looking tests. This is
not something to improvise; it needs an explicit scope decision (full
faithful port vs. harness-only-with-deferred-accuracy-tests vs. defer
entirely) before any Implementer touches it. Asked Ram directly, no
response yet (session timed out) -- defaulted to **not attempting it**
rather than guessing, since a wrong answer here is worse than no answer.
**W7 (mic integration) and W8 (Settings UI) both formally depend on W4** per
the original plan; W7 cannot be meaningfully completed until W4 is resolved
one way or another.
| U1 | Shared Listening Mode UI section | W8, A8 | queued | — |
| U2 | Permission/onboarding copy | W7 or A7 | queued | — |
| U3 | i18n + final lint/build pass | W8, A8, U1 | queued | — |

Status values: `queued`, `in_progress (<worktree>)`, `in_review
(<branch>)`, `merged`.
