---
name: voxrs-goals
argument-hint: [phase-number-or-slug]
description: Build the next (or specified) Vox-rs phase end-to-end via strict TDD in an isolated git worktree, following docs/vox-phases/. Use when picking up phased Vox-rs development work, with or without a specific phase argument.
---

# /voxrs-goals - Build a Vox-rs phase via TDD in an isolated worktree

**Argument (optional):** `$ARGUMENTS` - a phase number (`1`-`7`) or slug
(e.g. `memory`, `tts`). If empty, pick the next eligible phase yourself
(see STEP 2).

Follow every step below in order. Do not skip steps. Do not improvise
around them even if a shortcut looks obviously fine - the whole point of
this command is that a cheap/low-reasoning model can run it unattended and
still produce correct, reviewable work.

## STEP 0: Activate required skills

Call `activate_skill` for both of these before doing anything else:

- `test-driven-development`
- `using-git-worktrees`

Internalize the TDD iron law before writing a single line of code:
**no production code without a failing test first.** If at any point you
catch yourself writing implementation before its test, stop, delete what
you wrote, and start that piece over with the test.

## STEP 1: Orient yourself

Read, in this order:

1. `docs/vox-phases/README.md` - overall plan, ground rules, dependency
   graph.
2. `docs/vox-phases/STATUS.md` - current claim/progress state.
3. `docs/vox-phases/WORKTREE-CONVENTION.md` - exact commands for worktree
   setup/teardown.

## STEP 2: Pick a phase

- If `$ARGUMENTS` names a phase (number or slug), that is your target.
  Check its row in `STATUS.md`:
  - If `status` is `done` or `in_progress` with a _different_ owner, stop
    and report this to the user instead of proceeding - do not duplicate
    or collide with in-flight work.
  - If `status` is `blocked_on_<n>`, check phase `<n>`'s status; if it is
    not `done`, stop and report that this phase isn't ready yet.
- If `$ARGUMENTS` is empty, scan `STATUS.md` top to bottom and pick the
  first row whose `status` is `not_started` (skip any `blocked_on_<n>` row
  unless phase `<n>` is `done`, in which case treat it as eligible).
- If literally every phase is `done` or legitimately blocked with no
  eligible phase, stop and report that to the user - do not invent new
  scope.

## STEP 3: Claim it

Per `STATUS.md`'s own claim protocol:

```bash
cd /Users/s0v06yb/ram/projects/rust/Handy
git checkout main && git pull
```

Edit `docs/vox-phases/STATUS.md`: set your chosen phase's `status` to
`in_progress`, `owner` to your agent id (use the id you were given, or
`agent` plus a timestamp if you have no specific id), `claimed_at` to
the current UTC time.

```bash
git add docs/vox-phases/STATUS.md
git commit -m "docs(vox-phases): claim phase <N> - <slug>"
git push
```

If the push is rejected, `git pull --rebase` and re-check the row you
claimed - if someone else claimed the exact same phase first, go back to
STEP 2 and pick a different eligible one. Do not force-push.

## STEP 4: Set up the worktree

Follow `WORKTREE-CONVENTION.md` exactly:

```bash
PHASE_NUM=<N>
PHASE_SLUG=<slug>
BRANCH="phase/${PHASE_NUM}-${PHASE_SLUG}"
WORKTREE=".worktrees/phase-${PHASE_NUM}-${PHASE_SLUG}"

git worktree add "$WORKTREE" -b "$BRANCH"
cd "$WORKTREE"
bun install
cd src-tauri && cargo build && cd ..
cargo test --manifest-path src-tauri/Cargo.toml
bun run lint
```

If the baseline `cargo test` or `bun run lint` is not clean on a fresh
worktree off `main`, **stop and report this** rather than building on top
of a broken baseline - this means something upstream regressed and needs
separate attention first.

All remaining work happens **inside this worktree directory**. Do not edit
files in the primary checkout.

## STEP 5: Read the phase doc, fully

Read `docs/vox-phases/PHASE-<N>-<slug>.md` top to bottom before writing
anything. Pay particular attention to:

- Section 2 (Non-goals) - these are explicit scope boundaries, not
  suggestions. Do not implement anything listed there.
- Section 3 (Design) - follow the stated file layout, trait shapes, and
  "before you write any code" instructions (some phases require reading
  an existing file like `managers/history.rs` first to mirror its
  conventions - actually do this, don't skip it).
- Section 7/6 (Test cases) - this is your TDD checklist, in order.

## STEP 6: TDD loop, one test case at a time

For every test case listed in the phase doc's test-case table, in order:

1. **RED** - write that one test. Nothing else. Run it.
2. **Verify it fails for the right reason** (missing feature, not a typo
   or compile error in the test itself). If it errors instead of failing
   cleanly, fix the test until it fails correctly.
3. **GREEN** - write the minimum production code to make that test pass.
   Do not implement later test cases' behavior early "while you're in
   there" - one test at a time.
4. **Verify it passes**, and verify every previously-passing test in this
   phase's module still passes too.
5. **REFACTOR** - clean up naming/duplication now that it's green. Keep
   all tests green through this step.
6. Commit: small commits, e.g. `test(vox-phase-<N>): add <test name>` then
   `feat(vox-phase-<N>): implement <behavior>`.
7. Move to the next test case.

Never write production code for a test case you haven't written yet.
Never mark a test case "done" without having watched it fail first - if
you're not sure whether you actually watched it fail, delete the
implementation and redo that test case properly.

## STEP 7: Full validation before calling the phase done

Run ALL of these, inside the worktree, and they must all be clean:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
bun run lint
bun run format:check
```

Then walk the phase doc's own "Acceptance Criteria" checklist line by
line and confirm each box honestly - do not check a box you didn't
actually verify.

## STEP 8: Finish

```bash
git push -u origin "$BRANCH"
```

Then, back in the **primary checkout** (not the worktree):

```bash
cd /Users/s0v06yb/ram/projects/rust/Handy
git checkout main && git pull
```

Edit `docs/vox-phases/STATUS.md`: set the phase's `status` to `in_review`
and fill in the `pr` column (if you have the ability to open a PR via
`gh pr create` against `iamshreeram/vox`, do so and link it; if not,
state clearly in your final report that a human needs to open the PR from
branch `$BRANCH`).

```bash
git add docs/vox-phases/STATUS.md
git commit -m "docs(vox-phases): phase <N> ready for review"
git push
```

## STEP 9: Report

Tell the user, plainly:

- Which phase you built, and its branch name.
- How many test cases from the doc you implemented, and confirm every one
  was watched red before green.
- The exact validation commands from STEP 7 and that they all passed.
- Any Non-goal you were tempted to implement and correctly didn't.
- Whether a PR was opened, or needs to be opened manually.
- Whether `STATUS.md` was updated on `main`.

Do not start a second phase in the same invocation unless the user
explicitly asked for more than one - one `/voxrs-goals` run builds one
phase.
