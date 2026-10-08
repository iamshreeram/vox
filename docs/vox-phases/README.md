# Vox Build Phases — Index

This directory is the source of truth for the staged Rust implementation of the Vox local voice-assistant roadmap. The phase specifications are cross-checked against a separate Python Vox implementation where relevant; they record expected behavior, tests, and deliberate porting differences.

**Read this file first.** Then read [`STATUS.md`](./STATUS.md) to see current phase state and [`WORKTREE-CONVENTION.md`](./WORKTREE-CONVENTION.md) before creating a worktree.

## Why phases

Vox's existing dictation pipeline (speech recognition, VAD, paste, and model management) is the stable foundation. Each phase adds one independently testable subsystem in its own branch and worktree, avoiding collisions between parallel efforts.

Each phase is a separate engineering task with explicit scope, tests, acceptance criteria, and a reviewable branch. Do not treat this as one large patch.

## Ground rules

1. **TDD, strictly.** Write a failing test, observe the expected failure, implement the minimum behavior, and refactor with tests green.
2. **One phase = one worktree = one branch.** Never build two phases in the same working tree or implement phase work directly on `main`.
3. **Validate honestly.** Run the listed tests, formatting, lint, and clippy checks. If a baseline check already fails outside your changes, report it; do not claim a clean result.
4. **Preserve cross-platform builds.** Gate platform-specific code appropriately and keep other supported targets compiling.
5. **Reuse existing dependencies** before proposing a new crate. Document and justify any new dependency.
6. **Feature flags default off.** A disabled subsystem must have no behavior or resource-consumption side effects.
7. **Personal GitHub identity and content guardrails are mandatory.** Follow the repository's `AGENTS.md` and `CONTRIBUTING.md` before committing or pushing.

## Phase order

```
Phase 1: Memory (SQLite facts + deterministic recall) [no dependencies]
Phase 2: Command Router + Safety Gate              [no dependencies]
Phase 3: Generic Agent Bridge                      [no dependencies]
Phase 4: Native TTS                                 [no dependencies]
Phase 5: Wake Word                                  [uses existing VAD]
Phase 6: Ambient Mode                               [depends on Phase 5]
Phase 7: Screen OCR                                 [no dependencies]
```

Phases 1-5 and 7 can be developed independently. Phase 6 depends on Phase 5 and must branch from a commit that contains the wake-word interface.

## Phase documents

| File                        | Purpose                                                     |
| --------------------------- | ----------------------------------------------------------- |
| `STATUS.md`                 | Phase ownership and review state.                           |
| `WORKTREE-CONVENTION.md`    | Worktree names and setup/teardown.                          |
| `PHASE-1-memory.md`         | SQLite memory facts, extraction, deduplication, and recall. |
| `PHASE-2-command-router.md` | Deterministic app/path routing and safety confirmation.     |
| `PHASE-3-agent-bridge.md`   | Generic, user-configured subprocess bridge.                 |
| `PHASE-4-tts.md`            | Native macOS speech output.                                 |
| `PHASE-5-wakeword.md`       | Opt-in wake-word detection.                                 |
| `PHASE-6-ambient-mode.md`   | RAM-only rolling transcript and engagement decision.        |
| `PHASE-7-vision-ocr.md`     | Opt-in, observation-only screen OCR.                        |
| `PHASE-5-6-TASK-QUEUE.md`   | Multi-agent fan-out task breakdown for Phases 5+6 (read this before picking up any Phase 5/6 task). |
| `tasks/phase-5-tasks.md`    | Phase 5 task cards (files, tests, acceptance criteria per task). |
| `tasks/phase-6-tasks.md`    | Phase 6 task cards. |
| `tasks/ui-wiring-tasks.md`  | Cross-cutting Settings UI task cards shared by Phases 5 and 6. |
| `BACKLOG-voice-command-expansion.md` | Ideas backlog for "transcript into action" follow-ups beyond Phase 2 -- not a claimed phase, see the file itself before promoting anything out of it. |

## Starting a phase

The `/voxrs-goals` command documents the repository phase workflow. It does not override personal-repository identity/content guardrails, tests, or the human-only PR submission rule.
