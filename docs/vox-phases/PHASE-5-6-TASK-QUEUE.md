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

Branch naming: `phase/<5|6>-<slug>/<task-id>-<short-name>`
Worktree naming: `.worktrees/phase-<5|6>-<task-id>-<short-name>/`

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
| W1 | Settings + scaffolding | — | queued | — |
| W2 | Frame buffering/validation | W1 | queued | — |
| W3 | Model download/cache | W1 | queued | — |
| W4 | Real ONNX inference pipeline | W2, W3 | queued | — |
| W5 | pause/resume + multi-instance isolation | W1 | queued | — |
| W6 | Tauri commands + events | W1 | queued | — |
| W7 | Mic-flow integration (scope ext.) | W4, W5, W6 | queued | — |
| W8 | Settings UI (scope ext.) | W6 (finalize after W7) | queued | — |
| A1 | Settings + scaffolding | — | queued | — |
| A2 | RollingTranscript | A1 | queued | — |
| A3 | EngagementJudge | A1 | queued | — |
| A4 | EchoSuppressor | A1 | queued | — |
| A5 | Coordinator + mutual exclusion | A2, A3, A4, W1 | queued | — |
| A6 | Tauri commands + events | A1 | queued | — |
| A7 | Mic-flow integration (scope ext.) | A5, A6 | queued | — |
| A8 | Settings UI (scope ext.) | A6 (finalize after A7) | queued | — |
| U1 | Shared Listening Mode UI section | W8, A8 | queued | — |
| U2 | Permission/onboarding copy | W7 or A7 | queued | — |
| U3 | i18n + final lint/build pass | W8, A8, U1 | queued | — |

Status values: `queued`, `in_progress (<worktree>)`, `in_review
(<branch>)`, `merged`.
