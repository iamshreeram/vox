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
#     automatically; a stale lock must be removed by hand, see the message).
#     VOX_UPDATE_LOCK_DIR (absolute path) overrides the lock directory.
#   - refuses when tracked files are modified (untracked files are ignored)
#   - refuses on a detached HEAD
#   - refuses unless `origin` is the official Vox remote (or the exact URL in
#     VOX_UPDATE_ALLOW_ORIGIN, which is meant for forks)
#   - VOX_SOURCE_UPDATE=1 refuses unless the checked-out branch is main
#   - only ever fast-forwards; a diverged branch is reported, never merged
#   - VOX_UPDATE_TARGET_SHA (40 lowercase hex) pins the exact commit to build:
#     the script fast-forwards to that commit (never to origin/<branch>) and
#     refuses if it is unreachable, if HEAD is ahead of it, or if they diverged
#   - prefers the rustup toolchain in ~/.cargo/bin over any older rustc on PATH
#
# The build uses src-tauri/tauri.ci-unsigned.conf.json, which turns updater
# artifacts off, so a build failure is a real failure and is not masked by the
# missing signing key. A failed build never reaches the installer.
#
# Progress (only when VOX_UPDATE_STATUS_FILE is set): a JSON status file is
# rewritten atomically through each phase, see scripts/lib/update-status.sh.
# Without that variable the output and behaviour are exactly as before.
#
# Two-phase runs (VOX_UPDATE_PHASE=build|install, see scripts/lib/update-staging.sh):
# `build` stops after the bundle check, stages a copy of the fresh app and writes
# pending.json (status `waiting`); `install` installs exactly that staged copy.
# The default `all` is the single-phase run described above.
#
# Libraries live in scripts/lib/ (update-lock.sh, origin-check.sh,
# update-status.sh, update-staging.sh).
#
# Usage:
#   ./scripts/update-macos.sh
set -euo pipefail
# The HOME this script was started with. Captured before anything can change it.
REAL_HOME="${HOME:-}"
cd "$(dirname "$0")/.."

# Interactive shells can have an older package-manager rustc (e.g. Homebrew)
# earlier on PATH than the rustup toolchain, and cargo then fails with
# "requires rustc 1.88.0". Prefer the rustup toolchain when it is installed.
if [ -d "$HOME/.cargo/bin" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi

# Unattended runs (launched by Vox) must never wait for a credential prompt.
if [ "${VOX_UPDATE_NONINTERACTIVE:-0}" = "1" ]; then
  GIT_TERMINAL_PROMPT=0
  export GIT_TERMINAL_PROMPT
fi

# shellcheck source=lib/update-lock.sh
. "scripts/lib/update-lock.sh"
# shellcheck source=lib/origin-check.sh
. "scripts/lib/origin-check.sh"
# shellcheck source=lib/update-status.sh
. "scripts/lib/update-status.sh"
# shellcheck source=lib/update-staging.sh
. "scripts/lib/update-staging.sh"

# run_bun <args...>: every bun call goes through here.
# Unattended runs (launched by Vox from launchd) use a private HOME: a detached
# job cannot read ~/.npmrc when it is a symlink into a cloud-synced folder, and
# bun then hangs forever. Only bun sees the private HOME; git, install-macos.sh
# and codesign keep the real one. The caches stay at their real locations.
# Interactive runs call bun exactly as before.
run_bun() {
  local base home
  if [ "${VOX_UPDATE_NONINTERACTIVE:-0}" != "1" ]; then
    bun "$@"
    return
  fi
  base="${VOX_UPDATE_STATE_DIR:-}"
  if [ -z "$base" ] && [ -n "${VOX_UPDATE_STATUS_FILE:-}" ]; then
    base="$(dirname "$VOX_UPDATE_STATUS_FILE")"
  fi
  home=""
  case "$base" in
    /*) home="$base/home" ;;
  esac
  if [ -z "$home" ] || ! { mkdir -p "$home" && chmod 700 "$home"; } 2>/dev/null; then
    home="$TMP_BASE/vox-update-home-$(id -u)"
    if ! { mkdir -p "$home" && chmod 700 "$home"; }; then
      echo "Cannot create a private home for bun under $TMP_BASE -- not building." >&2
      return 1
    fi
  fi
  env HOME="$home" \
    CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}" \
    RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}" \
    BUN_INSTALL_CACHE_DIR="${BUN_INSTALL_CACHE_DIR:-$REAL_HOME/.bun/install/cache}" \
    bun "$@"
}

# ---- 0. Single-instance lock (taken first, released on every exit) ---------
TMP_BASE="${TMPDIR:-/tmp}"
while [ "${TMP_BASE%/}" != "$TMP_BASE" ]; do
  TMP_BASE="${TMP_BASE%/}"
done
if [ -z "$TMP_BASE" ]; then
  TMP_BASE="/"
fi
if [ -n "${VOX_UPDATE_LOCK_DIR:-}" ]; then
  case "$VOX_UPDATE_LOCK_DIR" in
    /*) LOCK_DIR="$VOX_UPDATE_LOCK_DIR" ;;
    *)
      echo "VOX_UPDATE_LOCK_DIR must be an absolute path." >&2
      exit 1
      ;;
  esac
else
  LOCK_DIR="$TMP_BASE/vox-update.lock"
fi

# Status file settings are checked before the lock so a bad setup writes nothing.
if ! status_validate_setup; then
  exit 1
fi
if ! staging_validate_setup; then
  exit 1
fi

if ! acquire_update_lock "$LOCK_DIR"; then
  exit 1
fi

# Set after the lock is ours, so a run refused by the lock never touches the
# status file of the run that holds it.
STATUS_FAIL_REASON=""
on_exit() {
  local rc=$?
  trap - EXIT
  if [ "$rc" -ne 0 ]; then
    status_fail_if_open "Update failed (exit status $rc)"
  fi
  staging_on_exit "$rc"
  release_update_lock
  exit "$rc"
}
trap on_exit EXIT

if status_enabled; then
  # Cargo prints its N/M progress lines only when told to, even without a TTY.
  CARGO_TERM_PROGRESS_WHEN="${CARGO_TERM_PROGRESS_WHEN:-always}"
  CARGO_TERM_PROGRESS_WIDTH="${CARGO_TERM_PROGRESS_WIDTH:-100}"
  export CARGO_TERM_PROGRESS_WHEN CARGO_TERM_PROGRESS_WIDTH
fi

# Everything below runs inside a function. `git merge --ff-only` can rewrite
# THIS file while bash is still reading it (bash reads scripts incrementally),
# which would make the rest of the run execute garbage. Bash parses a whole
# function before running it, and the final call line is read in one piece.
main() {
  echo "== Updating Vox =="
  staging_begin_phase
  if [ "$STAGING_PHASE" = install ]; then
    # Install phase: no git, bun or cargo. Its own checks and cleanup live in
    # the library; the status file ends with done, or failed from the EXIT trap.
    staging_run_install || return $?
    return 0
  fi

  # ---- 0b. Target pin: validated before any git, bun or cargo command -------
  PIN_SHA=""
  if [ "${VOX_UPDATE_TARGET_SHA+set}" = "set" ]; then
    if ! status_hex_of_len "$VOX_UPDATE_TARGET_SHA" 40; then
      STATUS_FAIL_REASON="Target commit is not a valid 40-character SHA"
      echo "VOX_UPDATE_TARGET_SHA must be 40 lowercase hex characters -- nothing was changed." >&2
      exit 1
    fi
    PIN_SHA="$VOX_UPDATE_TARGET_SHA"
    status_set_target "$PIN_SHA" ""
  elif status_enabled; then
    status_set_target "" "$(git rev-parse --short=7 HEAD 2>/dev/null || true)"
  fi
  status_step preparing 0 "Checking the source tree" "Update could not start"

  # ---- 1. Refuse to clobber uncommitted work --------------------------------
  # Untracked files are deliberately ignored: they cannot be overwritten by a
  # fast-forward merge of tracked files, and a stray scratch file should not
  # block updates.
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
    STATUS_FAIL_REASON="Uncommitted changes to tracked files"
    echo "You have uncommitted changes to tracked files -- not updating on top of them." >&2
    echo "Commit, stash, or discard them first, then re-run this script." >&2
    git status --short --untracked-files=no >&2
    exit 1
  fi

  BRANCH="$(git branch --show-current)"
  if [ -z "$BRANCH" ]; then
    STATUS_FAIL_REASON="Not on a branch (detached HEAD)"
    echo "Not currently on a branch (detached HEAD) -- check out a branch first." >&2
    exit 1
  fi

  # ---- 2. Only update from the official origin ------------------------------
  ORIGIN_URL="$(git remote get-url origin 2>/dev/null || true)"
  if ! origin_allowed "$ORIGIN_URL"; then
    STATUS_FAIL_REASON="Origin is not the official Vox remote"
    echo "Refusing to update: remote 'origin' is '$ORIGIN_URL'," >&2
    echo "which is not the official Vox origin (github.com/iamshreeram/vox)." >&2
    echo "Forks may set VOX_UPDATE_ALLOW_ORIGIN to their exact origin URL." >&2
    exit 1
  fi

  # ---- 3. Source-update mode only builds main -------------------------------
  if [ "${VOX_SOURCE_UPDATE:-}" = "1" ] && [ "$BRANCH" != "main" ]; then
    STATUS_FAIL_REASON="Source updates build only the main branch"
    echo "VOX_SOURCE_UPDATE=1 requires the 'main' branch (currently on '$BRANCH')." >&2
    exit 1
  fi

  # ---- 4. Fast-forward, never merge -----------------------------------------
  status_step preparing 2 "Fetching the latest code" "Could not fetch from origin"
  if [ -n "$PIN_SHA" ]; then
    echo "Fetching origin, then moving '${BRANCH}' to the pinned commit ${PIN_SHA}..."
    git fetch origin
    if ! git cat-file -e "${PIN_SHA}^{commit}" 2>/dev/null; then
      STATUS_FAIL_REASON="Target commit is not reachable from origin"
      echo "Target commit ${PIN_SHA} is not reachable -- nothing was changed." >&2
      exit 1
    fi
    if ! git merge-base --is-ancestor HEAD "$PIN_SHA"; then
      STATUS_FAIL_REASON="Local branch is ahead of or diverged from the target"
      echo "Local HEAD is ahead of or diverged from the target ${PIN_SHA} -- nothing was changed." >&2
      echo "Resolve it by hand, then re-run this script." >&2
      exit 1
    fi
    if ! git merge --ff-only "$PIN_SHA"; then
      STATUS_FAIL_REASON="Could not fast-forward to the target"
      echo "Cannot fast-forward '${BRANCH}' to ${PIN_SHA}." >&2
      exit 1
    fi
    if [ "$(git rev-parse HEAD)" != "$PIN_SHA" ]; then
      STATUS_FAIL_REASON="HEAD does not match the target"
      echo "HEAD is not the pinned target ${PIN_SHA} -- nothing was built." >&2
      exit 1
    fi
  else
    echo "Fast-forwarding '${BRANCH}' to 'origin/${BRANCH}'..."
    git fetch origin
    if ! git merge --ff-only "origin/${BRANCH}"; then
      STATUS_FAIL_REASON="Local branch has diverged from origin"
      echo "Cannot fast-forward '${BRANCH}' to 'origin/${BRANCH}': the local branch has diverged." >&2
      echo "Nothing was changed. Resolve it by hand (for example, move your local commits" >&2
      echo "elsewhere and reset '${BRANCH}' to 'origin/${BRANCH}'), then re-run this script." >&2
      exit 1
    fi
  fi

  AFTER_REV="$(git rev-parse --short HEAD)"
  status_set_target "$(git rev-parse HEAD)" ""

  # ---- 5. Keep JS deps in sync, then build the release bundle ---------------
  echo "Rust toolchain: $(command -v rustc || echo 'rustc not found') ($(rustc --version 2>/dev/null || echo 'unknown version'))"

  status_step dependencies 4 "Installing JavaScript dependencies" "Installing JavaScript dependencies failed"
  echo "Installing JS dependencies (no-op if nothing changed)..."
  run_bun install
  status_step dependencies 10 "Dependencies are ready" "Installing JavaScript dependencies failed"

  APP_BUNDLE="src-tauri/target/release/bundle/macos/Vox.app"
  # Remove any previous bundle so a failed build cannot leave an old app behind.
  rm -rf "$APP_BUNDLE"

  export VOX_BUILD_COMMIT="$(git rev-parse HEAD)" VOX_BUILD_BRANCH="$BRANCH" VOX_SOURCE_UPDATES=1

  # --bundles app: the installer only needs Vox.app, so skip the DMG.
  echo "Building release bundle -- this takes a few minutes..."
  if status_enabled; then
    BUILD_START="$(date +%s)"
    BUILD_LOG="$TMP_BASE/vox-update-build.log"
    status_step compiling 10 "Compiling" "Build failed"
    BUILD_RC=0
    run_bun run tauri build --bundles app --config src-tauri/tauri.ci-unsigned.conf.json 2>&1 \
      | tee "$BUILD_LOG" | status_progress_consume || BUILD_RC=$?
    status_reload_percent
    if [ "$BUILD_RC" -ne 0 ]; then
      STATUS_FAIL_REASON="Build failed (exit status $BUILD_RC)"
      echo "Build failed with exit status $BUILD_RC -- not installing. Log: $BUILD_LOG" >&2
      exit "$BUILD_RC"
    fi
  else
    run_bun run tauri build --bundles app --config src-tauri/tauri.ci-unsigned.conf.json
  fi

  status_step bundling 88 "Checking the app bundle" "Build did not produce the app bundle"
  if [ ! -d "$APP_BUNDLE" ]; then
    echo "Build did not produce $APP_BUNDLE -- not installing." >&2
    exit 1
  fi
  if status_enabled; then
    status_save_build_seconds "$(($(date +%s) - BUILD_START))"
  fi

  # ---- 6. Install (handles stop/re-sign/restart) -----------------------------
  if [ "$STAGING_PHASE" = build ]; then
    # Build phase: stage the fresh app and stop at `waiting`. Nothing installs.
    if ! staging_finish_build "$APP_BUNDLE" "$(git rev-parse HEAD)"; then
      echo "Could not stage the built app -- not installing." >&2
      return 1
    fi
    echo ""
    echo "== Build staged for install: now on $AFTER_REV =="
    return 0
  fi

  status_step installing 92 "Installing Vox" "Install failed"
  ./scripts/install-macos.sh "$APP_BUNDLE"

  status_step restarting 97 "Finishing the update" "Update failed after the install"

  echo ""
  echo "== Update complete: now on $AFTER_REV =="
  # done is the very last status action: the install returned 0 and the lock
  # is released only after this function returns.
  status_write done 100 "Update complete" exact
}

main "$@"; exit $?
