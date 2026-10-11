#!/usr/bin/env bash
# One command to go from "whatever's currently checked out" to "latest code,
# rebuilt, installed, and running" -- chains together the steps a human would
# otherwise run by hand after every merge:
#
#   fast-forward to origin  ->  bun install  ->  tauri build (unsigned CI config)
#   ->  install-macos.sh
#
# Safety rails (each one refuses BEFORE anything is built or installed):
#   - a single-instance lock in ${TMPDIR:-/tmp}/vox-update.lock (never removed
#     automatically; a stale lock must be removed by hand, see the message)
#   - refuses when tracked files are modified (untracked files are ignored)
#   - refuses on a detached HEAD
#   - refuses unless `origin` is the official Vox remote (or the exact URL in
#     VOX_UPDATE_ALLOW_ORIGIN, which is meant for forks)
#   - VOX_SOURCE_UPDATE=1 refuses unless the checked-out branch is main
#   - only ever fast-forwards; a diverged branch is reported, never merged
#
# The build uses src-tauri/tauri.ci-unsigned.conf.json, which turns updater
# artifacts off, so a build failure is a real failure and is not masked by the
# missing signing key. A failed build never reaches the installer.
#
# Libraries live in scripts/lib/ (update-lock.sh, origin-check.sh).
#
# Usage:
#   ./scripts/update-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# shellcheck source=lib/update-lock.sh
. "scripts/lib/update-lock.sh"
# shellcheck source=lib/origin-check.sh
. "scripts/lib/origin-check.sh"

# ---- 0. Single-instance lock (taken first, released on every exit) ---------
TMP_BASE="${TMPDIR:-/tmp}"
while [ "${TMP_BASE%/}" != "$TMP_BASE" ]; do
  TMP_BASE="${TMP_BASE%/}"
done
if [ -z "$TMP_BASE" ]; then
  TMP_BASE="/"
fi
LOCK_DIR="$TMP_BASE/vox-update.lock"

if ! acquire_update_lock "$LOCK_DIR"; then
  exit 1
fi
trap release_update_lock EXIT

# Everything below runs inside a function. `git merge --ff-only` can rewrite
# THIS file while bash is still reading it (bash reads scripts incrementally),
# which would make the rest of the run execute garbage. Bash parses a whole
# function before running it, and the final call line is read in one piece.
main() {
  echo "== Updating Vox =="

  # ---- 1. Refuse to clobber uncommitted work --------------------------------
  # Untracked files are deliberately ignored: they cannot be overwritten by a
  # fast-forward merge of tracked files, and a stray scratch file should not
  # block updates.
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    echo "You have uncommitted changes to tracked files -- not updating on top of them." >&2
    echo "Commit, stash, or discard them first, then re-run this script." >&2
    git status --short --untracked-files=no >&2
    exit 1
  fi

  BRANCH="$(git branch --show-current)"
  if [ -z "$BRANCH" ]; then
    echo "Not currently on a branch (detached HEAD) -- check out a branch first." >&2
    exit 1
  fi

  # ---- 2. Only update from the official origin ------------------------------
  ORIGIN_URL="$(git remote get-url origin 2>/dev/null || true)"
  if ! origin_allowed "$ORIGIN_URL"; then
    echo "Refusing to update: remote 'origin' is '$ORIGIN_URL'," >&2
    echo "which is not the official Vox origin (github.com/iamshreeram/vox)." >&2
    echo "Forks may set VOX_UPDATE_ALLOW_ORIGIN to their exact origin URL." >&2
    exit 1
  fi

  # ---- 3. Source-update mode only builds main -------------------------------
  if [ "${VOX_SOURCE_UPDATE:-}" = "1" ] && [ "$BRANCH" != "main" ]; then
    echo "VOX_SOURCE_UPDATE=1 requires the 'main' branch (currently on '$BRANCH')." >&2
    exit 1
  fi

  # ---- 4. Fast-forward to origin, never merge --------------------------------
  echo "Fast-forwarding '${BRANCH}' to 'origin/${BRANCH}'..."
  git fetch origin
  if ! git merge --ff-only "origin/${BRANCH}"; then
    echo "Cannot fast-forward '${BRANCH}' to 'origin/${BRANCH}': the local branch has diverged." >&2
    echo "Nothing was changed. Resolve it by hand (for example, move your local commits" >&2
    echo "elsewhere and reset '${BRANCH}' to 'origin/${BRANCH}'), then re-run this script." >&2
    exit 1
  fi

  AFTER_REV="$(git rev-parse --short HEAD)"

  # ---- 5. Keep JS deps in sync, then build the release bundle ---------------
  echo "Installing JS dependencies (no-op if nothing changed)..."
  bun install

  APP_BUNDLE="src-tauri/target/release/bundle/macos/Vox.app"
  # Remove any previous bundle so a failed build cannot leave an old app behind.
  rm -rf "$APP_BUNDLE"

  export VOX_BUILD_COMMIT="$(git rev-parse HEAD)" VOX_BUILD_BRANCH="$BRANCH" VOX_SOURCE_UPDATES=1

  # --bundles app: the installer only needs Vox.app, so skip the DMG.
  echo "Building release bundle -- this takes a few minutes..."
  bun run tauri build --bundles app --config src-tauri/tauri.ci-unsigned.conf.json

  if [ ! -d "$APP_BUNDLE" ]; then
    echo "Build did not produce $APP_BUNDLE -- not installing." >&2
    exit 1
  fi

  # ---- 6. Install (handles stop/re-sign/restart) -----------------------------
  ./scripts/install-macos.sh "$APP_BUNDLE"

  echo ""
  echo "== Update complete: now on $AFTER_REV =="
}

main "$@"; exit $?
