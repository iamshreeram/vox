#!/bin/bash
# Tests for scripts/dev-guard/proc-guard.sh (DESIGN-GUARD.md Rev 4): check,
# tick (R6'' stage bounds and sequencing), diagnostic throttling, ALERT
# throttling, rotation, watch start/stop/status (R5'' owner record, stop
# verification, stale replacement), orphans, report, backoff (R4), and that
# nothing is killed or persisted. Sourced by test-guard.sh after helpers.sh,
# t_lib.sh, t_misc.sh (sorted glob order); defines its own helpers.
# Bash 3.2 compatible (macOS /bin/bash).
#
# Safety rules for this file:
#  - fixtures are `sleep 25` / `sleep 20` (finite, <= 30 s);
#  - only pids recorded in _WT_PIDS are ever killed (no pkill/killall/name kills);
#    _wt_kill_recorded runs after every case;
#  - counts are simulated: DG_TEST=1 + DG_FAKE_COUNT file; real counts unused;
#  - helpers ps, sort, uniq, head, date, mv, rm, osascript are recording stubs,
#    put first on PATH only for proc-guard invocations (PATH=... on the command);
#  - no EXIT trap is set here (t_own.sh / t_run.sh own it).

PROC_GUARD="$SCRIPT_DIR/../proc-guard.sh"
_WT_PIDS=""
_WT_STUBS=""
_WT_OUT=""
_WT_RC=""

# _wt_record PID : remembers a pid this file started (killed after the case).
_wt_record() {
  _WT_PIDS="$_WT_PIDS $1"
}

# _wt_kill_recorded : kills ONLY the recorded pids that are still alive.
_wt_kill_recorded() {
  local p
  for p in $_WT_PIDS; do
    if kill -0 "$p" 2>/dev/null; then
      kill -KILL "$p" 2>/dev/null
    fi
  done
  _WT_PIDS=""
}

# _wt_case NAME FN : runs one test function, then kills its recorded pids.
_wt_case() {
  echo "# case: $1"
  "$2"
  _wt_kill_recorded
  unset DG_WARN DG_HARD DG_INTERVAL DG_ROTATE_BYTES DG_NOW DG_DEBUG_DELAYS
  unset DG_STUB_LOG DG_FAKE_COUNT DG_FAKE_PS_FAIL
}

# _wt_need PATH NAME : records a failure and returns 1 when a script is missing.
_wt_need() {
  if [ -f "$1" ]; then
    return 0
  fi
  _report fail "$2" "missing script: $1"
  return 1
}

# _wt_stubs DIR : recording stubs for the helpers. Each stub appends
# "ENTER name" and "EXIT name" lines to $DG_STUB_LOG around the real tool.
_wt_stubs() {
  local dir="$1" name real
  mkdir -p "$dir" || return 1
  for name in ps sort uniq head date mv rm osascript stat; do
    case "$name" in
      ps) real=/bin/ps ;;
      date) real=/bin/date ;;
      mv) real=/bin/mv ;;
      rm) real=/bin/rm ;;
      sort) real=/usr/bin/sort ;;
      uniq) real=/usr/bin/uniq ;;
      head) real=/usr/bin/head ;;
      stat) real=/usr/bin/stat ;;
      osascript) real=/usr/bin/true ;;
    esac
    printf '%s\n' '#!/bin/bash' \
      "echo \"ENTER $name\" >>\"\$DG_STUB_LOG\"" \
      "\"$real\" \"\$@\"" \
      'rc=$?' \
      "echo \"EXIT $name\" >>\"\$DG_STUB_LOG\"" \
      'exit $rc' >"$dir/$name" || return 1
    chmod 700 "$dir/$name" || return 1
  done
  return 0
}

# _wt_init : private DG_HOME, stubs, default knobs (warn 10, hard 20,
# interval 1, rotate 2000 bytes), fake count 5 and clock at 1000.
_wt_init() {
  new_dg_home || return 1
  _WT_STUBS="$DG_HOME/stubs"
  _wt_stubs "$_WT_STUBS" || return 1
  DG_WARN=10
  DG_HARD=20
  DG_INTERVAL=1
  DG_ROTATE_BYTES=2000
  DG_FAKE_COUNT="$DG_HOME/fake-count"
  DG_NOW="$DG_HOME/now"
  DG_DEBUG_DELAYS="$DG_HOME/delays"
  DG_STUB_LOG="$DG_HOME/stub.log"
  export DG_WARN DG_HARD DG_INTERVAL DG_ROTATE_BYTES DG_FAKE_COUNT DG_NOW
  export DG_DEBUG_DELAYS DG_STUB_LOG
  unset DG_FAKE_PS_FAIL
  : >"$DG_STUB_LOG"
  _wt_fake 5
  _wt_now 1000
  return 0
}

# _wt_fake N / _wt_now EPOCH : write the injected count file / clock file.
_wt_fake() {
  printf '%s\n' "$1" >"$DG_FAKE_COUNT"
}
_wt_now() {
  printf '%s\n' "$1" >"$DG_NOW"
}

# _wt_capture ARGS... : runs proc-guard with the stubs first on PATH; output
# goes to a file (a background loop must not hold a pipe). Sets _WT_OUT, _WT_RC.
_wt_capture() {
  PATH="$_WT_STUBS:$PATH" "$PROC_GUARD" "$@" >"$DG_HOME/out.txt" 2>&1
  _WT_RC=$?
  _WT_OUT=$(cat "$DG_HOME/out.txt" 2>/dev/null)
}

# _wt_tick : one tick with a fresh stub log.
_wt_tick() {
  : >"$DG_STUB_LOG"
  _wt_capture tick
}

# Stub log queries.
_wt_names() {
  grep '^ENTER ' "$DG_STUB_LOG" | cut -d' ' -f2 | tr '\n' ' ' | sed 's/ *$//'
}
_wt_peak() {
  awk '/^ENTER /{d++; if (d > m) m = d} /^EXIT /{d--} END {print m + 0}' "$DG_STUB_LOG"
}
# _wt_names_tick : helper names of a one-shot CLI tick after the home-setup
# prefix. dg_home_check runs its stat calls before the first ps, so the leading
# stat entries are stripped and the rest must be the tick's own sequence.
_wt_names_tick() {
  local n
  n=$(_wt_names)
  while :; do
    case "$n" in
      stat) n= ;;
      stat\ *) n=${n#stat } ;;
      *) break ;;
    esac
  done
  echo "$n"
}

# _wt_steady_check FILE : FILE holds one helper name per line (a snapshot of the
# stub log taken while the watcher loop runs). Each tick is the segment that
# starts at a ps entry. The first segment that is exactly date is tick 1;
# every complete segment after it must be exactly date. Prints ticks=N bad=[..].
_wt_steady_check() {
  local name seg="" seen=0 started=0 ticks=0 bad=""
  while IFS= read -r name; do
    if [ "$name" = ps ]; then
      if [ "$seen" -eq 1 ]; then
        if [ "$started" -eq 0 ]; then
          if [ "$seg" = " date" ]; then
            started=1
            ticks=1
          fi
        elif [ "$seg" = " date" ]; then
          ticks=$((ticks + 1))
        else
          bad="$bad[$seg ]"
        fi
      fi
      seen=1
      seg=""
    else
      seg="$seg $name"
    fi
  done <"$1"
  echo "ticks=$ticks bad=$bad"
}

_wt_nhelper() {
  grep -c "^ENTER $1\$" "$DG_STUB_LOG" 2>/dev/null
}

# _wt_count_in FILE NEEDLE : number of lines containing NEEDLE (0 if no file).
_wt_count_in() {
  local n
  n=$(grep -c -F "$2" "$1" 2>/dev/null)
  echo "${n:-0}"
}

# _wt_lines FILE : number of lines (0 if no file).
_wt_lines() {
  local n
  n=$(grep -c '' "$1" 2>/dev/null)
  echo "${n:-0}"
}

# _wt_ge NAME ACTUAL MIN
_wt_ge() {
  if [ "$2" -ge "$3" ] 2>/dev/null; then
    _report pass "$1"
  else
    _report fail "$1" "expected >= $3 got [$2]"
  fi
}

# _wt_pid_from TEXT : the pid printed as "... pid N".
_wt_pid_from() {
  printf '%s\n' "$1" | sed -n 's/.*pid \([0-9][0-9]*\).*/\1/p' | head -n 1
}

# _wt_wait_dead PID : polls up to 10 s for the pid to disappear.
_wt_wait_dead() {
  local i=0
  while kill -0 "$1" 2>/dev/null && [ "$i" -lt 100 ]; do
    sleep 0.1
    i=$((i + 1))
  done
  if kill -0 "$1" 2>/dev/null; then
    return 1
  fi
  return 0
}

# _wt_check_dead NAME PID
_wt_check_dead() {
  if _wt_wait_dead "$2"; then
    _report pass "$1"
  else
    _report fail "$1" "pid $2 still alive"
  fi
}

# _wt_prefill : writes a log bigger than DG_ROTATE_BYTES (2000). The log is
# written outside the guard, so the byte counter is dropped and the next tick
# re-seeds it from the file size.
_wt_prefill() {
  local i=0
  while [ "$i" -lt 100 ]; do
    echo "filler line $i padding padding padding"
    i=$((i + 1))
  done >"$DG_HOME/proc-watch.log"
  rm -f "$DG_HOME/log.bytes"
}

test_watch_check_levels() {
  _wt_need "$PROC_GUARD" "check: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 5
  _wt_capture check
  assert_eq "check ok exit 0" 0 "$_WT_RC"
  assert_contains "check ok prints procs=5 level=ok" "$_WT_OUT" "procs=5 level=ok"
  _wt_fake 15
  _wt_capture check
  assert_eq "check warn exit 10" 10 "$_WT_RC"
  assert_contains "check warn prints level=warn" "$_WT_OUT" "level=warn"
  _wt_fake 25
  _wt_capture check
  assert_eq "check hard exit 20" 20 "$_WT_RC"
  assert_contains "check hard prints level=hard" "$_WT_OUT" "level=hard"
  printf '%s\n' garbled-xx >"$DG_FAKE_COUNT"
  _wt_capture check
  assert_eq "check garbled count exit 30" 30 "$_WT_RC"
  assert_contains "check garbled count prints level=unknown" "$_WT_OUT" "level=unknown"
  unset DG_FAKE_COUNT
  DG_FAKE_PS_FAIL=1
  export DG_FAKE_PS_FAIL
  _wt_capture check
  assert_eq "check ps failure exit 30" 30 "$_WT_RC"
  assert_contains "check ps failure prints level=unknown" "$_WT_OUT" "level=unknown"
  unset DG_FAKE_PS_FAIL
  DG_FAKE_COUNT="$DG_HOME/fake-count"
  export DG_FAKE_COUNT
}

test_watch_tick_ordinary() {
  _wt_need "$PROC_GUARD" "tick: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 5
  _wt_tick
  assert_eq "ordinary tick exit 0" 0 "$_WT_RC"
  assert_eq "ordinary tick appends exactly one line" 1 "$(_wt_lines "$DG_HOME/proc-watch.log")"
  assert_zero "ordinary tick line is ISO COUNT LEVEL" grep -E -q '^[^ ]+ 5 ok$' "$DG_HOME/proc-watch.log"
  assert_eq "ordinary tick (after home setup) launches exactly ps then date" "ps date" "$(_wt_names_tick)"
  assert_eq "ordinary tick peak helper concurrency 1" 1 "$(_wt_peak)"
}

test_watch_tick_warn_diagnostic() {
  _wt_need "$PROC_GUARD" "tick warn: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 15
  _wt_tick
  assert_eq "warn tick: ordinary ps,date then 6 diagnostic helpers in order" \
    "ps date ps sort uniq sort head ps" "$(_wt_names_tick)"
  assert_eq "warn tick peak helper concurrency 1" 1 "$(_wt_peak)"
  assert_zero "warn tick logs a 15 warn line" grep -E -q '^[^ ]+ 15 warn$' "$DG_HOME/proc-watch.log"
  _wt_ge "warn tick appends a diagnostic block (more than one line)" \
    "$(_wt_lines "$DG_HOME/proc-watch.log")" 2
}

test_watch_tick_diag_throttle() {
  _wt_need "$PROC_GUARD" "tick throttle: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 15
  _wt_now 1000
  _wt_tick
  assert_eq "diagnostic at t=1000" 1 "$(_wt_nhelper head)"
  _wt_now 1030
  _wt_tick
  assert_eq "no diagnostic at t=1030 (under 60 s, after home setup)" "ps date" "$(_wt_names_tick)"
  _wt_now 1061
  _wt_tick
  assert_eq "diagnostic again at t=1061" 1 "$(_wt_nhelper head)"
}

test_watch_tick_hard_alert_throttle() {
  _wt_need "$PROC_GUARD" "tick hard: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 25
  _wt_now 1000
  _wt_tick
  assert_eq "hard tick notifies once (osascript)" 1 "$(_wt_nhelper osascript)"
  assert_eq "hard tick peak helper concurrency 1" 1 "$(_wt_peak)"
  assert_zero "hard tick logs an ALERT line" grep -q ALERT "$DG_HOME/proc-watch.log"
  _wt_now 1100
  _wt_tick
  assert_eq "no second notification at t=1100 (under 600 s)" 0 "$(_wt_nhelper osascript)"
  _wt_now 1601
  _wt_tick
  assert_eq "notification again at t=1601" 1 "$(_wt_nhelper osascript)"
}

test_watch_tick_rotation() {
  _wt_need "$PROC_GUARD" "tick rotation: proc-guard.sh present" || return 0
  _wt_init || return 0
  export DG_ROTATE_BYTES=2000
  _wt_prefill
  _wt_tick
  assert_zero "rotation keeps proc-watch.log.1" test -f "$DG_HOME/proc-watch.log.1"
  assert_zero "log.1 holds the old content" grep -q 'filler line 0' "$DG_HOME/proc-watch.log.1"
  _wt_ge "rotation tick invokes mv" "$(_wt_nhelper mv)" 1
  _wt_ge "rotation tick invokes rm" "$(_wt_nhelper rm)" 1
  assert_eq "rotation tick peak helper concurrency 1" 1 "$(_wt_peak)"
  _wt_prefill
  _wt_tick
  assert_zero "second rotation keeps proc-watch.log.2" test -f "$DG_HOME/proc-watch.log.2"
  _wt_prefill
  _wt_tick
  assert_zero "third rotation keeps no .3 copy" test ! -e "$DG_HOME/proc-watch.log.3"
}

test_watch_tick_unknown() {
  _wt_need "$PROC_GUARD" "tick unknown: proc-guard.sh present" || return 0
  _wt_init || return 0
  unset DG_FAKE_COUNT
  DG_FAKE_PS_FAIL=1
  export DG_FAKE_PS_FAIL
  _wt_tick
  unset DG_FAKE_PS_FAIL
  DG_FAKE_COUNT="$DG_HOME/fake-count"
  export DG_FAKE_COUNT
  assert_zero "failed sample logged as ISO unknown" grep -E -q '^[^ ]+ unknown$' "$DG_HOME/proc-watch.log"
  assert_not_contains "failed sample is never logged as ok" "$(tail -n 1 "$DG_HOME/proc-watch.log")" " ok"
}

test_watch_tick_steady_state() {
  _wt_need "$PROC_GUARD" "tick steady: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 5
  _wt_tick
  _wt_tick
  assert_eq "steady ordinary tick (log present, after home setup) launches exactly ps then date" "ps date" "$(_wt_names_tick)"
  assert_eq "steady ordinary tick adds no stat beyond the home check" "$_WT_HOME_STATS" "$(_wt_nhelper stat)"
  assert_eq "two ordinary ticks append two lines" 2 "$(_wt_lines "$DG_HOME/proc-watch.log")"
}

test_watch_tick_steady_warn() {
  _wt_need "$PROC_GUARD" "tick steady warn: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 5
  _wt_tick
  _wt_fake 15
  _wt_tick
  assert_eq "steady warn tick: ps date then 6 diagnostic helpers, no stat" \
    "ps date ps sort uniq sort head ps" "$(_wt_names_tick)"
}

# Every CLI invocation runs dg_home_check, which costs 2 stat calls (owner and
# mode) before any tick work. HOME_STATS is that baseline. A tick that needs no
# seed adds no stat beyond it; a seed adds exactly one.
_WT_HOME_STATS=2

# The counter alone decides rotation: reaching DG_ROTATE_BYTES rotates on the
# next tick, and the rotation adds no stat beyond the home check.
test_watch_tick_counter_rotation() {
  _wt_need "$PROC_GUARD" "tick counter rotation: proc-guard.sh present" || return 0
  _wt_init || return 0
  export DG_ROTATE_BYTES=2000
  echo 'old content line' >"$DG_HOME/proc-watch.log"
  printf '%s\n' 2000 >"$DG_HOME/log.bytes"
  _wt_tick
  assert_zero "counter at the limit rotates to proc-watch.log.1" grep -q 'old content line' "$DG_HOME/proc-watch.log.1"
  assert_eq "counter at the limit adds no stat beyond the home check" "$_WT_HOME_STATS" "$(_wt_nhelper stat)"
  assert_eq "counter reset to 0 then one new line counted" \
    "$(( $(wc -c <"$DG_HOME/proc-watch.log") ))" "$(cat "$DG_HOME/log.bytes")"
}

# A missing or garbled counter is seeded by exactly one stat; then none.
test_watch_counter_seed_once() {
  _wt_need "$PROC_GUARD" "counter seed: proc-guard.sh present" || return 0
  _wt_init || return 0
  echo 'seed line' >"$DG_HOME/proc-watch.log"
  rm -f "$DG_HOME/log.bytes"
  _wt_tick
  assert_eq "missing counter: one seed stat beyond the home check" "$((_WT_HOME_STATS + 1))" "$(_wt_nhelper stat)"
  _wt_tick
  assert_eq "after seeding, the next tick adds no seed stat" "$_WT_HOME_STATS" "$(_wt_nhelper stat)"
  printf '%s\n' garbled-xx >"$DG_HOME/log.bytes"
  _wt_tick
  assert_eq "garbled counter: one re-seed stat beyond the home check" "$((_WT_HOME_STATS + 1))" "$(_wt_nhelper stat)"
}

# date cannot fork: the tick is an unknown sample, written as unknown-time.
_wt_make_failing_date() {
  mkdir -p "$DG_HOME/failbin" || return 1
  printf '%s\n' '#!/bin/bash' 'exit 1' >"$DG_HOME/failbin/date" || return 1
  chmod 700 "$DG_HOME/failbin/date"
}

test_watch_date_failure_tick() {
  _wt_need "$PROC_GUARD" "date failure tick: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_make_failing_date || return 0
  _wt_fake 5
  PATH="$DG_HOME/failbin:$_WT_STUBS:$PATH" "$PROC_GUARD" tick >"$DG_HOME/out.txt" 2>&1
  assert_eq "date failure tick exit 0" 0 "$?"
  assert_zero "date failure logs unknown-time unknown" grep -q '^unknown-time unknown$' "$DG_HOME/proc-watch.log"
}

test_watch_date_failure_backoff() {
  _wt_need "$PROC_GUARD" "date failure backoff: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_make_failing_date || return 0
  _wt_fake 5
  PATH="$DG_HOME/failbin:$_WT_STUBS:$PATH" "$PROC_GUARD" watch start >"$DG_HOME/out.txt" 2>&1
  local pid waited all
  pid=$(_wt_pid_from "$(cat "$DG_HOME/out.txt")")
  _wt_record "$pid"
  waited=0
  while [ "$(_wt_lines "$DG_DEBUG_DELAYS")" -lt 3 ] && [ "$waited" -lt 250 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  _wt_capture watch stop
  all=" $(tr '\n' ' ' <"$DG_DEBUG_DELAYS" 2>/dev/null) "
  case "$all" in
    *" 2 4 "*) _report pass "date failure samples back off by doubling (2 then 4)" ;;
    *) _report fail "date failure samples back off by doubling (2 then 4)" "delays: $all" ;;
  esac
}

# The real steady state: the watcher loop itself. Home setup runs once at loop
# start, so each later ordinary sample must be exactly ps then date. The stub
# log is snapshotted before watch stop, so stop-time helper calls are excluded.
test_watch_loop_steady_state() {
  _wt_need "$PROC_GUARD" "loop steady: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_fake 5
  _wt_now 1000
  _wt_capture watch start
  local pid waited res ticks
  pid=$(_wt_pid_from "$_WT_OUT")
  _wt_record "$pid"
  waited=0
  while [ "$(_wt_lines "$DG_HOME/proc-watch.log")" -lt 4 ] && [ "$waited" -lt 200 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  grep '^ENTER ' "$DG_STUB_LOG" | cut -d' ' -f2 >"$DG_HOME/loop-names.txt"
  res=$(_wt_steady_check "$DG_HOME/loop-names.txt")
  _wt_capture watch stop
  assert_contains "loop steady: watcher stopped" "$_WT_OUT" "stopped"
  ticks=${res#ticks=}
  ticks=${ticks%% *}
  _wt_ge "loop steady: at least 3 ordinary samples observed" "$ticks" 3
  assert_eq "loop steady: every sample after the first is exactly ps then date" \
    "" "${res#*bad=}"
}

test_watch_hard_no_kill_no_secrets() {
  _wt_need "$PROC_GUARD" "tick secrets: proc-guard.sh present" || return 0
  _wt_init || return 0
  # Fixtures plant secrets in their environment and in their argv.
  env DG_PLANT_SECRET=envsecret-qq8 /bin/sleep 25 &
  local epid=$!
  _wt_record "$epid"
  /bin/sh -c '/bin/sleep 25; :' secretarg-qq7 &
  local apid=$!
  _wt_record "$apid"
  sleep 0.3
  _wt_fake 25
  _wt_now 1000
  _wt_tick
  assert_eq "hard tick exit 0" 0 "$_WT_RC"
  assert_zero "hard tick does not kill env fixture" kill -0 "$epid"
  assert_zero "hard tick does not kill argv fixture" kill -0 "$apid"
  assert_eq "log has no environment secret" 0 "$(_wt_count_in "$DG_HOME/proc-watch.log" envsecret-qq8)"
  assert_eq "log has no argument secret" 0 "$(_wt_count_in "$DG_HOME/proc-watch.log" secretarg-qq7)"
}

test_watch_orphans() {
  _wt_need "$PROC_GUARD" "orphans: proc-guard.sh present" || return 0
  _wt_init || return 0
  ( /bin/sleep 25 & echo $! >"$DG_HOME/orphan.pid" )
  local opid
  opid=$(cat "$DG_HOME/orphan.pid" 2>/dev/null)
  _wt_record "$opid"
  sleep 0.3
  local oppid
  oppid=$(ps -o ppid= -p "$opid" 2>/dev/null | tr -d ' ')
  assert_eq "orphan fixture reparented to ppid 1" 1 "$oppid"
  _wt_capture orphans
  assert_eq "orphans exit 0" 0 "$_WT_RC"
  assert_contains "orphans lists command name sleep" "$_WT_OUT" "sleep"
  assert_not_contains "orphans shows no arguments" "$_WT_OUT" "sleep 25"
}

test_watch_report() {
  _wt_need "$PROC_GUARD" "report: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_capture report
  assert_eq "report with no log exit 0" 0 "$_WT_RC"
  assert_contains "report with no log says no samples" "$_WT_OUT" "no samples"
  _wt_fake 25
  _wt_now 1000
  _wt_tick
  _wt_fake 5
  _wt_now 1200
  _wt_tick
  _wt_capture report
  assert_eq "report exit 0" 0 "$_WT_RC"
  assert_contains "report shows max count 25" "$_WT_OUT" "25"
  assert_contains "report shows last ALERT" "$_WT_OUT" "ALERT"
  if printf '%s\n' "$_WT_OUT" | grep -E -q 'samples?[^0-9]*2|2[^0-9]*samples?'; then
    _report pass "report shows 2 samples"
  else
    _report fail "report shows 2 samples" "output: $_WT_OUT"
  fi
}

test_watch_lifecycle() {
  _wt_need "$PROC_GUARD" "watch: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_capture watch status
  assert_eq "watch status before start exit 1" 1 "$_WT_RC"
  assert_contains "watch status before start says not running" "$_WT_OUT" "not running"
  _wt_capture watch start
  local pid
  pid=$(_wt_pid_from "$_WT_OUT")
  _wt_record "$pid"
  assert_contains "watch start prints started pid" "$_WT_OUT" "started pid"
  assert_eq "watch owner record pid is the loop pid" "$pid" \
    "$(cut -d' ' -f1 "$DG_HOME/locks/watch.lock/owner" 2>/dev/null)"
  _wt_capture watch start
  assert_contains "second watch start: already running same pid" "$_WT_OUT" "already running pid $pid"
  _wt_capture watch status
  assert_eq "watch status running exit 0" 0 "$_WT_RC"
  assert_contains "watch status prints running pid" "$_WT_OUT" "running pid $pid"
  _wt_capture watch stop
  assert_contains "watch stop prints stopped" "$_WT_OUT" "stopped"
  _wt_check_dead "watch loop gone after stop" "$pid"
  _wt_capture watch status
  assert_eq "watch status after stop exit 1" 1 "$_WT_RC"
}

test_watch_stale_replaced() {
  _wt_need "$PROC_GUARD" "watch stale: proc-guard.sh present" || return 0
  _wt_init || return 0
  _wt_capture watch start
  local first
  first=$(_wt_pid_from "$_WT_OUT")
  _wt_record "$first"
  kill -KILL "$first" 2>/dev/null
  _wt_check_dead "killed loop is dead (stale pid record left behind)" "$first"
  _wt_capture watch start
  local second
  second=$(_wt_pid_from "$_WT_OUT")
  _wt_record "$second"
  assert_contains "watch start replaces stale record" "$_WT_OUT" "started pid"
  assert_eq "replacement loop has a new pid" 1 "$([ "$second" != "$first" ] && echo 1 || echo 0)"
  _wt_capture watch stop
  assert_contains "replacement loop stops" "$_WT_OUT" "stopped"
}

test_watch_stop_refuses_unrelated() {
  _wt_need "$PROC_GUARD" "watch stop verify: proc-guard.sh present" || return 0
  _wt_init || return 0
  /bin/sleep 20 &
  local fpid=$!
  _wt_record "$fpid"
  local lstart
  lstart=$(LC_ALL=C /bin/ps -o lstart= -p "$fpid" 2>/dev/null | sed 's/^ *//;s/ *$//')
  mkdir -p "$DG_HOME/locks/watch.lock"
  printf '%s %s %s\n' "$fpid" faketoken-zz "$lstart" >"$DG_HOME/locks/watch.lock/owner"
  _wt_capture watch stop
  assert_rc_nonzero "watch stop refuses unverified pid (exit non-zero)" "$_WT_RC"
  assert_not_contains "watch stop does not claim stopped" "$_WT_OUT" "stopped"
  assert_zero "unrelated sleep fixture survives watch stop" kill -0 "$fpid"
}

test_watch_backoff_and_reset() {
  _wt_need "$PROC_GUARD" "watch backoff: proc-guard.sh present" || return 0
  _wt_init || return 0
  printf '%s\n' garbled-xx >"$DG_FAKE_COUNT"
  _wt_capture watch start
  local pid waited before after
  pid=$(_wt_pid_from "$_WT_OUT")
  _wt_record "$pid"
  waited=0
  while [ "$(_wt_lines "$DG_DEBUG_DELAYS")" -lt 4 ] && [ "$waited" -lt 250 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  before=$(_wt_lines "$DG_DEBUG_DELAYS")
  _wt_fake 5
  waited=0
  while [ "$(_wt_lines "$DG_DEBUG_DELAYS")" -lt $((before + 2)) ] && [ "$waited" -lt 250 ]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  _wt_capture watch stop
  local all tail_part
  all=" $(tr '\n' ' ' <"$DG_DEBUG_DELAYS" 2>/dev/null) "
  tail_part=" $(tail -n +$((before + 1)) "$DG_DEBUG_DELAYS" 2>/dev/null | tr '\n' ' ') "
  case "$all" in
    *" 2 4 "*) _report pass "unknown samples back off by doubling (2 then 4)" ;;
    *) _report fail "unknown samples back off by doubling (2 then 4)" "delays: $all" ;;
  esac
  case "$tail_part" in
    *" 1 "*) _report pass "good sample resets the delay to the base (1)" ;;
    *) _report fail "good sample resets the delay to the base (1)" "delays after good sample: $tail_part" ;;
  esac
  assert_contains "backoff loop stops on watch stop" "$_WT_OUT" "stopped"
}

_wt_case "tick/check: levels, exit codes, unknown on garbled or failing count" test_watch_check_levels
_wt_case "tick: ordinary tick is ps+date, one line, ok" test_watch_tick_ordinary
_wt_case "tick: warn adds the 6-helper diagnostic sequence" test_watch_tick_warn_diagnostic
_wt_case "tick: diagnostic throttled to once per 60 s" test_watch_tick_diag_throttle
_wt_case "tick: hard notifies at most once per 600 s, logs ALERT" test_watch_tick_hard_alert_throttle
_wt_case "tick: rotation keeps 2 copies (.1, .2) and no .3" test_watch_tick_rotation
_wt_case "tick: failed sample logged as unknown, never ok" test_watch_tick_unknown
_wt_case "tick: steady ordinary tick (second tick) is ps+date only, no stat" test_watch_tick_steady_state
_wt_case "tick: steady warn tick is ps+date+6 helpers, no stat" test_watch_tick_steady_warn
_wt_case "tick: counter at DG_ROTATE_BYTES rotates with no stat" test_watch_tick_counter_rotation
_wt_case "tick: missing or garbled counter seeded by exactly one stat" test_watch_counter_seed_once
_wt_case "tick: date failure is an unknown sample (unknown-time), exit 0" test_watch_date_failure_tick
_wt_case "watch: date failure doubles the delay (2 then 4)" test_watch_date_failure_backoff
_wt_case "orphans: lists ppid==1 fixture by command name, no arguments" test_watch_orphans
_wt_case "report: no samples, then max count and ALERT and sample count" test_watch_report
_wt_case "watch: start single instance, owner pid, status, stop" test_watch_lifecycle
_wt_case "watch: stale loop record replaced by a later start" test_watch_stale_replaced
_wt_case "watch: stop refuses a pid that fails owner verification" test_watch_stop_refuses_unrelated
_wt_case "watch loop: steady samples are exactly ps then date" test_watch_loop_steady_state
_wt_case "tick: hard kills nothing, log has no env or argv secrets" test_watch_hard_no_kill_no_secrets
_wt_case "watch: unknown samples back off, good sample resets" test_watch_backoff_and_reset
