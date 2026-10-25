#!/bin/bash
# guard-run.sh [--timeout SECS] [--lock NAME] [--headroom N] -- cmd args...
#
# Runs one command under dev-guard admission control (DESIGN-GUARD.md Rev 4).
# Scope: COOPERATIVE admission control, not a sandbox. Only commands started
# through this script are covered.
#
# Exit status: the command's own status (0/7/126/127 after GO are passed
# through unchanged); 64 usage error; 75 refused (count not ok, lock busy,
# no headroom); 124 timeout; 126 setup failure (the command never ran);
# 130/143 on INT/TERM.
#
# Bash 3.2 compatible (macOS /bin/bash). Needs lib.sh beside this file.

_GR_DIR=$(cd "$(dirname "$0")" && pwd) || exit 64
. "$_GR_DIR/lib.sh" || exit 64

_GR_TIMEOUT=3600
_GR_LOCK=""
_GR_HEADROOM=500
_GR_CMD=()
_GR_HOME=""
_GR_RUNDIR=""
_GR_LOCK_DIR=""
_GR_LOCK_TOK=""
_GR_CHILD=""
_GR_REAPED=0
_GR_ARMED=0
_GR_SIG=""
_GR_STATUS=0
_GR_OUTCOME=""
_GR_FINAL=126
_GR_START=$SECONDS
_GR_COUNT=""
_GR_V=""
_GR_CENSUS_OK=1
_GR_NEW_COUNT=unknown
_GR_BKEYS="|"
_GR_FAKE_MISMATCH=0

# Launcher: caps the soft process limit inside the child only, then becomes
# perl. perl calls setsid() (checked), announces READY, blocks for GO, and
# only then execs the command. The shell's own fds are closed by the caller.
_GR_LAUNCHER='ulimit -S -u "$1" || exit 126
code=$2
ready=$3
go=$4
shift 4
exec perl -MPOSIX -e "$code" -- "$ready" "$go" "$@"'

_GR_PERL='my ($ready, $go) = (shift @ARGV, shift @ARGV);
$SIG{INT} = "DEFAULT";
$SIG{QUIT} = "DEFAULT";
my $sid = POSIX::setsid();
exit 126 unless defined $sid && $sid > 0;
open(my $rw, ">", $ready) or exit 126;
print $rw "READY $$ " . getpgrp() . "\n";
close($rw) or exit 126;
open(my $gr, "<", $go) or exit 125;
my $line = <$gr>;
close($gr);
exit 125 unless defined $line && $line eq "GO\n";
exec @ARGV or exit 127;'

_gr_usage_error() {
  printf 'guard: usage: %s\n' "$1" >&2
  printf 'usage: guard-run.sh [--timeout SECS] [--lock NAME] [--headroom N] -- cmd args...\n' >&2
  exit 64
}

# _gr_parse ARGS... : sets the option globals and _GR_CMD. Validates before
# anything runs, so a usage error never starts a command.
_gr_parse() {
  local have_sep=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --)
        have_sep=1
        shift
        break
        ;;
      --timeout|--lock|--headroom)
        [ $# -ge 2 ] || _gr_usage_error "$1 needs a value"
        case "$1" in
          --timeout) _GR_TIMEOUT=$2 ;;
          --lock) _GR_LOCK=$2 ;;
          --headroom) _GR_HEADROOM=$2 ;;
        esac
        shift 2
        ;;
      *)
        _gr_usage_error "unknown argument: $1"
        ;;
    esac
  done
  [ "$have_sep" -eq 1 ] || _gr_usage_error "missing -- before the command"
  [ $# -ge 1 ] || _gr_usage_error "no command after --"
  _dg_is_uint "$_GR_TIMEOUT" || _gr_usage_error "--timeout must be 1-9 digits"
  _dg_is_uint "$_GR_HEADROOM" || _gr_usage_error "--headroom must be 1-9 digits"
  if [ -n "$_GR_LOCK" ] && ! dg_validate_name "$_GR_LOCK"; then
    _gr_usage_error "--lock name must be 1-32 characters of [A-Za-z0-9_-]"
  fi
  _GR_CMD=("$@")
}

# _gr_home_setup : resolves DG_HOME, creates it 0700 when absent, refuses
# anything dg_home_check rejects.
_gr_home_setup() {
  _GR_HOME=${DG_HOME:-$HOME/.vox-guard}
  if [ ! -e "$_GR_HOME" ]; then
    if ! mkdir -m 700 -- "$_GR_HOME" 2>/dev/null; then
      echo "guard: usage: cannot create DG_HOME $_GR_HOME" >&2
      exit 64
    fi
  fi
  if ! dg_home_check "$_GR_HOME"; then
    echo "guard: usage: DG_HOME must be a real directory owned by you with mode 0700: $_GR_HOME" >&2
    exit 64
  fi
  DG_HOME=$_GR_HOME
  export DG_HOME
}

# _gr_rundir_make : private 0700 temp dir with the ready/go/nap fifos. The
# nap fifo is opened read-write on fd 7 so builtin reads block for the
# requested interval instead of seeing EOF.
_gr_rundir_make() {
  _GR_RUNDIR=$(mktemp -d "${TMPDIR:-/tmp}/dgrun.XXXXXX") || {
    echo "guard: setup failure: cannot create a private temp dir" >&2
    exit 126
  }
  chmod 700 "$_GR_RUNDIR"
  if ! mkfifo "$_GR_RUNDIR/ready" "$_GR_RUNDIR/go" "$_GR_RUNDIR/nap"; then
    echo "guard: setup failure: cannot create fifos" >&2
    exit 126
  fi
  if ! exec 7<>"$_GR_RUNDIR/nap"; then
    echo "guard: setup failure: cannot open nap fifo" >&2
    exit 126
  fi
}

# _gr_nap : one-second builtin wait on fd 7.
_gr_nap() {
  read -t 1 -u 7 _gr_nap_x
  return 0
}

_gr_on_sig() {
  _GR_SIG=$1
}

_gr_sig_code() {
  if [ "$_GR_SIG" = TERM ]; then
    echo 143
  else
    echo 130
  fi
}

# _gr_exit : EXIT trap. Kills only our own unreaped child, closes fds,
# releases the lock only with our token, removes the private temp dir.
_gr_exit() {
  local rc=$?
  if [ -n "$_GR_CHILD" ] && [ "$_GR_REAPED" -eq 0 ]; then
    kill -KILL "$_GR_CHILD" 2>/dev/null
    wait "$_GR_CHILD" 2>/dev/null
  fi
  exec 5>&- 6>&- 7>&- 2>/dev/null
  if [ -n "$_GR_LOCK_DIR" ] && [ -n "$_GR_LOCK_TOK" ]; then
    dg_own_release "$_GR_LOCK_DIR" "$_GR_LOCK_TOK" 2>/dev/null
  fi
  if [ -n "$_GR_RUNDIR" ]; then
    rm -rf -- "$_GR_RUNDIR"
  fi
  exit "$rc"
}

# _gr_preflight : sets _GR_COUNT on ok; otherwise explains and returns 75.
# Unknown (ps or id failed) is refused, never read as zero.
_gr_preflight() {
  local count="" level
  count=$(dg_count) || count=""
  level=$(dg_level "$count")
  case "$level" in
    ok)
      _GR_COUNT=$count
      return 0
      ;;
    unknown)
      echo "guard: refused: process count unknown (ps or id failed); nothing was run" >&2
      ;;
    *)
      echo "guard: refused: process count $count is $level (warn ${DG_WARN:-1200}, hard ${DG_HARD:-1800}); nothing was run" >&2
      ;;
  esac
  return 75
}

# _gr_lock_take : acquires $_GR_LOCK by polling dg_own_acquire once a second
# until DG_LOCK_WAIT seconds. Sets _GR_LOCK_DIR/_GR_LOCK_TOK. 75 on failure.
_gr_lock_take() {
  local waited=0 rc wait_s
  if ! mkdir -p -m 700 -- "$_GR_HOME/locks" 2>/dev/null; then
    echo "guard: refused: cannot create $_GR_HOME/locks" >&2
    return 75
  fi
  _GR_LOCK_DIR="$_GR_HOME/locks/$_GR_LOCK.lock"
  wait_s=${DG_LOCK_WAIT:-600}
  _dg_is_uint "$wait_s" || wait_s=600
  while :; do
    if [ -n "$_GR_SIG" ]; then
      echo "guard: interrupted while waiting for lock $_GR_LOCK" >&2
      exit "$(_gr_sig_code)"
    fi
    dg_own_acquire "$_GR_LOCK_DIR" _GR_LOCK_TOK
    rc=$?
    if [ "$rc" -eq 0 ]; then
      return 0
    fi
    if [ "$rc" -ne 1 ]; then
      # 75 already explained by the lock library; anything else is refused.
      [ "$rc" -eq 75 ] || echo "guard: refused: lock $_GR_LOCK unavailable (status $rc)" >&2
      return 75
    fi
    if [ "$waited" -ge "$wait_s" ]; then
      echo "guard: refused: lock $_GR_LOCK still held after ${wait_s}s" >&2
      return 75
    fi
    _gr_nap
    waited=$((waited + 1))
  done
}

# _gr_ceiling : computes the child's soft process limit. Sets _GR_V.
_gr_ceiling() {
  local soft rc reserve="${DG_RESERVE:-100}"
  soft=$(ulimit -Su)
  _GR_V=$(dg_ulimit_value "$_GR_COUNT" "$_GR_HEADROOM" "$reserve" "$soft")
  rc=$?
  case "$rc" in
    0) return 0 ;;
    64)
      echo "guard: usage: DG_RESERVE must be 1-9 digits and the soft limit numeric" >&2
      return 64
      ;;
    *)
      echo "guard: refused: no headroom (count $_GR_COUNT, headroom $_GR_HEADROOM, reserve $reserve, soft limit $soft)" >&2
      return 75
      ;;
  esac
}

# _gr_ps_snapshot FILE : this user's processes with pid, ppid, lstart, comm.
# 1 when inspection failed (test knob honoured only under DG_TEST=1).
_gr_ps_snapshot() {
  local uid
  if [ "${DG_TEST:-}" = 1 ] && [ "${DG_FAKE_PS_FAIL_CENSUS:-}" = 1 ]; then
    return 1
  fi
  uid=$(id -u) || return 1
  LC_ALL=C ps -u "$uid" -o pid=,ppid=,lstart=,comm= >"$1" 2>/dev/null
}

# _gr_orphan_keys RAW OUT : keeps ppid==1 rows as "key<TAB>name", where key is
# pid|lstart words, so a reused pid with a new start time is a new process.
_gr_orphan_keys() {
  local pid ppid w1 w2 w3 w4 w5 comm out=""
  # Built in a variable and written once: a file append per line costs
  # milliseconds per process on this platform.
  while read -r pid ppid w1 w2 w3 w4 w5 comm; do
    if [ "$ppid" = 1 ]; then
      out="$out$pid|$w1|$w2|$w3|$w4|$w5"$'\t'"${comm##*/}"$'\n'
    fi
  done <"$1"
  printf '%s' "$out" >"$2" || return 1
  return 0
}

# _gr_load_bkeys FILE : loads the before-run keys into _GR_BKEYS as |k|k|.
_gr_load_bkeys() {
  local key name
  _GR_BKEYS="|"
  while IFS=$'\t' read -r key name; do
    _GR_BKEYS="$_GR_BKEYS$key|"
  done <"$1"
  return 0
}

_gr_census_before() {
  _GR_CENSUS_OK=1
  if _gr_ps_snapshot "$_GR_RUNDIR/ps-before" &&
    _gr_orphan_keys "$_GR_RUNDIR/ps-before" "$_GR_RUNDIR/orph-before"; then
    _gr_load_bkeys "$_GR_RUNDIR/orph-before"
  else
    _GR_CENSUS_OK=0
  fi
}

# _gr_census_after : best-effort observation of NEW launchd-parented processes
# (see DESIGN-GUARD R1'). It cannot attribute them to this command.
_gr_census_after() {
  local key name newlist n=0 new="$_GR_RUNDIR/new" tally="$_GR_RUNDIR/tally" msg="" cnt nm
  if [ "$_GR_CENSUS_OK" -eq 1 ] &&
    _gr_ps_snapshot "$_GR_RUNDIR/ps-after" &&
    _gr_orphan_keys "$_GR_RUNDIR/ps-after" "$_GR_RUNDIR/orph-after"; then
    newlist=""
    while IFS=$'\t' read -r key name; do
      case "$_GR_BKEYS" in
        *"|$key|"*) ;;
        *)
          newlist="$newlist$name"$'\n'
          n=$((n + 1))
          ;;
      esac
    done <"$_GR_RUNDIR/orph-after"
    printf '%s' "$newlist" >"$new"
    _GR_NEW_COUNT=$n
  else
    _GR_CENSUS_OK=0
  fi
  if [ "$_GR_CENSUS_OK" -ne 1 ]; then
    echo "guard: census-incomplete" >&2
    _GR_NEW_COUNT=unknown
    return 0
  fi
  if [ "$n" -gt 0 ]; then
    sort "$new" | uniq -c | sort -rn | head -n 5 >"$tally"
    while read -r cnt nm; do
      msg="${msg:+$msg, }$nm x$cnt"
    done <"$tally"
    echo "guard: unattributed new launchd-parented process(es): $msg" >&2
  fi
  return 0
}

# _gr_log STATUS : one bounded line per launched run. No env, no arguments.
_gr_log() {
  local dur=$((SECONDS - _GR_START))
  printf '%s exit=%s duration=%ss launchd_new=%s\n' \
    "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$1" "$dur" "$_GR_NEW_COUNT" \
    >>"$_GR_HOME/guard-run.log" 2>/dev/null
  return 0
}

# _gr_launch : opens the handshake fifos read-write (never blocks) and starts
# the launcher. The launcher gets none of the supervisor's fds except stdio.
_gr_launch() {
  if ! exec 5<>"$_GR_RUNDIR/ready" || ! exec 6<>"$_GR_RUNDIR/go"; then
    echo "guard: setup failure: cannot open handshake fifos" >&2
    return 126
  fi
  bash -c "$_GR_LAUNCHER" guard-launcher "$_GR_V" "$_GR_PERL" \
    "$_GR_RUNDIR/ready" "$_GR_RUNDIR/go" "${_GR_CMD[@]}" \
    0<&0 5>&- 6>&- 7>&- &
  _GR_CHILD=$!
  if [ -z "$_GR_CHILD" ]; then
    echo "guard: setup failure: could not start the launcher" >&2
    return 126
  fi
  return 0
}

# _gr_hold VALUE : test pause, honoured only under DG_TEST=1. Spins while the
# named file exists, and stops early if a signal arrives.
_gr_hold() {
  if [ "${DG_TEST:-}" != 1 ] || [ -z "$1" ]; then
    return 0
  fi
  while [ -e "$1" ] && [ -z "$_GR_SIG" ]; do
    _gr_nap
  done
  return 0
}

# _gr_reap : collects the child's status exactly, then marks it reaped.
_gr_reap() {
  local st=0
  while :; do
    wait "$_GR_CHILD" 2>/dev/null
    st=$?
    if ! kill -0 "$_GR_CHILD" 2>/dev/null; then
      break
    fi
  done
  _GR_STATUS=$st
  _GR_REAPED=1
  return 0
}

# _gr_group_down PGID : TERM the group, up to 3 s, then KILL. The group id is
# our child's pid, so it cannot be reused while any member exists. ESRCH
# (group already gone) means there is nothing to do.
_gr_group_down() {
  local n=0
  kill -TERM -- "-$1" 2>/dev/null || return 0
  while kill -0 -- "-$1" 2>/dev/null; do
    if [ "$n" -ge 3 ]; then
      kill -KILL -- "-$1" 2>/dev/null
      return 0
    fi
    _gr_nap
    n=$((n + 1))
  done
  return 0
}

# _gr_abort_launcher : before GO only. Sends ABORT, then KILLs the launcher
# pid alone (it has not run the command, so no group is touched), and reaps.
_gr_abort_launcher() {
  printf 'ABORT\n' >&6 2>/dev/null
  kill -KILL "$_GR_CHILD" 2>/dev/null
  _gr_reap
}

# _gr_handshake : waits for READY (bounded 5 s), verifies pid == pgid, arms.
# 0 armed; 1 setup failure (message printed, child reaped, command never ran);
# 2 signal before READY (child reaped, command never ran).
_gr_handshake() {
  local i=0 line="" rc tag rpid rpgid
  if [ "${DG_TEST:-}" = 1 ] && [ "${DG_FAKE_READY_MISMATCH:-}" = 1 ]; then
    _GR_FAKE_MISMATCH=1
  fi
  _gr_hold "${DG_PAUSE_launch_before_ready:-}"
  while :; do
    if [ -n "$_GR_SIG" ]; then
      _gr_abort_launcher
      return 2
    fi
    if ! kill -0 "$_GR_CHILD" 2>/dev/null; then
      _gr_reap
      echo "guard: setup failure: launcher exited before READY (status $_GR_STATUS); command not run" >&2
      return 1
    fi
    if [ "$i" -ge 5 ]; then
      _gr_abort_launcher
      echo "guard: setup failure: no READY within 5 s; command not run" >&2
      return 1
    fi
    read -r -t 1 -u 5 line
    rc=$?
    if [ "$rc" -eq 0 ]; then
      break
    fi
    i=$((i + 1))
  done
  read -r tag rpid rpgid <<<"$line"
  if [ "$_GR_FAKE_MISMATCH" = 1 ]; then
    rpgid=0
  fi
  if [ "$tag" != READY ] || ! _dg_is_uint "$rpid" || ! _dg_is_uint "$rpgid"; then
    _gr_abort_launcher
    echo "guard: setup failure: malformed READY line; command not run" >&2
    return 1
  fi
  if [ "$rpid" != "$_GR_CHILD" ] || [ "$rpgid" != "$_GR_CHILD" ]; then
    _gr_abort_launcher
    echo "guard: setup failure: READY pid/pgid mismatch; command not run" >&2
    return 1
  fi
  _GR_ARMED=1
  return 0
}

# _gr_go : writes GO, or ABORT plus group teardown when a signal arrived
# after READY. 0 = GO sent; 2 = aborted by signal.
_gr_go() {
  _gr_hold "${DG_PAUSE_launch_before_go:-}"
  if [ -n "$_GR_SIG" ]; then
    printf 'ABORT\n' >&6 2>/dev/null
    _gr_group_down "$_GR_CHILD"
    _gr_reap
    return 2
  fi
  printf 'GO\n' >&6
  return 0
}

# _gr_supervise : waits for the command, enforcing the timeout and signals
# once a second. Sets _GR_FINAL.
_gr_supervise() {
  local start=$SECONDS
  _GR_OUTCOME=exit
  while kill -0 "$_GR_CHILD" 2>/dev/null; do
    if [ -n "$_GR_SIG" ]; then
      _GR_OUTCOME=signal
      break
    fi
    if [ $((SECONDS - start)) -ge "$_GR_TIMEOUT" ]; then
      _GR_OUTCOME=timeout
      break
    fi
    _gr_nap
  done
  if [ "$_GR_OUTCOME" != exit ]; then
    _gr_group_down "$_GR_CHILD"
  fi
  _gr_reap
  _gr_group_down "$_GR_CHILD"
  case "$_GR_OUTCOME" in
    timeout)
      echo "guard: timeout after ${_GR_TIMEOUT}s; process group terminated" >&2
      _GR_FINAL=124
      ;;
    signal)
      _GR_FINAL=$(_gr_sig_code)
      ;;
    *)
      _GR_FINAL=$_GR_STATUS
      ;;
  esac
}

# _gr_run_child : handshake, GO, supervise. Sets _GR_FINAL.
_gr_run_child() {
  local hs
  _gr_handshake
  hs=$?
  if [ "$hs" -eq 2 ]; then
    _GR_FINAL=$(_gr_sig_code)
    return 0
  fi
  if [ "$hs" -ne 0 ]; then
    _GR_FINAL=126
    return 0
  fi
  _gr_go
  if [ $? -eq 2 ]; then
    _GR_FINAL=$(_gr_sig_code)
    return 0
  fi
  _gr_supervise
}

# _gr_finish STATUS : stragglers (only for an armed group), census, log, exit.
_gr_finish() {
  if [ "$_GR_ARMED" -eq 1 ]; then
    _gr_group_down "$_GR_CHILD"
  fi
  _gr_census_after
  _gr_log "$1"
  exit "$1"
}

_gr_main() {
  trap '_gr_on_sig INT' INT
  trap '_gr_on_sig TERM' TERM
  trap _gr_exit EXIT
  _gr_parse "$@"
  _gr_home_setup
  _gr_rundir_make
  _gr_preflight || exit 75
  if [ -n "$_GR_LOCK" ]; then
    _gr_lock_take || exit 75
    _gr_preflight || exit 75
  fi
  _gr_ceiling || exit $?
  if [ -n "$_GR_SIG" ]; then
    exit "$(_gr_sig_code)"
  fi
  _gr_census_before
  _gr_launch || exit 126
  _gr_run_child
  _gr_finish "$_GR_FINAL"
}

_gr_main "$@"
