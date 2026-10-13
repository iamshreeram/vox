#!/bin/bash
# Acceptance tests for scripts/lib/update-lock.sh (run with /bin/bash 3.2).
#
# Contract under test:
#   acquire_update_lock <lockdir>
#     - atomically mkdir <lockdir>; on success writes <lockdir>/owner (non-empty
#       token), sets UPDATE_LOCK_DIR and UPDATE_LOCK_TOKEN (== owner content),
#       returns 0.
#     - if <lockdir> already exists for ANY reason, never removes, renames or
#       modifies it; prints to stderr a message with the lock path and the text
#       "rm -rf" (manual recovery hint); returns non-zero.
#   release_update_lock
#     - removes UPDATE_LOCK_DIR only if <dir>/owner content == UPDATE_LOCK_TOKEN;
#       otherwise leaves it alone; always returns 0.
#
# Env:
#   ROOT_LIB_OVERRIDE  optional directory holding update-lock.sh (used to run
#                      this harness against a reference implementation).
#
# Every scenario runs in its own fresh mktemp -d sandbox with TMPDIR pointed
# inside it. Exit status is 1 if any scenario failed.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LIB_DIR="${ROOT_LIB_OVERRIDE:-$ROOT/scripts/lib}"
LIB_FILE="$LIB_DIR/update-lock.sh"

# Isolate from the user's git config and from any inherited update env.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
unset VOX_UPDATE_ALLOW_ORIGIN VOX_SOURCE_UPDATE VOX_SOURCE_UPDATES
unset VOX_BUILD_COMMIT VOX_BUILD_BRANCH FAKE_INSTALL_FAIL FAKE_BUILD_FAIL

SANDBOX_BASE="$(mktemp -d)" || { echo "cannot create sandbox base" >&2; exit 1; }
trap 'rm -rf "$SANDBOX_BASE"' EXIT

PASSED=0
FAILED=0
REASON_FILE=""

# ---- helpers (used inside scenario subshells) --------------------------------

# fail <reason...>: record the reason for the current scenario and abort it.
fail() {
  echo "$*" >>"$REASON_FILE"
  exit 1
}

# check <description> <command...>: run command; fail with description if non-zero.
check() {
  local desc="$1"
  shift
  if "$@"; then
    return 0
  fi
  fail "assertion failed: $desc"
}

inode_of() {
  ls -di "$1" | awk '{print $1}'
}

# load_lib: source the library under test, or fail clearly if it is missing.
load_lib() {
  if [ ! -f "$LIB_FILE" ]; then
    fail "library missing: $LIB_FILE (not implemented yet)"
  fi
  # shellcheck disable=SC1090
  . "$LIB_FILE"
  if ! type acquire_update_lock >/dev/null 2>&1; then
    fail "acquire_update_lock not defined by $LIB_FILE"
  fi
  if ! type release_update_lock >/dev/null 2>&1; then
    fail "release_update_lock not defined by $LIB_FILE"
  fi
}

# ---- scenarios (each receives its sandbox path as $1) ------------------------

scenario_fresh_acquire() {
  local sb="$1" lock="$sb/lock"
  load_lib
  acquire_update_lock "$lock" 2>"$sb/err"
  local rc=$?
  check "acquire returned 0 (got $rc)" [ "$rc" -eq 0 ]
  check "lock dir exists" [ -d "$lock" ]
  check "owner file exists and is non-empty" [ -s "$lock/owner" ]
  check "UPDATE_LOCK_TOKEN is non-empty" [ -n "${UPDATE_LOCK_TOKEN:-}" ]
  check "UPDATE_LOCK_DIR is set to lock path" [ "${UPDATE_LOCK_DIR:-}" = "$lock" ]
  local content
  content="$(cat "$lock/owner")"
  check "owner content equals UPDATE_LOCK_TOKEN" [ "$content" = "$UPDATE_LOCK_TOKEN" ]
}

scenario_second_acquire_refused() {
  local sb="$1" lock="$sb/lock"
  load_lib
  acquire_update_lock "$lock" >/dev/null 2>&1 || fail "first acquire failed"
  local before
  before="$(cat "$lock/owner")"
  acquire_update_lock "$lock" 2>"$sb/err"
  local rc=$?
  check "second acquire returned non-zero (got $rc)" [ "$rc" -ne 0 ]
  check "stderr contains lock path" grep -F -q -- "$lock" "$sb/err"
  check "stderr contains 'rm -rf'" grep -F -q -- "rm -rf" "$sb/err"
  check "owner file unchanged" [ "$(cat "$lock/owner")" = "$before" ]
  check "lock dir still exists" [ -d "$lock" ]
}

scenario_release_then_reacquire() {
  local sb="$1" lock="$sb/lock"
  load_lib
  acquire_update_lock "$lock" >/dev/null 2>&1 || fail "first acquire failed"
  release_update_lock
  local rc=$?
  check "release returned 0 (got $rc)" [ "$rc" -eq 0 ]
  check "lock dir removed after release" [ ! -e "$lock" ]
  acquire_update_lock "$lock" >/dev/null 2>&1
  rc=$?
  check "re-acquire after release succeeds (got $rc)" [ "$rc" -eq 0 ]
  check "lock dir exists after re-acquire" [ -d "$lock" ]
}

scenario_release_replaced_lock_untouched() {
  local sb="$1" lock="$sb/lock"
  load_lib
  acquire_update_lock "$lock" >/dev/null 2>&1 || fail "acquire failed"
  printf 'someone-else 12345\n' >"$lock/owner"
  release_update_lock
  local rc=$?
  check "release returned 0 (got $rc)" [ "$rc" -eq 0 ]
  check "foreign-owned lock dir left in place" [ -d "$lock" ]
  check "foreign owner file left untouched" \
    [ "$(cat "$lock/owner")" = "someone-else 12345" ]
}

scenario_release_nothing_acquired() {
  local sb="$1" other="$sb/unrelated"
  load_lib
  mkdir "$other" || fail "could not create unrelated dir"
  echo keep >"$other/data"
  unset UPDATE_LOCK_DIR UPDATE_LOCK_TOKEN
  release_update_lock
  local rc=$?
  check "release with nothing acquired returns 0 (got $rc)" [ "$rc" -eq 0 ]
  check "unrelated directory still exists" [ -d "$other" ]
  check "unrelated file still intact" [ "$(cat "$other/data")" = "keep" ]
}

scenario_stale_looking_lock_refused() {
  local sb="$1" lock="$sb/lock"
  load_lib
  mkdir "$lock" || fail "could not pre-create lock dir"
  printf '999999 bogus-lstart\n' >"$lock/owner"
  local ino_before
  ino_before="$(inode_of "$lock")"
  acquire_update_lock "$lock" 2>"$sb/err"
  local rc=$?
  check "acquire refused on stale-looking lock (got rc $rc)" [ "$rc" -ne 0 ]
  check "lock directory identity unchanged (same inode)" \
    [ "$(inode_of "$lock")" = "$ino_before" ]
  check "owner content unchanged" \
    [ "$(cat "$lock/owner")" = "999999 bogus-lstart" ]
  check "stderr contains 'rm -rf'" grep -F -q -- "rm -rf" "$sb/err"
}

scenario_ownerless_old_lock_refused() {
  local sb="$1" lock="$sb/lock"
  load_lib
  mkdir "$lock" || fail "could not pre-create lock dir"
  touch -t 200001010000 "$lock"
  local ino_before
  ino_before="$(inode_of "$lock")"
  acquire_update_lock "$lock" 2>"$sb/err"
  local rc=$?
  check "acquire refused on owner-less old lock (got rc $rc)" [ "$rc" -ne 0 ]
  check "lock directory identity unchanged (same inode)" \
    [ "$(inode_of "$lock")" = "$ino_before" ]
  check "lock directory still exists" [ -d "$lock" ]
  check "no owner file created in old lock" [ ! -e "$lock/owner" ]
  check "old mtime preserved (lock not touched)" \
    [ "$(stat -f %m "$lock")" -lt 978307200 ]
  check "stderr contains 'rm -rf'" grep -F -q -- "rm -rf" "$sb/err"
}

scenario_special_path_acquire_release() {
  local sb="$1" parent="$sb/a b'c"
  load_lib
  mkdir "$parent" || fail "could not create parent dir with space and quote"
  local lock="$parent/my lock'd"
  acquire_update_lock "$lock" >/dev/null 2>"$sb/err"
  local rc=$?
  check "acquire on special path returned 0 (got $rc)" [ "$rc" -eq 0 ]
  check "special-path lock dir exists" [ -d "$lock" ]
  check "special-path owner file non-empty" [ -s "$lock/owner" ]
  release_update_lock
  rc=$?
  check "release on special path returned 0 (got $rc)" [ "$rc" -eq 0 ]
  check "special-path lock dir removed" [ ! -e "$lock" ]
}

scenario_parallel_mutual_exclusion() {
  local sb="$1" lock="$sb/lock" log="$sb/log" i
  load_lib
  : >"$log"
  for i in 1 2 3 4 5 6 7 8; do
    (
      # Workers must not inherit the parent's EXIT trap (it would rm the sandbox).
      trap - EXIT
      if acquire_update_lock "$lock" >/dev/null 2>&1; then
        echo "enter $i" >>"$log"
        sleep 0.3
        echo "exit $i" >>"$log"
        release_update_lock
      fi
    ) &
  done
  wait

  local verdict
  verdict="$(awk '
    BEGIN { ok = 1; open = 0; n = 0; id = "" }
    /^enter / {
      if (open) ok = 0
      open = 1; n++; id = $2; next
    }
    /^exit / {
      if (!open || $2 != id) ok = 0
      open = 0; next
    }
    END {
      if (open) ok = 0
      if (ok && n > 0) print "OK " n; else print "BAD " n
    }' "$log")"
  case "$verdict" in
    OK\ *) ;;
    *) fail "log not strictly enter/exit alternating: verdict=$verdict; log: $(tr '\n' ' ' <"$log")" ;;
  esac
  check "at least one worker entered the critical section" [ -s "$log" ]
  check "lock dir gone after all workers finished" [ ! -e "$lock" ]
}

scenario_trap_release_normal_exit() {
  local sb="$1" lock="$sb/lock"
  load_lib
  (
    acquire_update_lock "$lock" >/dev/null 2>&1 || exit 9
    trap release_update_lock EXIT
    true
  )
  local rc=$?
  check "subshell exited 0 (got $rc)" [ "$rc" -eq 0 ]
  check "no lock left behind after normal exit" [ ! -e "$lock" ]
}

scenario_trap_release_explicit_exit() {
  local sb="$1" lock="$sb/lock"
  load_lib
  (
    acquire_update_lock "$lock" >/dev/null 2>&1 || exit 9
    trap release_update_lock EXIT
    exit 3
  )
  local rc=$?
  check "exit status 3 preserved (got $rc)" [ "$rc" -eq 3 ]
  check "no lock left behind after exit 3" [ ! -e "$lock" ]
}

# ---- runner -----------------------------------------------------------------

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

run_scenario "acquire on fresh path succeeds and writes owner token" scenario_fresh_acquire
run_scenario "second acquire refused, message has path and rm -rf, owner unchanged" scenario_second_acquire_refused
run_scenario "release removes dir; later acquire succeeds" scenario_release_then_reacquire
run_scenario "release leaves a lock replaced by another owner untouched" scenario_release_replaced_lock_untouched
run_scenario "release with nothing acquired returns 0 and keeps unrelated dir" scenario_release_nothing_acquired
run_scenario "stale-looking lock (dead pid owner) refused, inode and owner unchanged" scenario_stale_looking_lock_refused
run_scenario "owner-less old-mtime lock refused, unchanged" scenario_ownerless_old_lock_refused
run_scenario "lock path with spaces and single quote works" scenario_special_path_acquire_release
run_scenario "8 parallel workers: strict enter/exit alternation, lock gone" scenario_parallel_mutual_exclusion
run_scenario "trap release on normal subshell exit leaves no lock" scenario_trap_release_normal_exit
run_scenario "trap release on exit 3 leaves no lock and preserves status 3" scenario_trap_release_explicit_exit

echo "$PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
