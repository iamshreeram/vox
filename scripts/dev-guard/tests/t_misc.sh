#!/bin/bash
# Tests for scripts/dev-guard/cargo-guard.sh and bun-guard.sh (guard-run --lock,
# RUST_TEST_THREADS / CARGO_BUILD_JOBS defaults, argument pass-through, exit
# code, lock held while running and released after), the documentation and the
# worker brief (cooperative-scope sentence, recovery text, GUARD: line),
# bash-3.2 compatibility of every script, and a read -t FIFO smoke test.
# Sourced by test-guard.sh in sorted glob order (before t_own.sh, t_run.sh,
# t_watch.sh), so it defines its own helpers. Bash 3.2 compatible.
#
# Safety rules for this file:
#  - the cargo and bun stand-ins are local stub scripts that exit by themselves
#    (hold loop bounded to 10 s); only pids recorded in _WM_PIDS are killed;
#  - no pkill/killall; no real cargo or bun is ever run.

_WM_DG_DIR="$SCRIPT_DIR/.."
_WM_CARGO="$_WM_DG_DIR/cargo-guard.sh"
_WM_BUN="$_WM_DG_DIR/bun-guard.sh"
_WM_DOC="$SCRIPT_DIR/../../../docs/dev-guard.md"
_WM_BRIEF="$_WM_DG_DIR/BRIEF-TEMPLATE.md"
_WM_SCOPE="This is cooperative admission control and evidence collection, not a sandbox or an account-wide process governor."
_WM_FORBIDDEN='mapfile|declare -A|printf .%\(|wait -n|coproc|[$][{][A-Za-z_][A-Za-z0-9_]*,,'
_WM_PIDS=""

# _wm_record PID / _wm_kill_recorded : kill ONLY the pids this file recorded.
_wm_record() {
  _WM_PIDS="$_WM_PIDS $1"
}
_wm_kill_recorded() {
  local p
  for p in $_WM_PIDS; do
    if kill -0 "$p" 2>/dev/null; then
      kill -KILL "$p" 2>/dev/null
    fi
  done
  _WM_PIDS=""
}

# _wm_case NAME FN : runs one test function, then kills its recorded pids.
_wm_case() {
  echo "# case: $1"
  "$2"
  _wm_kill_recorded
  unset DG_WARN DG_HARD DG_FAKE_COUNT DG_STUB_HOLD DG_STUB_RC
}

# _wm_need PATH NAME : records a failure and returns 1 when a file is missing.
_wm_need() {
  if [ -f "$1" ]; then
    return 0
  fi
  _report fail "$2" "missing file: $1"
  return 1
}

# _wm_tool PATH : a stand-in cargo/bun. Prints STUB_STARTED, the value of
# RUST_TEST_THREADS and CARGO_BUILD_JOBS, and each argument on its own line;
# waits (bounded) while DG_STUB_HOLD exists; exits with DG_STUB_RC (default 0).
_wm_tool() {
  cat >"$1" <<'EOF'
#!/bin/bash
echo "STUB_STARTED"
echo "RTT=${RUST_TEST_THREADS-unset}"
echo "CBJ=${CARGO_BUILD_JOBS-unset}"
for a in "$@"; do
  echo "ARG[$a]"
done
i=0
while [ -n "$DG_STUB_HOLD" ] && [ -f "$DG_STUB_HOLD" ] && [ "$i" -lt 100 ]; do
  sleep 0.1
  i=$((i + 1))
done
exit "${DG_STUB_RC:-0}"
EOF
  chmod 700 "$1"
}

# _wm_setup : private DG_HOME, fake count 5 (level ok), stand-ins in DG_HOME/bin.
_wm_setup() {
  new_dg_home || return 1
  DG_WARN=1000
  DG_HARD=2000
  DG_FAKE_COUNT="$DG_HOME/fake-count"
  export DG_WARN DG_HARD DG_FAKE_COUNT
  printf '%s\n' 5 >"$DG_FAKE_COUNT"
  mkdir -p "$DG_HOME/bin" || return 1
  _wm_tool "$DG_HOME/bin/cargo" || return 1
  _wm_tool "$DG_HOME/bin/bun" || return 1
  return 0
}

# _wm_wait_started FILE : polls up to 10 s for STUB_STARTED in FILE.
_wm_wait_started() {
  local i=0
  while ! grep -q STUB_STARTED "$1" 2>/dev/null && [ "$i" -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
  done
}

test_misc_scripts_present() {
  local name f first
  for name in proc-guard.sh guard-run.sh cargo-guard.sh bun-guard.sh; do
    f="$_WM_DG_DIR/$name"
    assert_zero "$name exists and is executable" test -x "$f"
    first=$(head -n 1 "$f" 2>/dev/null)
    assert_eq "$name starts with #!/bin/bash" "#!/bin/bash" "$first"
  done
}

test_misc_shebang_and_exec() {
  local f base first
  for f in "$_WM_DG_DIR"/*.sh; do
    [ -f "$f" ] || continue
    base=$(basename "$f")
    first=$(head -n 1 "$f")
    assert_eq "$base starts with #!/bin/bash" "#!/bin/bash" "$first"
    if [ "$base" != "lib.sh" ]; then
      assert_zero "$base is executable" test -x "$f"
    fi
  done
}

test_misc_bash32() {
  local f base hit
  for f in "$_WM_DG_DIR"/*.sh; do
    [ -f "$f" ] || continue
    base=$(basename "$f")
    assert_zero "bash -n $base (/bin/bash 3.2)" /bin/bash -n "$f"
    hit=$(grep -E -n "$_WM_FORBIDDEN" "$f" 2>/dev/null | head -n 1)
    if [ -z "$hit" ]; then
      _report pass "no bash-4-only construct in $base"
    else
      _report fail "no bash-4-only construct in $base" "$hit"
    fi
  done
}

test_misc_read_t_smoke() {
  new_dg_home || return 0
  local fifo="$DG_HOME/smoke.fifo" out elapsed
  mkfifo "$fifo" || return 0
  # Child: a timed read with no data must time out (rc != 0) after >= 1 s;
  # then a line written to the same O_RDWR fifo must be read with rc 0.
  out=$(/bin/bash -c 'exec 3<>"$1"; s=$SECONDS; read -t 1 -u 3 x; rc=$?; e=$((SECONDS - s))
    echo "t_rc=$rc t_elapsed=$e"; echo "line-ok" >&3; read -t 1 -u 3 y; echo "w_rc=$? w_val=$y"' _ "$fifo")
  trc=$(printf '%s\n' "$out" | sed -n 's/.*t_rc=\([0-9]*\).*/\1/p')
  telapsed=$(printf '%s\n' "$out" | sed -n 's/.*t_elapsed=\([0-9]*\).*/\1/p')
  wrc=$(printf '%s\n' "$out" | sed -n 's/.*w_rc=\([0-9]*\).*/\1/p')
  wval=$(printf '%s\n' "$out" | sed -n 's/.*w_val=\(.*\)$/\1/p')
  if [ -n "$trc" ] && [ "$trc" -ne 0 ] 2>/dev/null; then
    _report pass "read -t on O_RDWR fifo: timeout returns non-zero after the interval (bash 3.2 returns 1)"
  else
    _report fail "read -t on O_RDWR fifo: timeout returns non-zero after the interval (bash 3.2 returns 1)" "out=[$out]"
  fi
  if [ -n "$telapsed" ] && [ "$telapsed" -ge 1 ] 2>/dev/null; then
    _report pass "read -t timeout blocks for >= 1 s"
  else
    _report fail "read -t timeout blocks for >= 1 s" "out=[$out]"
  fi
  assert_eq "read -t returns 0 immediately when a line is written" 0 "$wrc"
  assert_eq "read -t delivers the written line" "line-ok" "$wval"
}

test_misc_cargo_passthrough() {
  _wm_need "$_WM_CARGO" "cargo-guard: script present" || return 0
  _wm_setup || return 0
  local out rc
  env -u RUST_TEST_THREADS -u CARGO_BUILD_JOBS PATH="$DG_HOME/bin:$PATH" \
    "$_WM_CARGO" test "arg with spaces" --flag=x >"$DG_HOME/o.txt" 2>&1
  rc=$?
  out=$(cat "$DG_HOME/o.txt" 2>/dev/null)
  assert_eq "cargo-guard exit 0 on success" 0 "$rc"
  assert_contains "cargo-guard default RUST_TEST_THREADS=4" "$out" "RTT=4"
  assert_contains "cargo-guard default CARGO_BUILD_JOBS=4" "$out" "CBJ=4"
  assert_contains "cargo-guard keeps an argument with spaces intact" "$out" "ARG[arg with spaces]"
  assert_contains "cargo-guard passes flags through" "$out" "ARG[--flag=x]"
}

test_misc_cargo_exit_and_threads() {
  _wm_need "$_WM_CARGO" "cargo-guard: script present" || return 0
  _wm_setup || return 0
  local out rc
  DG_STUB_RC=7 PATH="$DG_HOME/bin:$PATH" "$_WM_CARGO" test >"$DG_HOME/o.txt" 2>&1
  rc=$?
  assert_eq "cargo-guard passes through exit code 7" 7 "$rc"
  RUST_TEST_THREADS=2 PATH="$DG_HOME/bin:$PATH" "$_WM_CARGO" test >"$DG_HOME/o.txt" 2>&1
  out=$(cat "$DG_HOME/o.txt" 2>/dev/null)
  assert_contains "user-provided RUST_TEST_THREADS=2 is respected" "$out" "RTT=2"
}

test_misc_cargo_lock() {
  _wm_need "$_WM_CARGO" "cargo-guard lock: script present" || return 0
  _wm_setup || return 0
  : >"$DG_HOME/hold"
  DG_STUB_HOLD="$DG_HOME/hold" PATH="$DG_HOME/bin:$PATH" "$_WM_CARGO" test \
    >"$DG_HOME/lock.out" 2>&1 &
  local bg=$!
  _wm_record "$bg"
  _wm_wait_started "$DG_HOME/lock.out"
  assert_zero "cargo lock dir exists while cargo runs" test -d "$DG_HOME/locks/cargo.lock"
  rm -f "$DG_HOME/hold"
  wait "$bg"
  assert_eq "cargo-guard lock run exit 0" 0 "$?"
  assert_zero "cargo lock released after the run" test ! -d "$DG_HOME/locks/cargo.lock"
}

test_misc_bun_passthrough() {
  _wm_need "$_WM_BUN" "bun-guard: script present" || return 0
  _wm_setup || return 0
  local out rc
  env -u RUST_TEST_THREADS -u CARGO_BUILD_JOBS PATH="$DG_HOME/bin:$PATH" \
    "$_WM_BUN" test "arg with spaces" >"$DG_HOME/o.txt" 2>&1
  rc=$?
  out=$(cat "$DG_HOME/o.txt" 2>/dev/null)
  assert_eq "bun-guard exit 0 on success" 0 "$rc"
  assert_contains "bun-guard keeps an argument with spaces intact" "$out" "ARG[arg with spaces]"
  DG_STUB_RC=7 PATH="$DG_HOME/bin:$PATH" "$_WM_BUN" test >"$DG_HOME/o.txt" 2>&1
  assert_eq "bun-guard passes through exit code 7" 7 "$?"
}

test_misc_bun_lock() {
  _wm_need "$_WM_BUN" "bun-guard lock: script present" || return 0
  _wm_setup || return 0
  : >"$DG_HOME/hold"
  DG_STUB_HOLD="$DG_HOME/hold" PATH="$DG_HOME/bin:$PATH" "$_WM_BUN" test \
    >"$DG_HOME/lock.out" 2>&1 &
  local bg=$!
  _wm_record "$bg"
  _wm_wait_started "$DG_HOME/lock.out"
  assert_zero "bun lock dir exists while bun runs" test -d "$DG_HOME/locks/bun.lock"
  rm -f "$DG_HOME/hold"
  wait "$bg"
  assert_eq "bun-guard lock run exit 0" 0 "$?"
  assert_zero "bun lock released after the run" test ! -d "$DG_HOME/locks/bun.lock"
}

test_misc_docs_scope_sentence() {
  assert_zero "docs/dev-guard.md exists" test -f "$_WM_DOC"
  assert_zero "docs/dev-guard.md has the exact cooperative-scope sentence" \
    grep -q -F "$_WM_SCOPE" "$_WM_DOC"
  assert_zero "BRIEF-TEMPLATE.md exists" test -f "$_WM_BRIEF"
  assert_zero "BRIEF-TEMPLATE.md has the exact cooperative-scope sentence" \
    grep -q -F "$_WM_SCOPE" "$_WM_BRIEF"
}

test_misc_docs_terms() {
  local w
  assert_zero "docs/dev-guard.md exists" test -f "$_WM_DOC"
  for w in 'proc-guard.sh watch start' 'guard-run.sh' 'cargo-guard.sh' 'ppid==1' 'DG_HOME'; do
    assert_zero "docs/dev-guard.md mentions: $w" grep -q -F "$w" "$_WM_DOC"
  done
  assert_zero "docs/dev-guard.md has the manual recovery text" \
    grep -q -i -E 'manual recovery|no guard process is running' "$_WM_DOC"
}

test_misc_brief_guard_line() {
  local guard_line
  assert_zero "BRIEF-TEMPLATE.md exists" test -f "$_WM_BRIEF"
  guard_line=$(grep -E '^GUARD:' "$_WM_BRIEF" 2>/dev/null)
  assert_zero "BRIEF-TEMPLATE.md has a line starting with GUARD:" test -n "$guard_line"
  assert_contains "GUARD line routes cargo through cargo-guard.sh" "$guard_line" "cargo-guard.sh"
  assert_contains "GUARD line routes bun through bun-guard.sh" "$guard_line" "bun-guard.sh"
  assert_contains "GUARD line routes everything else through guard-run.sh" "$guard_line" "guard-run.sh"
  assert_contains "GUARD line says never raise limits" "$guard_line" "raise limits"
  assert_contains "GUARD line says never kill by name" "$guard_line" "by name"
  assert_contains "GUARD line uses the word never" "$guard_line" "never"
}

_wm_case "misc: required scripts exist, are executable, start with #!/bin/bash" test_misc_scripts_present
_wm_case "misc: every dev-guard script has #!/bin/bash, lib.sh excluded from exec bit" test_misc_shebang_and_exec
_wm_case "misc: bash -n and no bash-4-only constructs (/bin/bash 3.2)" test_misc_bash32
_wm_case "misc: read -t on a fifo blocks and times out (bash 3.2 smoke)" test_misc_read_t_smoke
_wm_case "cargo-guard: pass-through args, default env, exit code 0" test_misc_cargo_passthrough
_wm_case "cargo-guard: exit code 7 passthrough, user RUST_TEST_THREADS=2" test_misc_cargo_exit_and_threads
_wm_case "cargo-guard: cargo lock held while running, released after" test_misc_cargo_lock
_wm_case "bun-guard: pass-through args and exit code" test_misc_bun_passthrough
_wm_case "bun-guard: bun lock held while running, released after" test_misc_bun_lock
_wm_case "docs: cooperative-scope sentence in docs and brief" test_misc_docs_scope_sentence
_wm_case "docs: dev-guard.md mentions commands, ppid==1, DG_HOME, recovery" test_misc_docs_terms
_wm_case "brief: GUARD: line routes via wrappers and forbids limit raises and kill-by-name" test_misc_brief_guard_line
