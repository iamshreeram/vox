#!/bin/bash
# proc-guard.sh: samples how many processes this user has, logs the samples,
# and can keep one background watcher running. It never kills anything.
#
# Usage:
#   proc-guard.sh check                 procs=N level=L; exit 0 ok, 10 warn, 20 hard, 30 unknown
#   proc-guard.sh tick                  append one sample to $DG_HOME/proc-watch.log
#   proc-guard.sh watch start|stop|status
#   proc-guard.sh orphans               ppid==1 processes of this user, by command name (read only)
#   proc-guard.sh report                summary of the log
#
# Scope: COOPERATIVE admission control and evidence collection, not a sandbox
# or an account-wide process governor. See docs/dev-guard.md.
# Bash 3.2 compatible (macOS /bin/bash). Needs lib.sh beside this file.

umask 077

_PG_DIR=$(cd "$(dirname "$0")" && pwd) || exit 64
_PG_SELF="$_PG_DIR/proc-guard.sh"
. "$_PG_DIR/lib.sh" || exit 64

_PG_DEFAULT_ROTATE=5242880
_PG_DIAG_GAP=60
_PG_NOTIFY_GAP=600
_PG_BACKOFF_MAX=120
_PG_LOG=""
_PG_LOCKDIR=""
_PG_LOOP_TOK=""
_PG_RUNDIR=""
_PG_INTERVAL=10
_PG_COUNT=""
_PG_LEVEL=""
_PG_UNKNOWN=0
_PG_BYTES=0
_PG_ISO=""
_PG_EPOCH=""
_PG_FAKE=""
_PG_OPID=""
_PG_OTOK=""
_PG_OLST=""
_PG_LST_CUR=""
_PG_SEEN="|"
_PG_ORPH_TOTAL=0
_PG_ORPH_NAMES=""

_pg_usage() {
  echo "proc-guard: usage: $1" >&2
  echo "usage: proc-guard.sh check | tick | watch start|stop|status | orphans | report" >&2
  exit 64
}

# _pg_digits VALUE : 0 when VALUE is one or more ASCII digits. Unlike
# _dg_is_uint there is no length cap, because epoch seconds have ten digits.
_pg_digits() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
  esac
  return 0
}

# _pg_home_setup : resolves DG_HOME (created 0700 when absent) and refuses
# anything dg_home_check rejects. Sets _PG_LOG.
_pg_home_setup() {
  local home=${DG_HOME:-$HOME/.vox-guard}
  if [ ! -e "$home" ]; then
    if ! mkdir -m 700 -- "$home" 2>/dev/null; then
      echo "proc-guard: usage: cannot create DG_HOME $home" >&2
      exit 64
    fi
  fi
  if ! dg_home_check "$home"; then
    echo "proc-guard: usage: DG_HOME must be a real directory owned by you with mode 0700: $home" >&2
    exit 64
  fi
  DG_HOME=$home
  export DG_HOME
  _PG_LOG="$DG_HOME/proc-watch.log"
}

# _pg_interval : sets _PG_INTERVAL from DG_INTERVAL. Outside tests the floor
# is 10 seconds, so a stray environment value cannot make the watcher busy.
_pg_interval() {
  local v=${DG_INTERVAL:-10}
  if ! _dg_is_uint "$v" || [ "$v" -lt 1 ]; then
    echo "proc-guard: usage: DG_INTERVAL must be a whole number of seconds" >&2
    exit 64
  fi
  if [ "${DG_TEST:-}" != 1 ] && [ "$v" -lt 10 ]; then
    v=10
  fi
  _PG_INTERVAL=$v
}

# _pg_fake_count : sets _PG_FAKE from DG_FAKE_COUNT (test knob, DG_TEST=1 only).
_pg_fake_count() {
  local line=""
  [ -r "$DG_FAKE_COUNT" ] || return 1
  IFS= read -r line <"$DG_FAKE_COUNT" || return 1
  if ! _dg_is_uint "$line" || [ "$line" -lt 1 ]; then
    return 1
  fi
  _PG_FAKE=$line
  return 0
}

# _pg_measure : sets _PG_COUNT to a positive count, or returns 1 (unknown).
# One ps call is always made, so the sample costs the same in tests and in
# production. A failed ps, an empty listing or a garbled test count is unknown.
_pg_measure() {
  local out="" line n=0
  _PG_COUNT=""
  if ! out=$(ps -u "$UID" -o pid= 2>/dev/null); then
    return 1
  fi
  if [ "${DG_TEST:-}" = 1 ]; then
    if [ "${DG_FAKE_PS_FAIL:-}" = 1 ]; then
      return 1
    fi
    if [ -n "${DG_FAKE_COUNT:-}" ]; then
      _pg_fake_count || return 1
      _PG_COUNT=$_PG_FAKE
      return 0
    fi
  fi
  while IFS= read -r line; do
    case "$line" in
      *[0-9]*) n=$((n + 1)) ;;
    esac
  done <<<"$out"
  if ! _dg_is_uint "$n" || [ "$n" -lt 1 ]; then
    return 1
  fi
  _PG_COUNT=$n
  return 0
}

# _pg_sample : sets _PG_COUNT and _PG_LEVEL. Returns 1 when unknown.
_pg_sample() {
  if _pg_measure; then
    _PG_LEVEL=$(dg_level "$_PG_COUNT")
    if [ "$_PG_LEVEL" != unknown ]; then
      return 0
    fi
  fi
  _PG_COUNT=unknown
  _PG_LEVEL=unknown
  return 1
}

# _pg_now : one date call gives both the ISO stamp and the epoch. Test runs
# may override the epoch with DG_NOW so the throttles can be checked.
_pg_now() {
  local stamp e=""
  if ! stamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ %s'); then
    return 1
  fi
  _PG_ISO=${stamp% *}
  _PG_EPOCH=${stamp##* }
  if [ "${DG_TEST:-}" = 1 ] && [ -n "${DG_NOW:-}" ] && [ -r "$DG_NOW" ]; then
    IFS= read -r e <"$DG_NOW" || true
    if _pg_digits "$e"; then
      _PG_EPOCH=$e
    fi
  fi
  return 0
}

# _pg_bytes_load : sets _PG_BYTES, the approximate log size, from the counter
# file DG_HOME/log.bytes (builtins only). Only when that file is missing or
# garbled does one stat seed the counter from the log, and only if the log
# exists; a missing log is size 0 with no stat at all.
_pg_bytes_load() {
  local line="" size=""
  _PG_BYTES=0
  if [ -r "$DG_HOME/log.bytes" ] && IFS= read -r line <"$DG_HOME/log.bytes" && _pg_digits "$line"; then
    _PG_BYTES=$line
    return 0
  fi
  if [ -f "$_PG_LOG" ]; then
    size=$(stat -f %z "$_PG_LOG" 2>/dev/null) || size=""
    if _pg_digits "$size"; then
      _PG_BYTES=$size
    fi
  fi
  return 0
}

# _pg_bytes_save : writes _PG_BYTES back to DG_HOME/log.bytes (best effort).
_pg_bytes_save() {
  printf '%s\n' "$_PG_BYTES" >"$DG_HOME/log.bytes" 2>/dev/null || true
  return 0
}

# _pg_emit LINE : appends LINE and a newline to the log, then adds the length
# to _PG_BYTES. ${#line} counts characters; the guard writes ASCII names, and
# the counter is an approximation anyway.
_pg_emit() {
  printf '%s\n' "$1" >>"$_PG_LOG"
  _PG_BYTES=$((_PG_BYTES + ${#1} + 1))
  return 0
}

# _pg_rotate : when the counter reaches DG_ROTATE_BYTES (test knob) or 5 MB,
# keeps .1 and .2 and drops the older copy, then resets the counter to 0.
_pg_rotate() {
  local max=$_PG_DEFAULT_ROTATE
  if [ "${DG_TEST:-}" = 1 ] && _dg_is_uint "${DG_ROTATE_BYTES:-}"; then
    max=$DG_ROTATE_BYTES
  fi
  if [ "$_PG_BYTES" -lt "$max" ]; then
    return 0
  fi
  if [ -f "$_PG_LOG" ]; then
    rm -f -- "$_PG_LOG.2"
    if [ -f "$_PG_LOG.1" ]; then
      mv -- "$_PG_LOG.1" "$_PG_LOG.2"
    fi
    mv -- "$_PG_LOG" "$_PG_LOG.1"
  fi
  _PG_BYTES=0
  return 0
}

# _pg_orphan_summary FILE : FILE holds "ppid comm" lines. Sets the number of
# ppid==1 rows and up to five distinct command basenames. No arguments.
_pg_orphan_summary() {
  local pp comm nd=0
  _PG_ORPH_TOTAL=0
  _PG_ORPH_NAMES=""
  _PG_SEEN="|"
  while read -r pp comm; do
    if [ "$pp" = 1 ]; then
      _PG_ORPH_TOTAL=$((_PG_ORPH_TOTAL + 1))
      comm=${comm##*/}
      case "$_PG_SEEN" in
        *"|$comm|"*) ;;
        *)
          if [ "$nd" -lt 5 ]; then
            _PG_ORPH_NAMES="${_PG_ORPH_NAMES:+$_PG_ORPH_NAMES,}$comm"
            nd=$((nd + 1))
          fi
          _PG_SEEN="$_PG_SEEN$comm|"
          ;;
      esac
    fi
  done <"$1" 2>/dev/null
  return 0
}

# _pg_diag : the top parent/command table and an orphan summary, at most once
# per 60 s. Six helpers run one after another; each writes a fixed stage file
# in DG_HOME, so no helper runs at the same time as another.
_pg_diag() {
  local last=0 f="$DG_HOME/.last-diag" s1 s2 s3 s4 s5 s6 line
  if [ -f "$f" ]; then
    IFS= read -r last <"$f" || last=0
  fi
  _pg_digits "$last" || last=0
  if [ $((_PG_EPOCH - last)) -lt "$_PG_DIAG_GAP" ]; then
    return 0
  fi
  printf '%s\n' "$_PG_EPOCH" >"$f"
  s1="$DG_HOME/.diag-1"
  s2="$DG_HOME/.diag-2"
  s3="$DG_HOME/.diag-3"
  s4="$DG_HOME/.diag-4"
  s5="$DG_HOME/.diag-5"
  s6="$DG_HOME/.diag-6"
  ps -A -o ppid=,comm= >"$s1" 2>/dev/null
  sort "$s1" >"$s2" 2>/dev/null
  uniq -c "$s2" >"$s3" 2>/dev/null
  sort -rn "$s3" >"$s4" 2>/dev/null
  head -n 8 "$s4" >"$s5" 2>/dev/null
  ps -u "$UID" -o ppid=,comm= >"$s6" 2>/dev/null
  _pg_orphan_summary "$s6"
  _pg_emit "$_PG_ISO DIAG level=$_PG_LEVEL"
  while IFS= read -r line; do
    _pg_emit "$_PG_ISO DIAG $line"
  done <"$s5"
  _pg_emit "$_PG_ISO DIAG ppid1_total=$_PG_ORPH_TOTAL names=$_PG_ORPH_NAMES"
  return 0
}

# _pg_alert : at the hard level, one ALERT line and one notification at most
# per 600 s. The notification is best effort.
_pg_alert() {
  local last=0 f="$DG_HOME/.last-notify"
  if [ -f "$f" ]; then
    IFS= read -r last <"$f" || last=0
  fi
  _pg_digits "$last" || last=0
  if [ $((_PG_EPOCH - last)) -lt "$_PG_NOTIFY_GAP" ]; then
    return 0
  fi
  printf '%s\n' "$_PG_EPOCH" >"$f"
  _pg_emit "$_PG_ISO ALERT level=hard count=$_PG_COUNT"
  osascript -e "display notification \"process count $_PG_COUNT is at the hard level\" with title \"dev-guard\"" \
    >/dev/null 2>&1 || true
  return 0
}

# _pg_tick : one sample. Ordinary ticks run ps and date only; the log size comes
# from the DG_HOME/log.bytes counter. Warn and hard add the diagnostic block.
# Hard adds the ALERT line. A failed date is still an unknown sample (no clock,
# so no diagnostic or ALERT). Nothing is killed; only names and counts are written.
_pg_tick() {
  local sample_rc=0 now_rc=0
  _PG_UNKNOWN=0
  _pg_sample || sample_rc=$?
  _pg_bytes_load
  _pg_now || now_rc=1
  _pg_rotate
  if [ "$now_rc" -ne 0 ]; then
    _PG_UNKNOWN=1
    _PG_ISO=unknown-time
    _pg_emit "$_PG_ISO unknown"
    _pg_bytes_save
    return 0
  fi
  if [ "$sample_rc" -ne 0 ]; then
    _PG_UNKNOWN=1
    _pg_emit "$_PG_ISO unknown"
    _pg_bytes_save
    return 0
  fi
  _pg_emit "$_PG_ISO $_PG_COUNT $_PG_LEVEL"
  case "$_PG_LEVEL" in
    warn|hard) _pg_diag ;;
  esac
  if [ "$_PG_LEVEL" = hard ]; then
    _pg_alert
  fi
  _pg_bytes_save
  return 0
}

# _pg_check : one read-only sample, printed as procs=N level=L.
_pg_check() {
  local code
  _pg_sample
  code=$(dg_level_code "$_PG_LEVEL")
  echo "procs=$_PG_COUNT level=$_PG_LEVEL"
  return "$code"
}

# _pg_owner_fields : sets _PG_OPID, _PG_OTOK, _PG_OLST from the watcher's
# owner record (pid, token, lstart). 1 when the record is missing or malformed.
_pg_owner_fields() {
  local f="$DG_HOME/locks/watch.lock/owner"
  _PG_OPID=""
  _PG_OTOK=""
  _PG_OLST=""
  [ -r "$f" ] || return 1
  read -r _PG_OPID _PG_OTOK _PG_OLST <"$f" || true
  if ! _pg_digits "$_PG_OPID" || [ -z "$_PG_OTOK" ] || [ -z "$_PG_OLST" ]; then
    return 1
  fi
  return 0
}

# _pg_lstart PID : sets _PG_LST_CUR to the start time, C locale as in lib.sh.
_pg_lstart() {
  local raw=""
  _PG_LST_CUR=""
  raw=$(LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null) || return 1
  read -r _PG_LST_CUR <<<"$raw" || true
  [ -n "$_PG_LST_CUR" ]
}

# _pg_watch_live : 0 only when the owner record names a live process that
# started at the recorded time.
_pg_watch_live() {
  _pg_owner_fields || return 1
  kill -0 "$_PG_OPID" 2>/dev/null || return 1
  _pg_lstart "$_PG_OPID" || return 1
  [ "$_PG_LST_CUR" = "$_PG_OLST" ]
}

# _pg_watch_start : starts the loop, which takes the lock itself, so the
# owner record names the loop's own pid. Then waits (bounded) for that record.
_pg_watch_start() {
  local child i=0
  if _pg_watch_live; then
    echo "already running pid $_PG_OPID"
    return 0
  fi
  "$_PG_SELF" _watch-loop </dev/null >/dev/null 2>&1 &
  child=$!
  while [ "$i" -lt 20 ]; do
    if _pg_owner_fields && [ "$_PG_OPID" = "$child" ]; then
      echo "started pid $child"
      return 0
    fi
    sleep 0.5
    i=$((i + 1))
  done
  if _pg_watch_live; then
    echo "already running pid $_PG_OPID"
    return 0
  fi
  echo "watch start failed: no owner record for pid $child" >&2
  return 1
}

# _pg_watch_status : exit 0 and "running pid N" when a watcher is live.
_pg_watch_status() {
  if _pg_watch_live; then
    echo "running pid $_PG_OPID"
    return 0
  fi
  echo "not running"
  return 1
}

# _pg_watch_stop : signals the watcher only after its owner record, start time
# and command line all check out. Escalates to KILL only for that verified pid.
_pg_watch_stop() {
  local cmd="" i=0
  if ! _pg_owner_fields; then
    echo "not running"
    return 0
  fi
  if ! kill -0 "$_PG_OPID" 2>/dev/null; then
    echo "not running (owner record for pid $_PG_OPID is stale)"
    return 0
  fi
  if ! _pg_lstart "$_PG_OPID" || [ "$_PG_LST_CUR" != "$_PG_OLST" ]; then
    echo "not running (owner record for pid $_PG_OPID does not match a live process)"
    return 0
  fi
  cmd=$(ps -p "$_PG_OPID" -o command= 2>/dev/null)
  case "$cmd" in
    *proc-guard.sh*) ;;
    *)
      echo "watch stop refused: pid $_PG_OPID is not a proc-guard watcher" >&2
      return 1
      ;;
  esac
  kill -TERM "$_PG_OPID" 2>/dev/null
  while kill -0 "$_PG_OPID" 2>/dev/null && [ "$i" -lt 25 ]; do
    sleep 0.2
    i=$((i + 1))
  done
  if kill -0 "$_PG_OPID" 2>/dev/null; then
    # Re-check the start time right before KILL: the pid may have exited and
    # been reused by another process. Skip the KILL when it no longer matches.
    if _pg_lstart "$_PG_OPID" && [ "$_PG_LST_CUR" = "$_PG_OLST" ]; then
      kill -KILL "$_PG_OPID" 2>/dev/null
    fi
  fi
  echo "stopped pid $_PG_OPID"
  return 0
}

# _pg_owner_is_mine TOKEN : 0 while the owner record still names this loop.
_pg_owner_is_mine() {
  _pg_owner_fields || return 1
  [ "$_PG_OPID" = "$$" ] && [ "$_PG_OTOK" = "$1" ]
}

# _pg_nap SECONDS : waits with builtin timed reads on an O_RDWR fifo, so no
# helper process is started while the watcher waits.
_pg_nap() {
  local i=0 junk=""
  while [ "$i" -lt "$1" ]; do
    read -r -t 1 -u 3 junk || true
    i=$((i + 1))
  done
  return 0
}

# _pg_loop_exit : EXIT trap for the watcher. Releases the lock only with our
# token, then removes the private fifo directory.
_pg_loop_exit() {
  if [ -n "$_PG_LOOP_TOK" ]; then
    dg_own_release "$_PG_LOCKDIR" "$_PG_LOOP_TOK" 2>/dev/null
  fi
  if [ -n "$_PG_RUNDIR" ]; then
    rm -rf -- "$_PG_RUNDIR"
  fi
  return 0
}

# _pg_watch_loop : the background watcher. Takes the single-instance lock
# (lib.sh ownership protocol), then ticks forever. The delay doubles after
# unknown samples (up to 120 s) and resets after a good one. Stops when the
# owner record no longer names it.
_pg_watch_loop() {
  local tok="" base cur rc
  _pg_interval
  base=$_PG_INTERVAL
  cur=$base
  _PG_LOCKDIR="$DG_HOME/locks/watch.lock"
  if ! mkdir -p -m 700 -- "$DG_HOME/locks" 2>/dev/null; then
    exit 1
  fi
  dg_own_acquire "$_PG_LOCKDIR" tok
  rc=$?
  if [ "$rc" -ne 0 ]; then
    exit 1
  fi
  _PG_LOOP_TOK=$tok
  trap _pg_loop_exit EXIT
  _PG_RUNDIR=$(mktemp -d "${TMPDIR:-/tmp}/dgwatch.XXXXXX") || exit 1
  if ! mkfifo "$_PG_RUNDIR/nap"; then
    exit 1
  fi
  if ! exec 3<>"$_PG_RUNDIR/nap"; then
    exit 1
  fi
  trap 'exit 0' TERM INT HUP
  while _pg_owner_is_mine "$tok"; do
    _pg_tick
    if [ "$_PG_UNKNOWN" -eq 1 ]; then
      cur=$((cur * 2))
      if [ "$cur" -gt "$_PG_BACKOFF_MAX" ]; then
        cur=$_PG_BACKOFF_MAX
      fi
    else
      cur=$base
    fi
    if [ "${DG_TEST:-}" = 1 ] && [ -n "${DG_DEBUG_DELAYS:-}" ]; then
      printf '%s\n' "$cur" >>"$DG_DEBUG_DELAYS"
    fi
    _pg_nap "$cur"
  done
  exit 0
}

# _pg_orphans : lists this user's ppid==1 processes by command name. Read only.
_pg_orphans() {
  local out="" names=""
  if ! out=$(ps -u "$UID" -o ppid=,comm= 2>/dev/null); then
    echo "orphans: cannot list processes (ps failed)" >&2
    return 1
  fi
  names=$(printf '%s\n' "$out" | while read -r pp comm; do
    if [ "$pp" = 1 ]; then
      printf '%s\n' "${comm##*/}"
    fi
  done)
  echo "ppid==1 processes of this user, by command name (read only; nothing is killed):"
  if [ -z "$names" ]; then
    echo "  (none)"
    return 0
  fi
  printf '%s\n' "$names" | sort | uniq -c | sort -rn
  return 0
}

# _pg_report : sample count, maximum count and the last three ALERT lines.
_pg_report() {
  local line a b c samples=0 unknowns=0 max=0 nalerts=0 a1="" a2="" a3="" x
  if [ ! -f "$_PG_LOG" ]; then
    echo "no samples yet (no log at $_PG_LOG)"
    return 0
  fi
  while IFS= read -r line; do
    read -r a b c <<<"$line" || true
    if [ "$b" = ALERT ]; then
      nalerts=$((nalerts + 1))
      a3=$a2
      a2=$a1
      a1=$line
      continue
    fi
    if [ "$b" = unknown ]; then
      unknowns=$((unknowns + 1))
      continue
    fi
    if _pg_digits "$b"; then
      case "$c" in
        ok|warn|hard)
          samples=$((samples + 1))
          if [ "$b" -gt "$max" ]; then
            max=$b
          fi
          ;;
      esac
    fi
  done <"$_PG_LOG"
  echo "samples: $samples (unknown: $unknowns, ALERT: $nalerts)"
  echo "max count: $max"
  if [ "$nalerts" -eq 0 ]; then
    echo "ALERT lines: none"
  else
    echo "last ALERT lines:"
    for x in "$a3" "$a2" "$a1"; do
      if [ -n "$x" ]; then
        echo "  $x"
      fi
    done
  fi
  return 0
}

_pg_main() {
  case "${1:-}" in
    check)
      _pg_check
      exit $?
      ;;
    tick)
      _pg_home_setup
      _pg_tick
      exit $?
      ;;
    watch)
      case "${2:-}" in
        start)
          _pg_home_setup
          _pg_watch_start
          exit $?
          ;;
        stop)
          _pg_home_setup
          _pg_watch_stop
          exit $?
          ;;
        status)
          _pg_home_setup
          _pg_watch_status
          exit $?
          ;;
        *) _pg_usage "watch start|stop|status" ;;
      esac
      ;;
    _watch-loop)
      _pg_home_setup
      _pg_watch_loop
      ;;
    orphans)
      _pg_orphans
      exit $?
      ;;
    report)
      _pg_home_setup
      _pg_report
      exit $?
      ;;
    *) _pg_usage "check|tick|watch|orphans|report" ;;
  esac
}

_pg_main "$@"
