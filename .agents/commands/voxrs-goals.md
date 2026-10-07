---
name: voxrs-goals
argument-hint: [phase-number-or-slug]
description: Build or review one Vox-RS roadmap phase via TDD in an isolated worktree, following docs/vox-phases/.
---

# /voxrs-goals — Build one Vox-RS phase

**Argument (optional):** `$ARGUMENTS` — a phase number (`1`-`7`) or slug. If empty, pick the next eligible phase from `docs/vox-phases/STATUS.md`.

## Mandatory personal-repository guardrails

This is Ram's personal GitHub repository. Before creating any commit:

```bash
git setlive
git config user.name
git config user.email
git var GIT_AUTHOR_IDENT
git var GIT_COMMITTER_IDENT
```

Expected identity: `Shreeram <shreeram.v@live.com>`. Before every push, inspect the complete outgoing range:

```bash
git log origin/$(git branch --show-current)..HEAD --format='%h %an <%ae> | %cn <%ce> %s'
```

STOP if any author/committer is not the personal identity. Do not push corporate, tool, or agent identities. Do not put employer branding, internal agents/tools, corporate addresses, or internal artifact/index hosts into source, docs, templates, commits, or assets. Preserve historical technical identifiers only when required for compatibility, and label them as legacy identifiers. Search changed files before commit. AI assistants must not open/submit PRs; Ram submits PRs himself.

## STEP 0: Required skills

Activate `test-driven-development` and `using-git-worktrees` before coding. Follow the TDD iron law: **no production code without a failing test first.**

## STEP 1: Orient

Read:
1. `docs/vox-phases/README.md`
2. `docs/vox-phases/STATUS.md`
3. `docs/vox-phases/WORKTREE-CONVENTION.md`
4. The selected `docs/vox-phases/PHASE-<N>-<slug>.md` in full
5. The corresponding Python Vox implementation and its tests under the local Python Vox checkout, when available. Compare behavior rather than copying names/identifiers; note deliberate differences in the phase spec. Never copy private machine paths into committed docs.

## STEP 2: Select one phase

- If an explicit phase is provided, verify it is eligible and not being worked by someone else.
- If no phase is provided, choose the first eligible `not_started` phase.
- If blocked or already owned by someone else, stop and report that.
- Build only one phase per run. If the user asks for multiple phases, take them one at a time in separate worktrees and report progress clearly.

## STEP 3: Claim without committing to main

Do not commit directly to `main`. Create the phase branch/worktree and record the claim in that branch's `STATUS.md` row with owner `Shreeram`, status `in_progress`, and a UTC timestamp. The owner field is the human project owner, never the assistant/session/tool ID. Commit the claim to the phase branch using the verified personal identity.

Before any push, run the outgoing-identity check above. Push only the phase branch, not `main`. Never force-push unless the user explicitly approves rewriting that branch; if approved, use `--force-with-lease`.

## STEP 4: Create/update the worktree and verify baseline

Follow `WORKTREE-CONVENTION.md`. Use `.worktrees/phase-<N>-<slug>/`. Run setup and baseline tests. If baseline tests or lint fail before implementation, stop and report rather than layering work on a broken baseline.

## STEP 5: TDD implementation

For each test case in the phase doc, in order:
1. Write one test and run it.
2. Confirm it fails for the expected missing behavior, not a test/compile mistake.
3. Implement the minimum production code.
4. Rerun the new and existing tests.
5. Refactor only while tests stay green.
6. Commit focused increments using the verified personal identity.

No arbitrary shell execution from voice text. Respect the phase non-goals. Keep files cohesive and below the repository's 600-line guidance where practical.

## STEP 6: Validate

Run the phase's required full validation commands. Report pre-existing failures separately and do not claim a globally clean check if unrelated baseline debt remains. Run `git diff --check`, inspect changed files for internal/company references and false branding/URLs, then inspect all outgoing author/committer identities.

## STEP 7: Finish

Push the phase branch after the identity and content audits. Update the phase row to `in_review` in the phase branch. Do not open the PR. Give Ram the branch and PR URL to submit manually, plus truthful test results and any known gaps.
