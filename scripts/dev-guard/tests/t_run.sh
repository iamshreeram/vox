#!/bin/bash
# Tests for scripts/dev-guard/guard-run.sh (DESIGN-GUARD.md Rev 4: R2'' launch
# handshake, R3' ulimit, R5'' owner records, R1' census wording, R4 unknown count,
# INTERFACE). Sourced by test-guard.sh after helpers.sh, t_lib.sh and t_own.sh.
# Bash 3.2 compatible (macOS /bin/bash).
#
# Safety rules for this file:
#  - every fixture process has a finite lifetime (<= 30 s);
#  - the EXIT trap kills ONLY pids recorded here (_RUN_PIDS); no pkill/killall;
#  - at most one guard-run supervisor is alive at a time (the lock tests run two,
#    sequentially in time overlap of ~3 s); fixtures are short;
#  - real process counts are never read: DG_TEST=1 + DG_FAKE_COUNT files are used;
#  - each test has a 40 s overall budget (_RUN_DEADLINE); every guard-run wait is
#    bounded by that budget and the supervisor pid is KILLed on overrun.
# Missing guard-run.sh fails each test immediately (no run is attempted).

RUN_GUARD="$SCRIPT_DIR/../guard-run.sh"
_RUN_PIDS=""
_RUN_PID=""
_RUN_W=""
_RUN_DEADLINE=0
_RUN_TAKEN=""
_RG_RC=""
_RG_HUNG=0

# _run_record PID : remembers a pid this test started (killed on exit if alive).
_run_record() {
  _RUN_PIDS="$_RUN_PIDS $1"
}

# _run_take_pid FILE : reads a pid written by a fixture, records it, sets _RUN_TAKEN.
_run_take_pid() {
  local pid=""
  if [ -s "$1" ]; then
    pid=$(cat "$1")
  fi
  case "$pid" in
    ''|*[!0-9]*) return 1 ;;
  esac
  _RUN_TAKEN="$pid"
  _run_record "$pid"
  return 0
}

# Only the pids recorded above (and the owner-test pids) are ever killed.
_run_kill_recorded() {
  local p
  for p in $_RUN_PIDS; do
    if kill -0 "$p" 2>/dev/null; then
      kill -KILL "$p" 2>/dev/null
    fi
  done
}

# EXIT hook (chained via dg_on_exit; cleanup_tmp runs afterwards in dg_run_exit).
_run_exit() {
  _run_kill_recorded
}
dg_on_exit '_run_exit'

# _run_fake_count N : the measured process count seen by guard-run (DG_TEST=1).
_run_fake_count() {
  printf '%s\n' "$1" > "$_RUN_W/fakecount"
  export DG_FAKE_COUNT="$_RUN_W/fakecount"
}

# _run_setup NAME : fresh DG_HOME, private work dir, default knobs, 40 s budget.
_run_setup() {
  _RUN_DEADLINE=$((SECONDS + 40))
  new_dg_home || return 1
  _RUN_W=$(mktemp -d "${TMPDIR:-/tmp}/dgrun.XXXXXX") || return 1
  chmod 700 "$_RUN_W"
  DG_TMP_LIST="$DG_TMP_LIST $_RUN_W"
  export DG_WARN=1200 DG_HARD=1800 DG_RESERVE=100 DG_LOCK_WAIT=20
  unset DG_FAKE_READY_MISMATCH DG_FAKE_PS_FAIL_CENSUS DG_FAKE_PS_FAIL
  unset DG_PAUSE_launch_before_ready DG_PAUSE_launch_before_go
  _run_fake_count 10
}

# _run_case NAME FUNC : missing guard-run.sh fails fast; otherwise set up and run.
_run_case() {
  if [ ! -f "$RUN_GUARD" ]; then
    _report fail "$1" "guard-run.sh missing: $RUN_GUARD"
    return 0
  fi
  if ! _run_setup "$1"; then
    _report fail "$1" "test setup failed"
    return 0
  fi
  "$2"
}

# _run_start OUT ERR ARGS... : starts guard-run in the background, sets _RUN_PID.
# Job control is switched on only for the fork so INT/QUIT keep their default
# disposition (non-interactive async jobs otherwise have SIGINT ignored).
_run_start() {
  local out="$1" err="$2"
  shift 2
  set -m
  /bin/bash "$RUN_GUARD" "$@" >"$out" 2>"$err" &
  _RUN_PID=$!
  set +m
  _run_record "$_RUN_PID"
}

# _run_idle SECONDS : waits with builtin timed reads on an idle O_RDWR fifo, so
# no helper process is started (no sleep).
_run_idle() {
  local i=0 junk=""
  if [ ! -p "$_RUN_W/idle" ] && ! mkfifo "$_RUN_W/idle"; then
    return 0
  fi
  if ! exec 4<>"$_RUN_W/idle"; then
    return 0
  fi
  while [ "$i" -lt "$1" ]; do
    read -r -t 1 -u 4 junk || true
    i=$((i + 1))
  done
  exec 4<&-
  return 0
}

# _run_wait LIMIT : waits for _RUN_PID (bounded by LIMIT and the test budget).
# Sets _RG_RC (exit status) and _RG_HUNG=1 when the watchdog had to KILL it.
# After a KILL the pid gets up to 5 s to die. If it does not, a failure is
# recorded and wait is skipped; the pid stays in _RUN_PIDS for the EXIT retry.
_run_wait() {
  local limit="$1" start="$SECONDS" left i=0
  _RG_HUNG=0
  left=$((_RUN_DEADLINE - SECONDS))
  if [ "$left" -lt "$limit" ]; then
    limit=$left
  fi
  while kill -0 "$_RUN_PID" 2>/dev/null; do
    if [ $((SECONDS - start)) -ge "$limit" ]; then
      _RG_HUNG=1
      kill -KILL "$_RUN_PID" 2>/dev/null
      break
    fi
    sleep 0.2
  done
  if [ "$_RG_HUNG" -eq 1 ]; then
    while kill -0 "$_RUN_PID" 2>/dev/null && [ "$i" -lt 5 ]; do
      _run_idle 1
      i=$((i + 1))
    done
    if kill -0 "$_RUN_PID" 2>/dev/null; then
      _report fail "guard-run did not die after KILL (pid $_RUN_PID)"
      _RG_RC=137
      return 0
    fi
  fi
  wait "$_RUN_PID" 2>/dev/null
  _RG_RC=$?
}

# _run_guard LIMIT OUT ERR ARGS... : start and wait in one step.
_run_guard() {
  local limit="$1" out="$2" err="$3"
  shift 3
  _run_start "$out" "$err" "$@"
  _run_wait "$limit"
}

# _run_assert_rc NAME EXPECTED : checks the last guard-run status.
_run_assert_rc() {
  if [ "$_RG_HUNG" -eq 1 ]; then
    _report fail "$1" "guard-run exceeded its time budget and was killed"
    return 0
  fi
  assert_eq "$1" "$2" "$_RG_RC"
}

# _run_assert_under NAME START LIMIT : elapsed seconds since START must be < LIMIT.
_run_assert_under() {
  local el=$((SECONDS - $2))
  if [ "$el" -lt "$3" ]; then
    _report pass "$1"
  else
    _report fail "$1" "took ${el}s (limit ${3}s)"
  fi
}

# _run_pid_dead NAME PID : PID must be gone (polls up to 3 s for reaping).
_run_pid_dead() {
  local i=0
  while kill -0 "$2" 2>/dev/null; do
    i=$((i + 1))
    if [ "$i" -ge 15 ]; then
      _report fail "$1" "pid $2 still alive"
      return 0
    fi
    sleep 0.2
  done
  _report pass "$1"
}

# _run_alive_list PID... : prints the pids still alive after a short reap grace.
_run_alive_list() {
  local tries=0 live p
  while :; do
    live=""
    for p in "$@"; do
      if kill -0 "$p" 2>/dev/null; then
        live="$live $p"
      fi
    done
    if [ -z "$live" ] || [ "$tries" -ge 15 ]; then
      break
    fi
    tries=$((tries + 1))
    sleep 0.2
  done
  echo "$live"
}

# _run_fake_perl NAME BODY : installs a fake perl (first on PATH for this test).
_run_fake_perl() {
  mkdir -p "$_RUN_W/fakebin"
  printf '#!/bin/bash\n%s\n' "$2" > "$_RUN_W/fakebin/perl"
  chmod 700 "$_RUN_W/fakebin/perl"
}

# ---------------------------------------------------------------- usage errors

_run_t_usage_no_sep() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" echo hi
  _run_assert_rc "run: usage: missing -- separator => 64" 64
}

_run_t_usage_no_cmd() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --
  _run_assert_rc "run: usage: no command after -- => 64" 64
}

_run_t_usage_timeout_abc() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --timeout abc -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: usage: --timeout abc => 64" 64
  assert_zero "run: usage: --timeout abc never ran command" test ! -e "$_RUN_W/marker"
}

_run_t_usage_timeout_neg() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --timeout -1 -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: usage: --timeout -1 => 64" 64
  assert_zero "run: usage: --timeout -1 never ran command" test ! -e "$_RUN_W/marker"
}

_run_t_usage_lock_name() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --lock ../x -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: usage: --lock ../x (bad name) => 64" 64
  assert_zero "run: usage: bad lock name never ran command" test ! -e "$_RUN_W/marker"
}

_run_t_usage_headroom() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --headroom 99999999999 -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: usage: --headroom 99999999999 => 64" 64
  assert_zero "run: usage: oversize headroom never ran command" test ! -e "$_RUN_W/marker"
}

# ------------------------------------------------------------- refusal (75)

# _run_refusal NAME VALUE : VALUE is a count, or MISSING (no fake count file).
_run_refusal() {
  if [ "$2" = "MISSING" ]; then
    export DG_FAKE_COUNT="$_RUN_W/no-such-count"
  else
    printf '%s\n' "$2" > "$_RUN_W/fakecount"
  fi
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "$1: refused => 75" 75
  assert_zero "$1: command not run" test ! -e "$_RUN_W/marker"
}

_run_t_refuse_warn() { _run_refusal "run: refuse at warn (1500, warn=1200)" 1500; }
_run_t_refuse_hard() { _run_refusal "run: refuse at hard (1900, hard=1800)" 1900; }
_run_t_refuse_garbled() { _run_refusal "run: refuse when count is unknown (garbled)" abc; }
_run_t_refuse_missing() { _run_refusal "run: refuse when count is unknown (missing file)" MISSING; }

# ------------------------------------------------------- exact passthrough

_run_t_status_cycle() {
  local i=0 c bad="" marker
  while [ "$i" -lt 12 ]; do
    case $((i % 4)) in
      0) c=0 ;;
      1) c=7 ;;
      2) c=126 ;;
      *) c=127 ;;
    esac
    marker="$_RUN_W/ran.$i"
    _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$marker'; exit $c"
    if [ "$_RG_HUNG" -eq 1 ] || [ "$_RG_RC" -ne "$c" ] || [ ! -e "$marker" ]; then
      bad="$bad iter$i:want$c:got$_RG_RC"
    fi
    i=$((i + 1))
  done
  assert_eq "run: 12 runs cycling 0/7/126/127 pass status through (command ran each time)" "" "$bad"
}

_run_t_passthrough() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c 'echo OUT-PASS; echo ERR-PASS >&2; exit 0'
  _run_assert_rc "run: passthrough rc 0" 0
  assert_eq "run: stdout passed through unchanged" "OUT-PASS" "$(cat "$_RUN_W/o")"
  assert_contains "run: stderr passed through" "$(cat "$_RUN_W/e")" "ERR-PASS"
  assert_not_contains "run: stdout carries no stderr text" "$(cat "$_RUN_W/o")" "ERR-PASS"
}

# ------------------------------------------------------- timeout and cleanup

_run_t_timeout_group() {
  local start=$SECONDS
  _run_guard 15 "$_RUN_W/o" "$_RUN_W/e" --timeout 2 -- /bin/sh -c "sleep 25 & echo \$! > '$_RUN_W/gpid'; wait"
  _run_assert_rc "run: timeout => 124" 124
  _run_assert_under "run: timeout test finished in < 15 s" "$start" 15
  if _run_take_pid "$_RUN_W/gpid"; then
    _run_pid_dead "run: same-group grandchild killed on timeout" "$_RUN_TAKEN"
  else
    _report fail "run: same-group grandchild killed on timeout" "grandchild pid never recorded"
  fi
}

_run_t_fast_exit_long_timeout() {
  local start=$SECONDS
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --timeout 3600 -- /bin/sh -c "echo \$\$ > '$_RUN_W/pid'; exit 0"
  _run_assert_rc "run: fast command with --timeout 3600 => 0" 0
  _run_assert_under "run: guard-run exits promptly (< 5 s)" "$start" 5
  if _run_take_pid "$_RUN_W/pid"; then
    _run_pid_dead "run: fixture pid gone after fast exit" "$_RUN_TAKEN"
  else
    _report fail "run: fixture pid gone after fast exit" "fixture pid never recorded"
  fi
}

_run_t_leftover_group_child() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --timeout 3600 -- /bin/sh -c "sleep 25 & echo \$! > '$_RUN_W/gpid'; exit 0"
  _run_assert_rc "run: command exiting with a same-group child => 0" 0
  if _run_take_pid "$_RUN_W/gpid"; then
    _run_pid_dead "run: same-group leftover child killed at exit" "$_RUN_TAKEN"
  else
    _report fail "run: same-group leftover child killed at exit" "child pid never recorded"
  fi
}

_run_t_twenty_short() {
  local i=1 bad="" pids="" p
  while [ "$i" -le 20 ]; do
    _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "echo \$\$ > '$_RUN_W/p$i'; exit 0"
    if [ "$_RG_HUNG" -eq 1 ] || [ "$_RG_RC" -ne 0 ]; then
      bad="$bad run$i:rc$_RG_RC"
    fi
    if _run_take_pid "$_RUN_W/p$i"; then
      pids="$pids $_RUN_TAKEN"
    else
      bad="$bad run$i:nopid"
    fi
    i=$((i + 1))
  done
  assert_eq "run: 20 short runs all exit 0 with a recorded fixture pid" "" "$bad"
  p=$(_run_alive_list $pids)
  assert_eq "run: 20 sequential runs leave zero live fixture or helper pids" "" "$p"
}

# ------------------------------------------------------------ setup failures

# _run_setup_fail NAME BODY : a fake perl body; command must never run.
_run_setup_fail() {
  local old_path="$PATH" rc
  _run_fake_perl "$1" "$2"
  PATH="$_RUN_W/fakebin:$old_path"
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  PATH="$old_path"
  rc=$_RG_RC
  if [ "$_RG_HUNG" -eq 1 ]; then
    _report fail "$1" "guard-run exceeded its time budget and was killed"
  else
    case "$rc" in
      126|127) _report pass "$1 => $rc" ;;
      *) _report fail "$1" "expected 126 or 127 got $rc" ;;
    esac
  fi
  assert_zero "$1: command never executed" test ! -e "$_RUN_W/marker"
}

_run_t_setsid_minus1() {
  _run_setup_fail "run: setup failure, setsid returns -1 (fake perl)" 'exit 126 # setsid() returned -1: no READY is written'
}

_run_t_setsid_undef() {
  _run_setup_fail "run: setup failure, undef setsid result, no READY (fake perl)" 'exit 0 # no READY ever arrives: handshake timeout'
}

_run_t_exec_fail() {
  _run_setup_fail "run: setup failure, exec fails before READY (fake perl)" 'exit 127 # exec failure before READY'
}

# ------------------------------------------------------- handshake barriers

# _run_barrier NAME POINT SIGNAL EXPECTED : signal the guard while it is held at
# the given barrier (ready|go). The command must never execute.
_run_barrier() {
  local pause="$_RUN_W/pause"
  touch "$pause"
  case "$2" in
    ready) export DG_PAUSE_launch_before_ready="$pause" ;;
    go) export DG_PAUSE_launch_before_go="$pause" ;;
  esac
  _run_start "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  sleep 1.5
  kill -"$3" "$_RUN_PID" 2>/dev/null
  _run_wait 15
  rm -f "$pause"
  _run_assert_rc "$1" "$4"
  assert_zero "$1: command never executed" test ! -e "$_RUN_W/marker"
}

_run_t_barrier_ready_int() { _run_barrier "run: INT held before READY => 130" ready INT 130; }
_run_t_barrier_ready_term() { _run_barrier "run: TERM held before READY => 143" ready TERM 143; }
_run_t_barrier_go_int() { _run_barrier "run: INT held after READY before GO => 130" go INT 130; }
_run_t_barrier_go_term() { _run_barrier "run: TERM held after READY before GO => 143" go TERM 143; }

_run_t_ready_mismatch() {
  export DG_FAKE_READY_MISMATCH=1
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: READY pid != pgid => 126" 126
  assert_zero "run: READY mismatch never executed command" test ! -e "$_RUN_W/marker"
}

# ------------------------------------------------------------ ulimit ceiling

_run_t_ulimit_value() {
  local soft expected actual
  _run_fake_count 500
  soft=$(ulimit -Su)
  expected=$( ( . "$DG_LIB_PATH" && dg_ulimit_value 500 500 100 "$soft" ) 2>/dev/null )
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c 'ulimit -u'
  _run_assert_rc "run: ulimit probe command rc 0" 0
  actual=$(cat "$_RUN_W/o")
  assert_eq "run: child ulimit -u equals dg_ulimit_value(500, 500, 100, soft)" "$expected" "$actual"
}

_run_t_reserve_insufficient() {
  export DG_RESERVE=100000000
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: insufficient reserve => 75" 75
  assert_zero "run: insufficient reserve never ran command" test ! -e "$_RUN_W/marker"
}

_run_t_headroom_zero() {
  _run_fake_count 500
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --headroom 0 -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: headroom 0 with reserve 100 at count 500 => 75" 75
  assert_zero "run: headroom 0 never ran command" test ! -e "$_RUN_W/marker"
}

# ------------------------------------------------------------------- locks

_run_t_lock_order() {
  local p1 p2 rc1 rc2 a_end b_start
  _run_start "$_RUN_W/o1" "$_RUN_W/e1" --lock demo -- /bin/sh -c "date +%s > '$_RUN_W/a_start'; sleep 3; date +%s > '$_RUN_W/a_end'"
  p1=$_RUN_PID
  sleep 0.5
  _run_start "$_RUN_W/o2" "$_RUN_W/e2" --lock demo -- /bin/sh -c "date +%s > '$_RUN_W/b_start'"
  p2=$_RUN_PID
  _RUN_PID=$p1
  _run_wait 20
  rc1=$_RG_RC
  _RUN_PID=$p2
  _run_wait 20
  rc2=$_RG_RC
  assert_eq "run: --lock first holder exits 0" 0 "$rc1"
  assert_eq "run: --lock second waiter proceeds (exit 0)" 0 "$rc2"
  a_end=$(cat "$_RUN_W/a_end" 2>/dev/null)
  b_start=$(cat "$_RUN_W/b_start" 2>/dev/null)
  if [ -n "$a_end" ] && [ -n "$b_start" ] && [ "$b_start" -ge "$a_end" ]; then
    _report pass "run: second command started only after first released the lock"
  else
    _report fail "run: second command started only after first released the lock" "a_end=[$a_end] b_start=[$b_start]"
  fi
}

_run_t_lock_wait_timeout() {
  export DG_LOCK_WAIT=2
  _run_start "$_RUN_W/o1" "$_RUN_W/e1" --lock busy -- /bin/sh -c "sleep 6"
  local p1=$_RUN_PID
  sleep 0.5
  _run_guard 15 "$_RUN_W/o2" "$_RUN_W/e2" --lock busy -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: lock wait timeout => 75" 75
  assert_zero "run: lock wait timeout never ran command" test ! -e "$_RUN_W/marker"
  _RUN_PID=$p1
  _run_wait 15
  assert_eq "run: holder of the lock still exits 0" 0 "$_RG_RC"
}

_run_t_lock_stale_reclaimed() {
  local dead
  /bin/sh -c 'exit 0' &
  dead=$!
  wait "$dead"
  if kill -0 "$dead" 2>/dev/null; then
    _report fail "run: stale-lock fixture pid is dead" "pid $dead unexpectedly alive"
  else
    _report pass "run: stale-lock fixture pid is dead"
  fi
  mkdir -p "$DG_HOME/locks/st.lock"
  printf '%s tok-dead Mon Jan  1 00:00:00 2024\n' "$dead" > "$DG_HOME/locks/st.lock/owner"
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --lock st -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: dead-owner lock reclaimed => 0" 0
  assert_zero "run: reclaimed lock ran the command" test -e "$_RUN_W/marker"
  assert_zero "run: reclaimed lock released afterwards" test ! -d "$DG_HOME/locks/st.lock"
}

_run_t_lock_no_owner_fails_closed() {
  export DG_LOCK_WAIT=2
  mkdir -p "$DG_HOME/locks/bare.lock"
  _run_guard 15 "$_RUN_W/o" "$_RUN_W/e" --lock bare -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: lock dir without owner record => 75 (never reclaimed)" 75
  assert_zero "run: ownerless lock never ran command" test ! -e "$_RUN_W/marker"
  assert_zero "run: ownerless lock dir left in place" test -d "$DG_HOME/locks/bare.lock"
}

_run_t_lock_released_after_command() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" --lock rel -- /bin/sh -c 'exit 0'
  _run_assert_rc "run: --lock command rc 0" 0
  assert_zero "run: lock released after command (no lock dir)" test ! -d "$DG_HOME/locks/rel.lock"
}

_run_t_lock_released_after_timeout() {
  _run_guard 15 "$_RUN_W/o" "$_RUN_W/e" --timeout 2 --lock to -- sleep 25
  _run_assert_rc "run: --lock command timed out => 124" 124
  assert_zero "run: lock released after timeout kill (no lock dir)" test ! -d "$DG_HOME/locks/to.lock"
  _run_guard 10 "$_RUN_W/o2" "$_RUN_W/e2" --lock to -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: lock reusable after timeout => 0" 0
  assert_zero "run: lock reusable after timeout ran command" test -e "$_RUN_W/marker"
}

# ------------------------------------------------------- orphan census

_run_t_census_escapee() {
  local esc unrel
  cat > "$_RUN_W/escapee.sh" <<'EOF'
#!/bin/sh
perl -MPOSIX -e 'POSIX::setsid(); exec "sleep","8"' &
echo $! > "$1"
exit 0
EOF
  /bin/sleep 20 &
  unrel=$!
  _run_record "$unrel"
  _run_guard 15 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh "$_RUN_W/escapee.sh" "$_RUN_W/esc"
  _run_assert_rc "run: escapee fixture command rc 0" 0
  assert_contains "run: census reports unattributed new launchd-parented process(es)" "$(cat "$_RUN_W/e")" "unattributed new launchd-parented"
  if _run_take_pid "$_RUN_W/esc"; then
    esc=$_RUN_TAKEN
    assert_zero "run: escapee (own session) not killed by guard-run" kill -0 "$esc"
    kill -KILL "$esc" 2>/dev/null
  else
    _report fail "run: escapee (own session) not killed by guard-run" "escapee pid never recorded"
  fi
  assert_zero "run: unrelated test-started sleep untouched" kill -0 "$unrel"
  kill -KILL "$unrel" 2>/dev/null
}

_run_t_census_incomplete() {
  export DG_FAKE_PS_FAIL_CENSUS=1
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c "touch '$_RUN_W/marker'"
  _run_assert_rc "run: census failure does not change command status" 0
  assert_contains "run: census failure reported as census-incomplete" "$(cat "$_RUN_W/e")" "census-incomplete"
}

# ------------------------------------------------------- secret hygiene

_run_t_no_secrets_in_logs() {
  export DG_SECRET_PROBE=sekrit-7f3a9
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/sh -c 'exit 0' dummy argmark-5d2e
  _run_assert_rc "run: secret-probe run rc 0" 0
  export DG_FAKE_PS_FAIL_CENSUS=1
  _run_guard 10 "$_RUN_W/o2" "$_RUN_W/e2" -- /bin/sh -c 'exit 0' dummy argmark-5d2e
  assert_eq "run: no env value, env name or command argument in DG_HOME" "" \
    "$(grep -r -e sekrit-7f3a9 -e DG_SECRET_PROBE -e argmark-5d2e "$DG_HOME" 2>/dev/null)"
}

# A single argument is one program path, never a shell line. A path with a
# space must run that tool; a single argument with ';' must not run a shell.
_run_t_single_arg_space_path() {
  mkdir -p "$_RUN_W/my dir"
  printf '%s\n' '#!/bin/sh' 'echo tool-marker-x1' >"$_RUN_W/my dir/tool"
  chmod 700 "$_RUN_W/my dir/tool"
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- "$_RUN_W/my dir/tool"
  _run_assert_rc "run: single path argument containing a space runs that tool => 0" 0
  assert_contains "run: single space-path tool prints its marker" "$(cat "$_RUN_W/o")" "tool-marker-x1"
}

_run_t_single_arg_shell_text() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- 'echo one; echo two'
  _run_assert_rc "run: one argument 'echo one; echo two' is not a shell line => 127" 127
  assert_not_contains "run: one argument never ran a shell (no second echo output)" "$(cat "$_RUN_W/o")" "two"
}

_run_t_multi_arg_still_works() {
  _run_guard 10 "$_RUN_W/o" "$_RUN_W/e" -- /bin/echo multi arg ok
  _run_assert_rc "run: multi-argument command => 0" 0
  assert_eq "run: multi-argument command output" "multi arg ok" "$(cat "$_RUN_W/o")"
}

# ------------------------------------------------------- registration

_run_case "run: usage: missing -- separator => 64" _run_t_usage_no_sep
_run_case "run: usage: no command after -- => 64" _run_t_usage_no_cmd
_run_case "run: usage: --timeout abc => 64" _run_t_usage_timeout_abc
_run_case "run: usage: --timeout -1 => 64" _run_t_usage_timeout_neg
_run_case "run: usage: --lock ../x => 64" _run_t_usage_lock_name
_run_case "run: usage: --headroom 99999999999 => 64" _run_t_usage_headroom
_run_case "run: refuse at warn => 75" _run_t_refuse_warn
_run_case "run: refuse at hard => 75" _run_t_refuse_hard
_run_case "run: refuse when count unknown (garbled) => 75" _run_t_refuse_garbled
_run_case "run: refuse when count unknown (missing file) => 75" _run_t_refuse_missing
_run_case "run: 12 runs cycling 0/7/126/127" _run_t_status_cycle
_run_case "run: stdout/stderr pass through" _run_t_passthrough
_run_case "run: single argument with a space in its path runs that tool" _run_t_single_arg_space_path
_run_case "run: single argument 'echo one; echo two' is not run by a shell" _run_t_single_arg_shell_text
_run_case "run: multi-argument command still runs" _run_t_multi_arg_still_works
_run_case "run: timeout => 124 and group grandchild dead" _run_t_timeout_group
_run_case "run: fast exit with --timeout 3600 leaves nothing, exits promptly" _run_t_fast_exit_long_timeout
_run_case "run: same-group leftover child killed at exit" _run_t_leftover_group_child
_run_case "run: 20 short runs leave zero live pids" _run_t_twenty_short
_run_case "run: setup failure, setsid -1 (fake perl) => 126" _run_t_setsid_minus1
_run_case "run: setup failure, undef setsid / no READY (fake perl) => 126" _run_t_setsid_undef
_run_case "run: setup failure, exec fails before READY (fake perl) => 126/127" _run_t_exec_fail
_run_case "run: INT at READY barrier => 130, command never ran" _run_t_barrier_ready_int
_run_case "run: TERM at READY barrier => 143, command never ran" _run_t_barrier_ready_term
_run_case "run: INT at GO barrier => 130, command never ran" _run_t_barrier_go_int
_run_case "run: TERM at GO barrier => 143, command never ran" _run_t_barrier_go_term
_run_case "run: READY pid != pgid => 126, command never ran" _run_t_ready_mismatch
_run_case "run: child ulimit -u equals dg_ulimit_value" _run_t_ulimit_value
_run_case "run: insufficient reserve => 75" _run_t_reserve_insufficient
_run_case "run: headroom 0 below reserve => 75" _run_t_headroom_zero
_run_case "run: --lock second waiter runs after first releases" _run_t_lock_order
_run_case "run: --lock wait timeout => 75" _run_t_lock_wait_timeout
_run_case "run: stale (dead-owner) lock reclaimed" _run_t_lock_stale_reclaimed
_run_case "run: ownerless lock fails closed (75)" _run_t_lock_no_owner_fails_closed
_run_case "run: --lock released after command" _run_t_lock_released_after_command
_run_case "run: --lock released after timeout kill" _run_t_lock_released_after_timeout
_run_case "run: census reports escapee, escapee and unrelated sleep untouched" _run_t_census_escapee
_run_case "run: census-incomplete when census ps fails" _run_t_census_incomplete
_run_case "run: no env values or command arguments in DG_HOME" _run_t_no_secrets_in_logs
