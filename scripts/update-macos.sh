#!/usr/bin/env bash
# One command to go from "whatever's currently checked out" to "latest code,
# rebuilt, installed, and running" -- chains together the steps a human would
# otherwise run by hand after every merge:
#
#   git pull  ->  bun install  ->  bun run tauri build  ->  install-macos.sh
#
# install-macos.sh already knows how to auto-restart the app if it was
# running before (see its own header comment), so this script doesn't need
# to duplicate that -- it just hands off the freshly built bundle to it.
#
# Usage:
#   ./scripts/update-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== Updating Vox =="

# ---- 1. Refuse to clobber uncommitted work --------------------------------
if [ -n "$(git status --porcelain)" ]; then
  echo "You have uncommitted changes -- not pulling on top of them." >&2
  echo "Commit, stash, or discard them first, then re-run this script." >&2
  git status --short >&2
  exit 1
fi

BRANCH="$(git branch --show-current)"
if [ -z "$BRANCH" ]; then
  echo "Not currently on a branch (detached HEAD) -- check out a branch first." >&2
  exit 1
fi

# ---- 2. Pull the latest for whatever branch is checked out ----------------
echo "Pulling latest '${BRANCH}'..."
git fetch origin
if ! git merge --ff-only "origin/${BRANCH}" 2>/dev/null; then
  echo "  '${BRANCH}' has diverged from 'origin/${BRANCH}' -- merging instead of fast-forwarding."
  git merge "origin/${BRANCH}" --no-edit
fi

AFTER_REV="$(git rev-parse --short HEAD)"

# ---- 3. Keep JS deps in sync, then build the release bundle ---------------
echo "Installing JS dependencies (no-op if nothing changed)..."
bun install

echo "Building release bundle -- this takes a few minutes..."
bun run tauri build || true
# `bun run tauri build` exits non-zero if the (separate, optional) signed
# updater-manifest step fails -- expected and harmless here, since no
# TAURI_SIGNING_PRIVATE_KEY is configured for local dev builds. The actual
# app bundle below is what matters; confirm it exists rather than trusting
# the overall exit code.
APP_BUNDLE="src-tauri/target/release/bundle/macos/Vox.app"
if [ ! -d "$APP_BUNDLE" ]; then
  echo "Build did not produce $APP_BUNDLE -- something actually failed, not just the optional updater-signing step." >&2
  exit 1
fi

# ---- 4. Install (handles stop/re-sign/restart) -----------------------------
./scripts/install-macos.sh "$APP_BUNDLE"

echo ""
echo "== Update complete: now on $AFTER_REV =="
