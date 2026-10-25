#!/bin/bash
# Unit tests for scripts/dev-guard/lib.sh pure functions.
# Sourced by test-guard.sh (which provides helpers and DG_LIB_PATH).
# Every test begins with require_lib, so a missing lib.sh fails each test
# with "lib.sh missing" instead of crashing.

if [ -f "$DG_LIB_PATH" ]; then
  . "$DG_LIB_PATH"
fi

# _is_pos_int VALUE : true for a positive decimal integer.
_is_pos_int() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$1" -gt 0 ]
}

# _write_file PATH CONTENT : writes CONTENT followed by a newline.
_write_file() {
  printf '%s\n' "$2" >"$1"
}

# ---------------------------------------------------------------- dg_count

test_dg_count_real() {
  require_lib "dg_count: real ps gives a positive integer" || return 0
  local out rc
  out=$(unset DG_FAKE_COUNT DG_FAKE_PS_FAIL; dg_count); rc=$?
  assert_eq "dg_count: real ps exit code" "0" "$rc"
  if _is_pos_int "$out"; then
    _report pass "dg_count: real ps gives a positive integer"
  else
    _report fail "dg_count: real ps gives a positive integer" "got [$out]"
  fi
}

test_dg_count_fake_file() {
  require_lib "dg_count: fake count file replaces value" || return 0
  new_dg_home
  local f="$DG_HOME/count-ok" out rc
  _write_file "$f" "1500"
  out=$(DG_FAKE_COUNT="$f" dg_count); rc=$?
  assert_eq "dg_count: fake count file exit code" "0" "$rc"
  assert_eq "dg_count: fake count file value" "1500" "$out"
}

test_dg_count_fake_first_line_only() {
  require_lib "dg_count: only the first line of fake file is used" || return 0
  new_dg_home
  local f="$DG_HOME/count-two" out rc
  printf '42\nignored\n' >"$f"
  out=$(DG_FAKE_COUNT="$f" dg_count); rc=$?
  assert_eq "dg_count: first line of fake file used" "42" "$out"
  assert_eq "dg_count: first line of fake file exit code" "0" "$rc"
}

test_dg_count_fake_ignored_without_test_mode() {
  require_lib "dg_count: fake file ignored when DG_TEST is not 1" || return 0
  new_dg_home
  local f="$DG_HOME/count-999" out
  _write_file "$f" "999999"
  out=$(unset DG_FAKE_PS_FAIL; DG_TEST= DG_FAKE_COUNT="$f" dg_count)
  assert_not_contains "dg_count: fake value not used outside test mode" "$out" "999999"
}

test_dg_count_missing_fake_file() {
  require_lib "dg_count: missing fake file fails" || return 0
  new_dg_home
  local out rc
  out=$(DG_FAKE_COUNT="$DG_HOME/does-not-exist" dg_count); rc=$?
  assert_rc_nonzero "dg_count: missing fake file exit code non-zero" "$rc"
  assert_eq "dg_count: missing fake file prints nothing" "" "$out"
}

test_dg_count_garbled_fake_lines() {
  require_lib "dg_count: garbled fake lines fail" || return 0
  new_dg_home
  local f="$DG_HOME/count-bad" out rc v
  for v in "abc" "0" "-1" "12 13"; do
    _write_file "$f" "$v"
    out=$(DG_FAKE_COUNT="$f" dg_count); rc=$?
    assert_rc_nonzero "dg_count: garbled [$v] exit code non-zero" "$rc"
    assert_eq "dg_count: garbled [$v] prints nothing" "" "$out"
  done
  : >"$f"
  out=$(DG_FAKE_COUNT="$f" dg_count); rc=$?
  assert_rc_nonzero "dg_count: empty fake file exit code non-zero" "$rc"
  assert_eq "dg_count: empty fake file prints nothing" "" "$out"
}

test_dg_count_ps_failure() {
  require_lib "dg_count: injected ps failure fails" || return 0
  new_dg_home
  local out rc
  out=$(DG_FAKE_PS_FAIL=1 dg_count); rc=$?
  assert_rc_nonzero "dg_count: ps failure exit code non-zero" "$rc"
  assert_eq "dg_count: ps failure prints nothing" "" "$out"
}

# ---------------------------------------------------------------- dg_level

# _level_case NAME COUNT EXPECTED_OUT EXPECTED_RC [WARN HARD]
_level_case() {
  local name="$1" count="$2" exp_out="$3" exp_rc="$4" out rc
  if [ -n "$5" ]; then
    out=$(DG_WARN="$5" DG_HARD="$6" dg_level "$count"); rc=$?
  else
    out=$(unset DG_WARN DG_HARD; dg_level "$count"); rc=$?
  fi
  assert_eq "$name: output" "$exp_out" "$out"
  assert_eq "$name: exit code" "$exp_rc" "$rc"
}

test_dg_level_default_boundaries() {
  require_lib "dg_level: default boundaries" || return 0
  _level_case "dg_level 0" 0 ok 0
  _level_case "dg_level 1199" 1199 ok 0
  _level_case "dg_level 1200" 1200 warn 0
  _level_case "dg_level 1799" 1799 warn 0
  _level_case "dg_level 1800" 1800 hard 0
}

test_dg_level_overridden_thresholds() {
  require_lib "dg_level: overridden thresholds" || return 0
  _level_case "dg_level 9 with WARN=10 HARD=20" 9 ok 0 10 20
  _level_case "dg_level 10 with WARN=10 HARD=20" 10 warn 0 10 20
  _level_case "dg_level 19 with WARN=10 HARD=20" 19 warn 0 10 20
  _level_case "dg_level 20 with WARN=10 HARD=20" 20 hard 0 10 20
}

test_dg_level_garbled_input() {
  require_lib "dg_level: garbled input is unknown" || return 0
  local v out rc
  for v in "abc" "" "-1" "12 13"; do
    out=$(unset DG_WARN DG_HARD; dg_level "$v"); rc=$?
    assert_eq "dg_level garbled [$v]: output" "unknown" "$out"
    assert_eq "dg_level garbled [$v]: exit code" "1" "$rc"
  done
}

test_dg_level_code() {
  require_lib "dg_level_code: maps levels to codes" || return 0
  assert_eq "dg_level_code ok" "0" "$(dg_level_code ok)"
  assert_eq "dg_level_code warn" "10" "$(dg_level_code warn)"
  assert_eq "dg_level_code hard" "20" "$(dg_level_code hard)"
  assert_eq "dg_level_code unknown" "30" "$(dg_level_code unknown)"
}

# ------------------------------------------------------- dg_ulimit_value

# _make_sysctl_stub DIR BODY : creates DIR/sysctl running BODY.
_make_sysctl_stub() {
  mkdir -p "$1"
  printf '#!/bin/bash\n%s\n' "$2" >"$1/sysctl"
  chmod 755 "$1/sysctl"
}

# _ulimit_case NAME EXP_OUT EXP_RC STUB_DIR|- COUNT HEADROOM RESERVE SOFT
# STUB_DIR "-" means the real PATH is used unchanged.
_ulimit_case() {
  local name="$1" exp_out="$2" exp_rc="$3" dir="$4" out rc
  shift 4
  if [ "$dir" = "-" ]; then
    out=$(dg_ulimit_value "$@"); rc=$?
  else
    out=$(PATH="$dir:$PATH" dg_ulimit_value "$@"); rc=$?
  fi
  assert_eq "$name: exit code" "$exp_rc" "$rc"
  assert_eq "$name: output" "$exp_out" "$out"
}

test_dg_ulimit_value_table() {
  require_lib "dg_ulimit_value: numeric table" || return 0
  _ulimit_case "ulimit normal" "600" 0 - 100 500 100 2666
  _ulimit_case "ulimit clamped by SOFT" "1200" 0 - 1000 500 100 1200
  _ulimit_case "ulimit exact RESERVE boundary ok" "1100" 0 - 1000 100 100 1100
  _ulimit_case "ulimit one below RESERVE refused" "" 75 - 1000 100 100 1099
  _ulimit_case "ulimit SOFT below COUNT refused" "" 75 - 1000 500 100 900
  _ulimit_case "ulimit zero count refused" "" 75 - 0 500 100 2666
  _ulimit_case "ulimit zero headroom refused" "" 75 - 1000 0 100 2666
  _ulimit_case "ulimit negative count rejected" "" 64 - -5 500 100 2666
  _ulimit_case "ulimit nonnumeric count rejected" "" 64 - abc 500 100 2666
  _ulimit_case "ulimit nonnumeric headroom rejected" "" 64 - 100 x 100 2666
  _ulimit_case "ulimit nonnumeric soft rejected" "" 64 - 100 500 100 abc
  _ulimit_case "ulimit 10-digit overflow rejected" "" 64 - 1234567890 500 100 2666
  _ulimit_case "ulimit empty count rejected" "" 64 - "" 500 100 2666
  _ulimit_case "ulimit no arguments rejected" "" 64 -
}

test_dg_ulimit_value_unlimited() {
  require_lib "dg_ulimit_value: unlimited uses sysctl" || return 0
  new_dg_home
  _make_sysctl_stub "$DG_HOME/stub-2666" 'echo 2666'
  _make_sysctl_stub "$DG_HOME/stub-fail" 'exit 1'
  _make_sysctl_stub "$DG_HOME/stub-garbage" 'echo abc'
  _ulimit_case "ulimit unlimited with sysctl 2666" "600" 0 \
    "$DG_HOME/stub-2666" 100 500 100 unlimited
  _ulimit_case "ulimit unlimited clamped to sysctl 2666 refused" "" 75 \
    "$DG_HOME/stub-2666" 2600 500 100 unlimited
  _ulimit_case "ulimit unlimited with failing sysctl refused" "" 75 \
    "$DG_HOME/stub-fail" 100 500 100 unlimited
  _ulimit_case "ulimit unlimited with garbage sysctl refused" "" 75 \
    "$DG_HOME/stub-garbage" 100 500 100 unlimited
}

# ------------------------------------------------------ dg_validate_name

test_dg_validate_name_accepts() {
  require_lib "dg_validate_name: accepted names" || return 0
  local long
  long=$(printf '%032d' 0 | tr 0 a)
  assert_zero "dg_validate_name accepts [abc]" dg_validate_name abc
  assert_zero "dg_validate_name accepts [a-b_9]" dg_validate_name a-b_9
  assert_zero "dg_validate_name accepts single char" dg_validate_name A
  assert_zero "dg_validate_name accepts 32 chars" dg_validate_name "$long"
}

test_dg_validate_name_rejects() {
  require_lib "dg_validate_name: rejected names" || return 0
  local long nl uni
  long=$(printf '%033d' 0 | tr 0 a)
  nl=$(printf 'a\nb')
  uni=$(printf 'caf\303\251')
  assert_nonzero "dg_validate_name rejects empty" dg_validate_name ""
  assert_nonzero "dg_validate_name rejects 33 chars" dg_validate_name "$long"
  assert_nonzero "dg_validate_name rejects slash" dg_validate_name "a/b"
  assert_nonzero "dg_validate_name rejects dot-dot" dg_validate_name ".."
  assert_nonzero "dg_validate_name rejects dot" dg_validate_name "a.b"
  assert_nonzero "dg_validate_name rejects space" dg_validate_name "a b"
  assert_nonzero "dg_validate_name rejects newline" dg_validate_name "$nl"
  assert_nonzero "dg_validate_name rejects unicode" dg_validate_name "$uni"
}

# ------------------------------------------------------- dg_home_check

test_dg_home_check() {
  require_lib "dg_home_check: directory modes and links" || return 0
  new_dg_home
  local base="$DG_HOME/homes"
  mkdir -p "$base/good" "$base/open"
  chmod 700 "$base/good"
  chmod 755 "$base/open"
  ln -s "$base/good" "$base/link"
  assert_zero "dg_home_check accepts real 0700 dir" dg_home_check "$base/good"
  assert_nonzero "dg_home_check refuses 0755 dir" dg_home_check "$base/open"
  assert_nonzero "dg_home_check refuses symlink to 0700 dir" dg_home_check "$base/link"
  assert_nonzero "dg_home_check refuses missing dir" dg_home_check "$base/missing"
}

# ------------------------------------------------------------ run them

test_dg_count_real
test_dg_count_fake_file
test_dg_count_fake_first_line_only
test_dg_count_fake_ignored_without_test_mode
test_dg_count_missing_fake_file
test_dg_count_garbled_fake_lines
test_dg_count_ps_failure
test_dg_level_default_boundaries
test_dg_level_overridden_thresholds
test_dg_level_garbled_input
test_dg_level_code
test_dg_ulimit_value_table
test_dg_ulimit_value_unlimited
test_dg_validate_name_accepts
test_dg_validate_name_rejects
test_dg_home_check
