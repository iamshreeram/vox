# Worktree Convention

Applies the project's `using-git-worktrees` methodology to this specific
repo/plan. Read this before starting any phase.

## Directory & branch naming

```
.worktrees/phase-<n>-<slug>/     e.g. .worktrees/phase-1-memory/
branch: phase/<n>-<slug>         e.g. phase/1-memory
```

`<n>` and `<slug>` must match the phase's row in `STATUS.md` exactly.

## One-time setup (first phase started in this clone)

```bash
# .worktrees/ must be git-ignored before anything is created in it
git check-ignore -q .worktrees || {
  echo ".worktrees/" >> .gitignore
  git add .gitignore
  git commit -m "chore: ignore .worktrees/ for parallel phase development"
  git push
}
```

## Starting a phase

```bash
PHASE_NUM=1
PHASE_SLUG=memory
BRANCH="phase/${PHASE_NUM}-${PHASE_SLUG}"
WORKTREE=".worktrees/phase-${PHASE_NUM}-${PHASE_SLUG}"

# From repo root, on main, AFTER claiming the phase in STATUS.md (see STATUS.md)
git checkout main && git pull
git worktree add "$WORKTREE" -b "$BRANCH"
cd "$WORKTREE"

# Project setup
bun install
cd src-tauri && cargo build && cd ..

# Baseline check — must be green before writing a single line of new code
cargo test --manifest-path src-tauri/Cargo.toml
bun run lint
```

If the baseline is not green, stop and report it — do not start a phase on
top of a broken `main`.

## While working

- Commit frequently, small commits, one Red→Green→Refactor cycle per commit
  where practical (`test: add failing test for X` then `feat: implement X`).
- Never touch files outside your phase's module list (see the phase doc's
  "Files touched" section) except the specific integration points it names
  (e.g. registering a new manager in `lib.rs`'s setup). If you find yourself
  needing to edit something another phase also needs, stop and flag it in
  `STATUS.md` as a cross-phase conflict rather than guessing.
- Run `cargo fmt && cargo clippy` and `bun run format` before every commit,
  not just at the end.

## Finishing a phase

```bash
# Inside the worktree
cargo test --manifest-path src-tauri/Cargo.toml   # full workspace, not just new module
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
bun run lint && bun run format:check

git push -u origin "$BRANCH"
```

Open a PR from `$BRANCH` into `main`. Update `STATUS.md` on `main` (separate
small commit, not part of the feature branch) to `in_review` with the PR
link, then `done` once merged.

## Cleaning up after merge

```bash
cd ../..            # back to the primary checkout, out of the worktree
git worktree remove ".worktrees/phase-${PHASE_NUM}-${PHASE_SLUG}"
git branch -d "phase/${PHASE_NUM}-${PHASE_SLUG}"
```

## Running two phases at once on one machine

Each worktree is a fully independent checkout sharing one `.git` — you can
have `.worktrees/phase-1-memory/` and `.worktrees/phase-4-tts/` open in two
separate terminals/editors/agent sessions simultaneously with zero
interference, including running `cargo build`/`cargo test` concurrently in
each (they get independent `target/` dirs by default under each worktree).
