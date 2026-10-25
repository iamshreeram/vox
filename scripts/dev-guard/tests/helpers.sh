#!/bin/bash
# Shared assertion helpers for the dev-guard shell tests.
# Sourced by test-guard.sh. Bash 3.2 compatible (macOS /bin/bash).
# Output is TAP-like: "ok N - name" / "not ok N - name" plus a detail line.

PASS_COUNT=0
FAIL_COUNT=0
TEST_NUM=0
DG_TMP_LIST=""

# _report pass|fail NAME [DETAIL]
_report() {
  TEST_NUM=$((TEST_NUM + 1))
  if [ "$1" = "pass" ]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    echo "ok $TEST_NUM - $2"
  else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo "not ok $TEST_NUM - $2"
    if [ -n "$3" ]; then
      echo "#   $3"
    fi
  fi
}

# assert_eq NAME EXPECTED ACTUAL
assert_eq() {
  if [ "$2" = "$3" ]; then
    _report pass "$1"
  else
    _report fail "$1" "expected [$2] got [$3]"
  fi
}

# assert_zero NAME CMD...   (command must exit 0; its output is discarded)
assert_zero() {
  local name="$1" rc
  shift
  "$@" >/dev/null 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    _report pass "$name"
  else
    _report fail "$name" "expected exit 0 got $rc"
  fi
}

# assert_nonzero NAME CMD...  (command must exit non-zero; output discarded)
assert_nonzero() {
  local name="$1" rc
  shift
  "$@" >/dev/null 2>&1
  rc=$?
  if [ "$rc" -ne 0 ]; then
    _report pass "$name"
  else
    _report fail "$name" "expected non-zero exit got 0"
  fi
}

# assert_rc_nonzero NAME RC  (checks an already captured exit status)
assert_rc_nonzero() {
  if [ "$2" -ne 0 ]; then
    _report pass "$1"
  else
    _report fail "$1" "expected non-zero exit got 0"
  fi
}

# assert_exit NAME EXPECTED_CODE CMD...
assert_exit() {
  local name="$1" expected="$2" rc
  shift 2
  "$@" >/dev/null 2>&1
  rc=$?
  assert_eq "$name" "$expected" "$rc"
}

# assert_contains NAME HAYSTACK NEEDLE
assert_contains() {
  case "$2" in
    *"$3"*) _report pass "$1" ;;
    *) _report fail "$1" "[$3] not found in [$2]" ;;
  esac
}

# assert_not_contains NAME HAYSTACK NEEDLE
assert_not_contains() {
  case "$2" in
    *"$3"*) _report fail "$1" "[$3] unexpectedly found in [$2]" ;;
    *) _report pass "$1" ;;
  esac
}

# assert_file_exists NAME PATH
assert_file_exists() {
  if [ -f "$2" ]; then
    _report pass "$1"
  else
    _report fail "$1" "no such file: $2"
  fi
}

# require_lib NAME : records a failure and returns 1 when lib.sh is absent.
require_lib() {
  if [ -f "$DG_LIB_PATH" ]; then
    return 0
  fi
  _report fail "$1" "lib.sh missing"
  return 1
}

# new_dg_home: private 0700 temp dir exported as DG_HOME, DG_TEST=1.
# Call it directly (not in a subshell) so the exports take effect.
new_dg_home() {
  DG_HOME=$(mktemp -d "${TMPDIR:-/tmp}/dg-test.XXXXXX") || return 1
  chmod 700 "$DG_HOME"
  DG_TMP_LIST="$DG_TMP_LIST $DG_HOME"
  export DG_HOME
  export DG_TEST=1
}

# cleanup_tmp: removes every temp dir registered by this run. Idempotent.
cleanup_tmp() {
  local d
  for d in $DG_TMP_LIST; do
    if [ -n "$d" ]; then
      rm -rf "$d"
    fi
  done
  DG_TMP_LIST=""
}

# EXIT hooks: every test file registers its cleanup with dg_on_exit instead of
# calling trap itself, so no file overwrites another's trap. Hooks run in
# registration order; cleanup_tmp always runs last.
DG_EXIT_HOOKS=()

# dg_on_exit CMD : registers CMD (a shell command string) for the EXIT trap.
dg_on_exit() {
  DG_EXIT_HOOKS[${#DG_EXIT_HOOKS[@]}]="$1"
}

# dg_run_exit : the single EXIT trap body.
dg_run_exit() {
  local i=0
  while [ "$i" -lt "${#DG_EXIT_HOOKS[@]}" ]; do
    eval "${DG_EXIT_HOOKS[$i]}"
    i=$((i + 1))
  done
  cleanup_tmp
}
trap dg_run_exit EXIT

# finish: prints the summary; exit 1 on any failure.
finish() {
  echo "# passed $PASS_COUNT failed $FAIL_COUNT"
  if [ "$FAIL_COUNT" -ne 0 ]; then
    exit 1
  fi
  exit 0
}
