# Phase Status

Single source of truth for which phase is claimed/in-progress/done. Read
this before claiming a phase. Update it (on `main`, commit + push) the
moment you claim a phase — before you create your worktree — so two models
starting at nearly the same time don't both grab the same work.

**Claim protocol:**

1. `git checkout main && git pull`
2. Edit your phase's row below: `status` → `in_progress`, `owner` → your
   agent id / model name, `claimed_at` → current UTC timestamp.
3. Commit (`docs(vox-phases): claim phase N`) and push directly to `main`.
   If the push is rejected (someone else pushed first), `pull --rebase` and
   check whether they claimed the _same_ phase — if so, pick a different
   one. This file is intentionally tiny so merge conflicts here are rare
   and trivial to resolve by hand.
4. Only after your claim is pushed, create your worktree (see
   `WORKTREE-CONVENTION.md`) and start the TDD loop.
5. When done (PR opened or merged), flip `status` → `done` and fill in `pr`.

| #   | Phase                        | status       | owner    | branch                   | claimed_at           | pr                                                                                                            |
| --- | ---------------------------- | ------------ | -------- | ------------------------ | -------------------- | ------------------------------------------------------------------------------------------------------------- |
| 1   | Memory                       | done         | Shreeram | `phase/1-memory`         | 2026-10-07T06:54:43Z | https://github.com/iamshreeram/vox/pull/1 (merged)                                                             |
| 2   | Command Router + Safety Gate | done         | Shreeram | `phase/2-command-router` | 2026-10-07T09:35:28Z | https://github.com/iamshreeram/vox/pull/2 (merged)                                                             |
| 3   | Agent Bridge                 | in_progress  | ram-ai-46fb74 | `phase/3-agent-bridge`   | 2026-10-09T03:05:00Z | —                                                                                                             |
| 4   | Native TTS                   | in_progress  | ram-ai-9fb2cd | `phase/4-tts`            | 2026-10-09T18:00:00Z | —                                                                                                             |
| 5   | Wake Word                    | in_progress  | Shreeram | `phase/5-wakeword`       | 2026-10-08T00:00:00Z | multi-agent task queue: see `docs/vox-phases/PHASE-5-6-TASK-QUEUE.md`                                          |
| 6   | Ambient Mode                 | in_progress  | Shreeram | `phase/6-ambient-mode`   | 2026-10-08T00:00:00Z | multi-agent task queue: see `docs/vox-phases/PHASE-5-6-TASK-QUEUE.md`                                          |
| 7   | Screen OCR                   | not_started  | —        | `phase/7-vision-ocr`     | —                    | —                                                                                                             |

Valid `status` values: `not_started`, `blocked_on_<n>`, `in_progress`,
`in_review` (PR open), `done`.
