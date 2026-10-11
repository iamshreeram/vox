#!/bin/bash
# Acceptance tests for scripts/update-macos.sh (with scripts/lib/update-lock.sh
# and scripts/lib/origin-check.sh). Run with /bin/bash 3.2:
#   bash scripts/test-update-macos.sh
#
# Each scenario gets its own sandbox (mktemp -d) containing:
#   origin.git  bare origin, branch main
#   checkout    clone of origin (branch main tracks origin/main); receives the
#               REAL scripts/update-macos.sh and scripts/lib/ from this repo and
#               a STUB scripts/install-macos.sh
#   pusher      throwaway clone used to advance origin
#   stubs/      stub `bun` on PATH
#   tmp/        TMPDIR for the script under test (lock lives here, never /tmp)
#
# The real /tmp/vox-update.lock, ~/Library, /Applications and the user's git
# config are never touched (TMPDIR is redirected, GIT_CONFIG_* are /dev/null).
#
# Env:
#   ROOT_LIB_OVERRIDE  optional directory holding origin-check.sh (used to run
#                      the origin table against a reference implementation).

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB_DIR="${ROOT_LIB_OVERRIDE:-$ROOT/scripts/lib}"
ORIGIN_LIB="$LIB_DIR/origin-check.sh"

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
unset VOX_UPDATE_ALLOW_ORIGIN VOX_SOURCE_UPDATE VOX_SOURCE_UPDATES
unset VOX_BUILD_COMMIT VOX_BUILD_BRANCH FAKE_INSTALL_FAIL FAKE_BUILD_FAIL

BUNDLE_REL="src-tauri/target/release/bundle/macos/Vox.app"
CONFIG_REL="src-tauri/tauri.ci-unsigned.conf.json"

SANDBOX_BASE="$(mktemp -d)" || { echo "cannot create sandbox base" >&2; exit 1; }
trap 'rm -rf "$SANDBOX_BASE"' EXIT

PASSED=0
FAILED=0
REASON_FILE=""

# Per-scenario globals (set by setup_env inside each scenario subshell).
SB=""
ORIGIN=""
CHECKOUT=""
PUSHER=""
STUBDIR=""
STUB_LOG=""
RUN_OUT=""
RUN_RC=""

# ---- helpers -----------------------------------------------------------------

fail() {
  echo "$*" >>"$REASON_FILE"
  exit 1
}

check() {
  local desc="$1"
  shift
  if "$@"; then
    return 0
  fi
  fail "assertion failed: $desc"
}

# gitc: git with a fixed identity and no signing; never reads user config.
gitc() {
  git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false "$@"
}

inode_of() {
  ls -di "$1" | awk '{print $1}'
}

log_has() {
  [ -f "$STUB_LOG" ] && grep -F -q -- "$1" "$STUB_LOG"
}

out_has() {
  [ -f "$RUN_OUT" ] && grep -F -q -- "$1" "$RUN_OUT"
}

# count_log <pattern>: number of matching stub-log lines.
count_log() {
  if [ -f "$STUB_LOG" ]; then
    grep -F -c -- "$1" "$STUB_LOG"
  else
    echo 0
  fi
}

# setup_env <sandbox>: build origin, seed, checkout, pusher and stubs.
setup_env() {
  SB="$1"
  ORIGIN="$SB/origin.git"
  local seed="$SB/seed"
  CHECKOUT="$SB/checkout"
  PUSHER="$SB/pusher"
  STUBDIR="$SB/stubs"
  STUB_LOG="$SB/stub.log"
  RUN_OUT="$SB/run.out"
  mkdir -p "$SB/tmp" "$STUBDIR"

  git init -q --bare "$ORIGIN" || return 1
  git --git-dir="$ORIGIN" symbolic-ref HEAD refs/heads/main || return 1

  git init -q "$seed" || return 1
  git -C "$seed" symbolic-ref HEAD refs/heads/main || return 1
  mkdir -p "$seed/scripts"
  cp "$ROOT/scripts/update-macos.sh" "$seed/scripts/update-macos.sh" || return 1
  chmod +x "$seed/scripts/update-macos.sh"
  if [ -d "$ROOT/scripts/lib" ]; then
    cp -R "$ROOT/scripts/lib" "$seed/scripts/lib" || return 1
  fi
  cat >"$seed/scripts/install-macos.sh" <<'EOF'
#!/bin/bash
# STUB install-macos.sh for the update-macos harness.
echo "install-ran $1" >>"$STUB_LOG"
exit 0
EOF
  chmod +x "$seed/scripts/install-macos.sh"
  echo "seed readme" >"$seed/README.txt"

  gitc -C "$seed" add -A || return 1
  gitc -C "$seed" commit -q -m "seed" || return 1
  git -C "$seed" remote add origin "$ORIGIN" || return 1
  git -C "$seed" push -q origin main || return 1

  git clone -q "$ORIGIN" "$CHECKOUT" || return 1

  cat >"$STUBDIR/bun" <<'EOF'
#!/bin/bash
# STUB bun for the update-macos harness.
echo "bun $*" >>"$STUB_LOG"
if [ "$1" = "install" ]; then
  if [ "${FAKE_INSTALL_FAIL:-0}" = "1" ]; then exit 1; fi
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "tauri" ] && [ "$3" = "build" ]; then
  shift 3
  echo "tauri-build $*" >>"$STUB_LOG"
  echo "env VOX_BUILD_COMMIT=$VOX_BUILD_COMMIT VOX_BUILD_BRANCH=$VOX_BUILD_BRANCH VOX_SOURCE_UPDATES=$VOX_SOURCE_UPDATES VOX_SOURCE_UPDATE=$VOX_SOURCE_UPDATE" >>"$STUB_LOG"
  if [ "${FAKE_BUILD_FAIL:-0}" = "1" ]; then exit 1; fi
  mkdir -p src-tauri/target/release/bundle/macos/Vox.app
  echo dummy >src-tauri/target/release/bundle/macos/Vox.app/dummy
  exit 0
fi
echo "bun: unexpected args: $*" >>"$STUB_LOG"
exit 2
EOF
  chmod +x "$STUBDIR/bun"
  return 0
}

# advance_origin: push one new commit to origin main from the pusher clone.
# Sets ORIGIN_NEW_SHA to the new origin HEAD.
advance_origin() {
  rm -rf "$PUSHER"
  git clone -q "$ORIGIN" "$PUSHER" || fail "could not clone pusher"
  echo "upstream change" >"$PUSHER/upstream.txt"
  gitc -C "$PUSHER" add upstream.txt || fail "pusher add failed"
  gitc -C "$PUSHER" commit -q -m "upstream change" || fail "pusher commit failed"
  git -C "$PUSHER" push -q origin main || fail "pusher push failed"
  ORIGIN_NEW_SHA="$(git -C "$ORIGIN" rev-parse main)"
}

# run_update: run the script under test in the checkout with stubs on PATH.
# Environment variables (VOX_UPDATE_ALLOW_ORIGIN, VOX_SOURCE_UPDATE, FAKE_*)
# are inherited from the scenario subshell.
run_update() {
  (
    cd "$CHECKOUT" &&
      PATH="$STUBDIR:$PATH" TMPDIR="$SB/tmp" STUB_LOG="$STUB_LOG" \
        bash scripts/update-macos.sh
  ) >"$RUN_OUT" 2>&1
  RUN_RC=$?
}

lock_path() {
  echo "$SB/tmp/vox-update.lock"
}

# assert_refused_before_build: non-zero exit, no build and no install.
assert_refused_before_build() {
  check "exit non-zero (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "no tauri-build logged" bash -c '! { [ -f "$1" ] && grep -F -q -- "tauri-build" "$1"; }' _ "$STUB_LOG"
  check "no install-ran logged" bash -c '! { [ -f "$1" ] && grep -F -q -- "install-ran" "$1"; }' _ "$STUB_LOG"
}

# ---- scenarios ---------------------------------------------------------------

scenario_success_new_origin_commit() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  advance_origin
  run_update
  check "exit 0 (got $RUN_RC; output: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "tauri build invoked with unsigned config" log_has "tauri-build --bundles app --config $CONFIG_REL"
  check "VOX_BUILD_COMMIT equals new origin HEAD" log_has "VOX_BUILD_COMMIT=$ORIGIN_NEW_SHA"
  check "VOX_BUILD_BRANCH=main" log_has "VOX_BUILD_BRANCH=main"
  check "VOX_SOURCE_UPDATES=1" log_has "VOX_SOURCE_UPDATES=1"
  check "install-ran with bundle path" log_has "install-ran $BUNDLE_REL"
  check "local HEAD equals origin HEAD" \
    [ "$(git -C "$CHECKOUT" rev-parse HEAD)" = "$ORIGIN_NEW_SHA" ]
  check "lock dir absent afterwards" [ ! -e "$(lock_path)" ]
}

scenario_build_fails_stale_bundle_removed() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  mkdir -p "$CHECKOUT/$BUNDLE_REL"
  echo stale >"$CHECKOUT/$BUNDLE_REL/stale.txt"
  export FAKE_BUILD_FAIL=1
  run_update
  check "exit non-zero on build failure (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "build was attempted" log_has "tauri-build"
  check "install NOT run after build failure" bash -c '! { [ -f "$1" ] && grep -F -q -- "install-ran" "$1"; }' _ "$STUB_LOG"
  check "stale bundle no longer exists" [ ! -e "$CHECKOUT/$BUNDLE_REL" ]
  check "lock released" [ ! -e "$(lock_path)" ]
}

scenario_diverged_refused_ff_only() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  echo "local work" >"$CHECKOUT/local.txt"
  gitc -C "$CHECKOUT" add local.txt || fail "local add failed"
  gitc -C "$CHECKOUT" commit -q -m "local unpushed" || fail "local commit failed"
  advance_origin
  local before
  before="$(git -C "$CHECKOUT" rev-parse HEAD)"
  run_update
  check "exit non-zero on diverged history (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "output mentions fast-forward" out_has "fast-forward"
  check "local HEAD unchanged" [ "$(git -C "$CHECKOUT" rev-parse HEAD)" = "$before" ]
  check "zero merge commits in HEAD" \
    [ "$(git -C "$CHECKOUT" rev-list --merges HEAD | wc -l | tr -d ' ')" -eq 0 ]
  check "no tauri-build or install" bash -c '! { [ -f "$1" ] && grep -E -q -- "tauri-build|install-ran" "$1"; }' _ "$STUB_LOG"
  check "lock released" [ ! -e "$(lock_path)" ]
}

scenario_tracked_modification_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  echo "local edit" >>"$CHECKOUT/README.txt"
  run_update
  assert_refused_before_build
  check "tracked modification still present" grep -F -q "local edit" "$CHECKOUT/README.txt"
}

scenario_untracked_only_proceeds() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  echo "scratch" >"$CHECKOUT/untracked-notes.txt"
  run_update
  check "exit 0 with only untracked file (got $RUN_RC; output: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "install ran" log_has "install-ran $BUNDLE_REL"
}

scenario_lock_present_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  mkdir "$(lock_path)" || fail "could not pre-create lock"
  local ino
  ino="$(inode_of "$(lock_path)")"
  run_update
  check "exit non-zero when lock present (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "lock dir untouched (same inode)" [ "$(inode_of "$(lock_path)")" = "$ino" ]
  check "lock dir still exists" [ -d "$(lock_path)" ]
  check "no tauri-build logged" bash -c '! { [ -f "$1" ] && grep -F -q -- "tauri-build" "$1"; }' _ "$STUB_LOG"
}

scenario_origin_not_allowed_unset() {
  setup_env "$1" || fail "sandbox setup failed"
  advance_origin
  run_update
  assert_refused_before_build
  check "output mentions origin" out_has "origin"
}

scenario_origin_not_allowed_different_value() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$SB/some-other-origin.git"
  advance_origin
  run_update
  assert_refused_before_build
  check "output mentions origin" out_has "origin"
}

scenario_source_update_non_main_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  gitc -C "$CHECKOUT" switch -q -c topic || fail "could not create topic"
  git -C "$CHECKOUT" push -q -u origin topic || fail "could not push topic"
  export VOX_SOURCE_UPDATE=1
  run_update
  assert_refused_before_build
}

scenario_source_update_main_succeeds() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_SOURCE_UPDATE=1
  run_update
  check "exit 0 on main with VOX_SOURCE_UPDATE=1 (got $RUN_RC; output: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "log shows VOX_SOURCE_UPDATE=1" log_has "VOX_SOURCE_UPDATE=1"
  check "install ran" log_has "install-ran $BUNDLE_REL"
}

scenario_detached_head_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  advance_origin
  git -C "$CHECKOUT" switch -q --detach HEAD || fail "could not detach HEAD"
  run_update
  assert_refused_before_build
}

scenario_already_up_to_date_builds() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  run_update
  check "exit 0 when up to date (got $RUN_RC; output: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "tauri build ran" log_has "tauri-build --bundles app --config $CONFIG_REL"
  check "install ran" log_has "install-ran $BUNDLE_REL"
}

scenario_bun_install_fails() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export FAKE_INSTALL_FAIL=1
  run_update
  check "exit non-zero when bun install fails (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "bun install was attempted" log_has "bun install"
  check "no tauri-build logged" bash -c '! { [ -f "$1" ] && grep -F -q -- "tauri-build" "$1"; }' _ "$STUB_LOG"
  check "no install-ran logged" bash -c '! { [ -f "$1" ] && grep -F -q -- "install-ran" "$1"; }' _ "$STUB_LOG"
  check "lock released" [ ! -e "$(lock_path)" ]
}

scenario_second_run_after_success() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  advance_origin
  run_update
  check "first run exit 0 (got $RUN_RC)" [ "$RUN_RC" -eq 0 ]
  run_update
  check "second run exit 0 (got $RUN_RC; output: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "two installs ran" [ "$(count_log "install-ran")" -eq 2 ]
  check "lock released after both runs" [ ! -e "$(lock_path)" ]
}

# ---- origin table (origin-check.sh) -----------------------------------------

# origin_call <url>: run origin_allowed in a fresh subshell with set -e active.
# Exit 0 = allowed, 1 = refused, 98/99 = function/library missing.
origin_call() {
  (
    set -e
    if [ ! -f "$ORIGIN_LIB" ]; then exit 99; fi
    . "$ORIGIN_LIB"
    if ! type origin_allowed >/dev/null 2>&1; then exit 98; fi
    if origin_allowed "$1"; then
      exit 0
    else
      exit 1
    fi
  )
}

# expect_origin <allow|refuse> <url> <env-mode>
#   env-mode: "unset"            -> VOX_UPDATE_ALLOW_ORIGIN unset
#             "set:<value>"      -> VOX_UPDATE_ALLOW_ORIGIN=<value> (may be empty)
# Records a mismatch in ORIGIN_FAILS. Runs in the CURRENT shell (no subshell),
# so the failure record is not lost.
expect_origin() {
  local want="$1" url="$2" mode="$3" rc shown
  shown="$(printf '%q' "$url")"
  if [ "$mode" = "unset" ]; then
    (
      unset VOX_UPDATE_ALLOW_ORIGIN
      origin_call "$url"
    )
  else
    local val="${mode#set:}"
    (
      export VOX_UPDATE_ALLOW_ORIGIN="$val"
      origin_call "$url"
    )
  fi
  rc=$?
  case "$rc" in
    98) ORIGIN_FAILS="$ORIGIN_FAILS [origin_allowed undefined in $ORIGIN_LIB]" ;;
    99) ORIGIN_FAILS="$ORIGIN_FAILS [missing library $ORIGIN_LIB]" ;;
    0)
      if [ "$want" = "refuse" ]; then
        ORIGIN_FAILS="$ORIGIN_FAILS [expected refuse ($mode) for $shown, got allow]"
      fi
      ;;
    1)
      if [ "$want" = "allow" ]; then
        ORIGIN_FAILS="$ORIGIN_FAILS [expected allow ($mode) for $shown, got refuse]"
      fi
      ;;
    *) ORIGIN_FAILS="$ORIGIN_FAILS [unexpected rc $rc for $shown]" ;;
  esac
}

scenario_origin_table() {
  ORIGIN_FAILS=""
  local u nl
  nl="$(printf 'https://github.com/iamshreeram/vox\n.')"
  nl="${nl%.}"

  # Default (VOX_UPDATE_ALLOW_ORIGIN unset): these must be allowed.
  for u in \
    "https://github.com/iamshreeram/vox" \
    "https://github.com/iamshreeram/vox.git" \
    "git@github.com:iamshreeram/vox" \
    "git@github.com:iamshreeram/vox.git" \
    "ssh://git@github.com/iamshreeram/vox" \
    "ssh://git@github.com/iamshreeram/vox.git"; do
    expect_origin allow "$u" unset
  done

  # Default: these must be refused.
  for u in \
    "" \
    "https://evilgithub.com/iamshreeram/vox" \
    "https://github.com.evil.com/iamshreeram/vox" \
    "https://user@github.com/iamshreeram/vox" \
    "https://user:pw@github.com/iamshreeram/vox.git" \
    "https://github.com/iamshreeram/vox/extra" \
    "https://github.com/iamshreeram/vox-evil" \
    "https://github.com/iamshreeram/vox/" \
    "https://github.com/someoneelse/vox" \
    "https://github.com/iamshreeram/other" \
    "http://github.com/iamshreeram/vox" \
    "git@github.com:iamshreeram/vox.git " \
    " https://github.com/iamshreeram/vox" \
    "$nl" \
    "https://github.com/IAMSHREERAM/vox" \
    "https://github.com/iamshreeram/vox.git.evil" \
    "file:///tmp/iamshreeram/vox"; do
    expect_origin refuse "$u" unset
  done

  # VOX_UPDATE_ALLOW_ORIGIN set to a local path: exact match only.
  expect_origin allow "/some/local/path" "set:/some/local/path"
  expect_origin refuse "/some/local/path2" "set:/some/local/path"
  expect_origin refuse "" "set:/some/local/path"

  # VOX_UPDATE_ALLOW_ORIGIN set to the empty string: empty URL refused.
  expect_origin refuse "" "set:"

  if [ -n "$ORIGIN_FAILS" ]; then
    fail "origin table mismatches:$ORIGIN_FAILS"
  fi
}

# ---- runner ------------------------------------------------------------------

run_scenario() {
  local name="$1" fn="$2" sb rc reason
  sb="$(mktemp -d "$SANDBOX_BASE/s.XXXXXX")" || {
    echo "FAIL: $name -- cannot create sandbox"
    FAILED=$((FAILED + 1))
    return
  }
  REASON_FILE="$sb.reason"
  (
    trap 'rm -rf "$sb"' EXIT
    export TMPDIR="$sb/tmp"
    mkdir -p "$sb/tmp"
    cd "$sb" || exit 1
    "$fn" "$sb"
  ) >"$sb.out" 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "PASS: $name"
    PASSED=$((PASSED + 1))
  else
    reason="$(tr '\n' ' ' <"$REASON_FILE" 2>/dev/null)"
    if [ -z "$reason" ]; then
      reason="exit status $rc; output: $(tr '\n' ' ' <"$sb.out")"
    fi
    echo "FAIL: $name -- $reason"
    FAILED=$((FAILED + 1))
  fi
  rm -f "$sb.reason" "$sb.out"
  rm -rf "$sb"
}

run_scenario "(1) success: new origin commit builds, installs, HEAD==origin, lock released" scenario_success_new_origin_commit
run_scenario "(2) build fails: stale bundle removed, no install, lock released" scenario_build_fails_stale_bundle_removed
run_scenario "(3) diverged history refused with fast-forward message, no merge commit" scenario_diverged_refused_ff_only
run_scenario "(4) tracked modification refused before build, edit preserved" scenario_tracked_modification_refused
run_scenario "(5) only untracked file present: proceeds (exit 0)" scenario_untracked_only_proceeds
run_scenario "(6) lock already present: refused, lock dir untouched" scenario_lock_present_refused
run_scenario "(7a) origin not allowed (unset): refused, mentions origin" scenario_origin_not_allowed_unset
run_scenario "(7b) origin not allowed (different value): refused" scenario_origin_not_allowed_different_value
run_scenario "(8a) VOX_SOURCE_UPDATE=1 on non-main branch: refused" scenario_source_update_non_main_refused
run_scenario "(8b) VOX_SOURCE_UPDATE=1 on main: succeeds, logs VOX_SOURCE_UPDATE=1" scenario_source_update_main_succeeds
run_scenario "(9) detached HEAD: refused" scenario_detached_head_refused
run_scenario "(10) already up to date: exit 0, still builds and installs" scenario_already_up_to_date_builds
run_scenario "(11) bun install fails: no build, no install, lock released" scenario_bun_install_fails
run_scenario "(12) second immediate run after success also succeeds" scenario_second_run_after_success
run_scenario "(13) origin table: origin_allowed allow/refuse matrix (set -e safe)" scenario_origin_table

echo "$PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
