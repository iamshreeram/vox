#!/bin/bash
# dev-guard pure helpers. Sourced, never executed: defines functions only.
# Bash 3.2 compatible (macOS /bin/bash). Inputs are validated before any
# arithmetic, so a malformed value can never reach $(( )) unchecked.

# _dg_is_uint VALUE : 0 when VALUE is 1-9 ASCII digits. The length cap keeps
# later arithmetic far away from overflow.
_dg_is_uint() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "${#1}" -le 9 ]
}

# dg_count : prints this user's process count, or fails with no output.
# Fake knobs are honoured only under DG_TEST=1, so a stray environment
# variable can never fake the guard's reading in production.
dg_count() {
  local uid out line n=0
  if [ "$DG_TEST" = "1" ] && [ "$DG_FAKE_PS_FAIL" = "1" ]; then
    return 1
  fi
  if [ "$DG_TEST" = "1" ] && [ -n "$DG_FAKE_COUNT" ]; then
    [ -r "$DG_FAKE_COUNT" ] || return 1
    # read fails on an empty file, which is the failure we want.
    IFS= read -r line <"$DG_FAKE_COUNT" || return 1
    _dg_is_uint "$line" && [ "$line" -gt 0 ] || return 1
    printf '%s\n' "$line"
    return 0
  fi
  uid=$(id -u) || return 1
  out=$(ps -u "$uid" -o pid=) || return 1
  # A here-string, not a pipe: a pipe would run the loop in a subshell and
  # lose n. Only lines holding digits count, so blank output counts as zero.
  while IFS= read -r line; do
    case "$line" in
      *[0-9]*) n=$((n + 1)) ;;
    esac
  done <<<"$out"
  _dg_is_uint "$n" && [ "$n" -gt 0 ] || return 1
  printf '%s\n' "$n"
  return 0
}

# dg_level COUNT : prints ok|warn|hard. Thresholds come from DG_WARN and
# DG_HARD; a malformed value anywhere yields "unknown" so callers fail safe.
dg_level() {
  local count="$1" warn="${DG_WARN:-1200}" hard="${DG_HARD:-1800}"
  if ! _dg_is_uint "$count" || ! _dg_is_uint "$warn" || ! _dg_is_uint "$hard"; then
    echo unknown
    return 1
  fi
  if [ "$count" -ge "$hard" ]; then
    echo hard
  elif [ "$count" -ge "$warn" ]; then
    echo warn
  else
    echo ok
  fi
  return 0
}

# dg_level_code LEVEL : maps a level to an exit-style code. Anything that is
# not a known level maps to 30, so an unexpected value is never treated as ok.
dg_level_code() {
  case "$1" in
    ok) echo 0 ;;
    warn) echo 10 ;;
    hard) echo 20 ;;
    *) echo 30 ;;
  esac
}

# dg_ulimit_value COUNT HEADROOM RESERVE SOFT : prints the process limit to
# set, min(COUNT+HEADROOM, SOFT), when it leaves at least RESERVE spare slots
# above COUNT. Otherwise prints nothing and returns 75 (refuse). Returns 64 on
# malformed arguments. SOFT may be "unlimited", which is resolved through
# sysctl so the per-user cap is never guessed.
dg_ulimit_value() {
  local count="$1" headroom="$2" reserve="$3" soft="$4" limit
  if [ "$#" -ne 4 ]; then
    return 64
  fi
  _dg_is_uint "$count" && _dg_is_uint "$headroom" && _dg_is_uint "$reserve" || return 64
  if [ "$soft" = "unlimited" ]; then
    soft=$(sysctl -n kern.maxprocperuid 2>/dev/null) || return 75
    _dg_is_uint "$soft" || return 75
  elif ! _dg_is_uint "$soft"; then
    return 64
  fi
  # A zero count is not a real measurement, so there is nothing to budget for.
  if [ "$count" -eq 0 ]; then
    return 75
  fi
  limit=$((count + headroom))
  if [ "$soft" -lt "$limit" ]; then
    limit=$soft
  fi
  if [ "$limit" -ge $((count + reserve)) ]; then
    printf '%s\n' "$limit"
    return 0
  fi
  return 75
}

# dg_validate_name NAME : 0 only for 1-32 chars from [A-Za-z0-9_-]. The
# bracket negation rejects newlines and slashes, which a loose regex can miss.
dg_validate_name() {
  # C locale: in a UTF-8 locale, [A-Z] ranges can match accented letters.
  local LC_ALL=C
  case "$1" in
    ''|*[!A-Za-z0-9_-]*) return 1 ;;
  esac
  [ "${#1}" -le 32 ]
}

# dg_home_check DIR : 0 only when DIR is a real directory (not a symlink)
# owned by the current user with mode exactly 0700. Anything else is refused,
# because a state dir that others can write to cannot be trusted.
dg_home_check() {
  local dir="$1" owner mode
  [ -n "$dir" ] && [ -d "$dir" ] && [ ! -L "$dir" ] || return 1
  owner=$(stat -f %u "$dir") || return 1
  mode=$(stat -f %Lp "$dir") || return 1
  [ "$owner" = "$(id -u)" ] && [ "$mode" = "700" ]
}

# ---------------------------------------------------------------------------
# Ownership/lock protocol (design R5' / R5''). Bash 3.2 compatible.
#
# A lock is the directory DIR. Exclusive acquisition is a plain `mkdir DIR`.
# The owner record DIR/owner is one line `PID TOKEN LSTART`: written to
# owner.tmp inside DIR, then renamed to owner. PID is $$ of the acquiring
# shell. Bash 3.2 has no BASHPID, so a subshell reports its parent's $$;
# acquire and release must therefore run in the shell that owns the lock.
# LSTART is `ps -o lstart=` of PID in the C locale (_dg_own_lstart).
#
# States (_dg_own_state): none; init (no readable owner record: never
# reclaimed); live; unknown (identity unreadable: never reclaimed); stale
# (pid dead, or live pid whose lstart differs from the record).
#
# Test hooks DG_PAUSE_<name> are honoured only when DG_TEST=1. A hook spins
# on a private fifo (builtin read -t 1) while its named file exists.
# Internal names use the _dg_own / _DG_ prefix; the VARNAME given to
# dg_own_acquire must not start with _dg_.

_DG_NAP_DIR=""
_DG_ABS=""
_DG_PARENT=""
_DG_NAME=""
_DG_TOK=""
_DG_WAIT=600
_DG_STATE=""
_DG_R_PID=""
_DG_R_TOK=""
_DG_R_LST=""
_DG_CUR_LST=""
_DG_JPID=""
_DG_JTOK=""

# _dg_own_nap_open : creates a private fifo and opens it on fd 8 (O_RDWR,
# so reads block until the timeout instead of seeing EOF).
_dg_own_nap_open() {
  local d
  d=$(mktemp -d "${TMPDIR:-/tmp}/dgnap.XXXXXX") || return 1
  if ! mkfifo "$d/f" 2>/dev/null; then
    rm -rf -- "$d"
    return 1
  fi
  if ! exec 8<>"$d/f"; then
    rm -rf -- "$d"
    return 1
  fi
  _DG_NAP_DIR=$d
  return 0
}

# _dg_own_nap : waits one second. Never sleeps. Fails when no fd is available.
_dg_own_nap() {
  if [ -z "$_DG_NAP_DIR" ]; then
    _dg_own_nap_open || return 1
  fi
  read -t 1 -u 8 _dg_nap_x
  return 0
}

_dg_own_nap_close() {
  if [ -n "$_DG_NAP_DIR" ]; then
    exec 8>&-
    rm -rf -- "$_DG_NAP_DIR"
    _DG_NAP_DIR=""
  fi
  return 0
}

# _dg_own_pause HOOKVAR : honoured only under DG_TEST=1.
_dg_own_pause() {
  local _hook
  if [ "$DG_TEST" != "1" ]; then
    return 0
  fi
  eval "_hook=\${$1:-}"
  if [ -z "$_hook" ]; then
    return 0
  fi
  while [ -e "$_hook" ]; do
    _dg_own_nap || return 1
  done
  return 0
}

# _dg_own_lstart PID : sets _DG_CUR_LST; 1 when the identity is unreadable.
_dg_own_lstart() {
  local raw
  _DG_CUR_LST=""
  if [ "$DG_TEST" = "1" ] && [ "$DG_FAKE_PS_FAIL" = "1" ]; then
    return 1
  fi
  raw=$(LC_ALL=C ps -o lstart= -p "$1" 2>/dev/null) || return 1
  # read with the default IFS trims leading and trailing blanks.
  read -r _DG_CUR_LST <<<"$raw"
  [ -n "$_DG_CUR_LST" ]
}

# _dg_own_token : sets _DG_TOK to a token unique enough for one lock.
_dg_own_token() {
  _DG_TOK="$$.$RANDOM$RANDOM$RANDOM.$(date +%s)"
}

# _dg_own_read DIR : sets _DG_R_PID/_DG_R_TOK/_DG_R_LST from DIR/owner.
# 1 when there is no readable, well-formed owner record.
_dg_own_read() {
  local line rest
  _DG_R_PID=""
  _DG_R_TOK=""
  _DG_R_LST=""
  [ -r "$1/owner" ] || return 1
  IFS= read -r line <"$1/owner" || [ -n "$line" ] || return 1
  case "$line" in
    *' '*) ;;
    *) return 1 ;;
  esac
  rest=${line#* }
  case "$rest" in
    *' '*) ;;
    *) return 1 ;;
  esac
  _DG_R_PID=${line%% *}
  _DG_R_TOK=${rest%% *}
  _DG_R_LST=${rest#* }
  if ! _dg_is_uint "$_DG_R_PID" || [ -z "$_DG_R_TOK" ] || [ -z "$_DG_R_LST" ]; then
    return 1
  fi
  return 0
}

# _dg_own_state DIR : sets _DG_STATE (none|init|live|unknown|stale) and the
# _DG_R_* record fields. Never modifies anything.
_dg_own_state() {
  _DG_STATE=""
  if [ ! -d "$1" ]; then
    if [ -e "$1" ] || [ -L "$1" ]; then
      _DG_STATE=init
    else
      _DG_STATE=none
    fi
    return 0
  fi
  if ! _dg_own_read "$1"; then
    _DG_STATE=init
    return 0
  fi
  if ! kill -0 "$_DG_R_PID" 2>/dev/null; then
    _DG_STATE=stale
    return 0
  fi
  if ! _dg_own_lstart "$_DG_R_PID"; then
    _DG_STATE=unknown
    return 0
  fi
  if [ "$_DG_CUR_LST" = "$_DG_R_LST" ]; then
    _DG_STATE=live
  else
    _DG_STATE=stale
  fi
  return 0
}

# _dg_own_split ABS : sets _DG_PARENT (dirname) and _DG_NAME (basename
# without .lock). 64 for a path with no usable name or a glob character.
_dg_own_split() {
  local base
  _DG_PARENT=${1%/*}
  if [ -z "$_DG_PARENT" ]; then
    _DG_PARENT=/
  fi
  base=${1##*/}
  _DG_NAME=${base%.lock}
  case "$_DG_NAME" in
    ''|*'*'*|*'?'*|*'['*) return 64 ;;
  esac
  return 0
}

# _dg_own_abs DIR : sets _DG_ABS to the absolute path, captured once.
_dg_own_abs() {
  case "$1" in
    /*) _DG_ABS=$1 ;;
    *) _DG_ABS=$PWD/$1 ;;
  esac
}

# _dg_own_wait_secs : sets _DG_WAIT from DG_LOCK_WAIT (default 600).
_dg_own_wait_secs() {
  if _dg_is_uint "${DG_LOCK_WAIT:-600}"; then
    _DG_WAIT=${DG_LOCK_WAIT:-600}
  else
    _DG_WAIT=600
  fi
}

# _dg_own_publish ABS TOKEN : writes the owner record. Re-checks, just before
# writing, that ABS still exists and has no owner file, else writes nothing.
_dg_own_publish() {
  if [ ! -d "$1" ] || [ -e "$1/owner" ]; then
    return 1
  fi
  if ! _dg_own_lstart "$$"; then
    rmdir -- "$1" 2>/dev/null
    return 1
  fi
  if ! printf '%s %s %s\n' "$$" "$2" "$_DG_CUR_LST" >"$1/owner.tmp"; then
    rm -f -- "$1/owner.tmp"
    return 1
  fi
  if ! mv -- "$1/owner.tmp" "$1/owner"; then
    rm -f -- "$1/owner.tmp"
    return 1
  fi
  return 0
}

# _dg_own_claim_once ABS : mkdir, then publish. 0 = owned (_DG_TOK set),
# 1 = mkdir failed (someone else created it), 2 = publish aborted.
_dg_own_claim_once() {
  if ! mkdir -- "$1" 2>/dev/null; then
    return 1
  fi
  _dg_own_token
  _dg_own_pause DG_PAUSE_after_mkdir
  if ! _dg_own_publish "$1" "$_DG_TOK"; then
    return 2
  fi
  return 0
}

# _dg_own_retire ABS PID TOKEN : renames ABS to NAME.dead.PID.TOKEN and
# removes it, but only while its record still carries TOKEN.
_dg_own_retire() {
  local dead
  _dg_own_read "$1" && [ "$_DG_R_TOK" = "$3" ] || return 1
  _dg_own_split "$1" || return 1
  dead="$_DG_PARENT/$_DG_NAME.dead.$2.$3"
  rm -rf -- "$dead"
  if ! mv -- "$1" "$dead"; then
    return 1
  fi
  rm -rf -- "$dead"
  return 0
}

# _dg_own_init_msg ABS ALLOW_RECLAIM : explains a dir stuck without a record.
_dg_own_init_msg() {
  local home path
  if [ "$2" = 1 ]; then
    if [ -n "$DG_HOME" ]; then
      home=$DG_HOME
    else
      home=${_DG_PARENT%/*}
    fi
    path="$home/locks/$_DG_NAME.lock"
  else
    path=$1
  fi
  printf 'lock %s has no owner record; if no guard process is running, remove %s manually\n' "$_DG_NAME" "$path" >&2
}

# _dg_own_lock ABS ALLOW_RECLAIM : 0 = acquired (_DG_TOK set); 1 = refused;
# 2 = stale, reclaim needed (ALLOW_RECLAIM=1; _DG_JPID/_DG_JTOK set);
# 75 = still initializing after DG_LOCK_WAIT seconds.
_dg_own_lock() {
  local tries=0 waited=0 rc
  _dg_own_split "$1" || return 1
  while :; do
    _dg_own_state "$1"
    case "$_DG_STATE" in
      none)
        tries=$((tries + 1))
        if [ "$tries" -gt 50 ]; then
          return 1
        fi
        _dg_own_claim_once "$1"
        rc=$?
        if [ "$rc" -eq 0 ]; then
          return 0
        fi
        if [ "$rc" -ne 1 ]; then
          return 1
        fi
        ;;
      stale)
        if [ "$2" = 1 ]; then
          _DG_JPID=$_DG_R_PID
          _DG_JTOK=$_DG_R_TOK
          return 2
        fi
        tries=$((tries + 1))
        if [ "$tries" -gt 50 ]; then
          return 1
        fi
        _dg_own_retire "$1" "$_DG_R_PID" "$_DG_R_TOK"
        ;;
      init)
        if [ "$waited" -ge "$_DG_WAIT" ]; then
          _dg_own_init_msg "$1" "$2"
          return 75
        fi
        _dg_own_nap || return 1
        waited=$((waited + 1))
        ;;
      *)
        return 1
        ;;
    esac
  done
}

# _dg_own_clean PARENT NAME : removes leftover NAME.dead.*, NAME.rel.* and
# NAME.tmp.* entries. Anything ending in .lock is a live lock and is kept.
_dg_own_clean() {
  local s
  for s in "$1/$2".dead.* "$1/$2".rel.* "$1/$2".tmp.*; do
    case "$s" in
      *.lock) continue ;;
    esac
    if [ -e "$s" ] || [ -L "$s" ]; then
      rm -rf -- "$s"
    fi
  done
  return 0
}

# _dg_own_reclaim_checked ABS JPID JTOK : runs under the reclaim lock. The
# record must still carry JTOK (or ABS must already be gone); then retire.
_dg_own_reclaim_checked() {
  if [ -d "$1" ]; then
    _dg_own_read "$1" && [ "$_DG_R_TOK" = "$3" ] || return 1
  fi
  _dg_own_pause DG_PAUSE_reclaim_judged
  if [ -d "$1" ]; then
    _dg_own_retire "$1" "$2" "$3" || return 1
  fi
  return 0
}

# _dg_own_reclaim ABS JPID JTOK : serialises reclaim of a judged-stale ABS
# through the reclaim lock DIR/.reclaim-NAME. 0 = ABS removed (caller retries
# mkdir once); 1 = refused; 75 = reclaim lock stuck initializing.
_dg_own_reclaim() {
  local rl rltok rc
  _dg_own_split "$1" || return 1
  rl="$_DG_PARENT/.reclaim-$_DG_NAME"
  _dg_own_lock "$rl" 0
  rc=$?
  if [ "$rc" -ne 0 ]; then
    return "$rc"
  fi
  rltok=$_DG_TOK
  _dg_own_reclaim_checked "$1" "$2" "$3"
  rc=$?
  _dg_own_drop "$rl" "$rltok"
  return "$rc"
}

# _dg_own_drop ABS TOKEN : unpaused release. Re-reads the token immediately
# before the rename; renames to NAME.rel.PID and removes only on a match.
_dg_own_drop() {
  local rel
  _dg_own_split "$1" || return 1
  rel="$_DG_PARENT/$_DG_NAME.rel.$$"
  rm -rf -- "$rel"
  _dg_own_read "$1" || return 1
  if [ "$_DG_R_TOK" != "$2" ]; then
    return 1
  fi
  if ! mv -- "$1" "$rel"; then
    return 1
  fi
  rm -rf -- "$rel"
  return 0
}

# _dg_own_acquire_impl DIR VARNAME : see dg_own_acquire.
_dg_own_acquire_impl() {
  local _dg_d="$1" _dg_v="$2" _dg_rc
  case "$_dg_v" in
    ''|[0-9]*|*[!A-Za-z0-9_]*) return 64 ;;
  esac
  if [ -z "$_dg_d" ]; then
    return 64
  fi
  _dg_own_abs "$_dg_d"
  _dg_own_split "$_DG_ABS" || return 64
  _dg_own_wait_secs
  _dg_own_clean "$_DG_PARENT" "$_DG_NAME"
  _dg_own_clean "$_DG_PARENT" ".reclaim-$_DG_NAME"
  _dg_own_lock "$_DG_ABS" 1
  _dg_own_rc=$?
  if [ "$_dg_own_rc" -eq 2 ]; then
    _dg_own_reclaim "$_DG_ABS" "$_DG_JPID" "$_DG_JTOK"
    _dg_own_rc=$?
    if [ "$_dg_own_rc" -eq 0 ]; then
      if _dg_own_claim_once "$_DG_ABS"; then
        _dg_own_rc=0
      else
        _dg_own_rc=1
      fi
    fi
  fi
  if [ "$_dg_own_rc" -eq 0 ]; then
    eval "$_dg_v=\$_DG_TOK"
  fi
  return "$_dg_own_rc"
}

# dg_own_acquire DIR VARNAME : 0 = acquired (VARNAME set to the token);
# 1 = held by a live or unknown owner; 75 = no owner record after DG_LOCK_WAIT
# seconds (message on stderr); 64 = bad arguments.
dg_own_acquire() {
  local _dg_w_rc
  _dg_own_acquire_impl "$@"
  _dg_w_rc=$?
  _dg_own_nap_close
  return "$_dg_w_rc"
}

# dg_own_status DIR : prints owner PID TOKEN, stale, initializing or none.
# Always returns 0 and never reclaims.
dg_own_status() {
  if [ -z "$1" ]; then
    return 0
  fi
  _dg_own_abs "$1"
  _dg_own_split "$_DG_ABS" || return 0
  _dg_own_state "$_DG_ABS"
  case "$_DG_STATE" in
    none) echo none ;;
    init) echo initializing ;;
    stale) echo stale ;;
    *) echo "owner $_DG_R_PID $_DG_R_TOK" ;;
  esac
  return 0
}

# dg_own_release DIR TOKEN : 0 = released (DIR removed); 1 = token mismatch,
# unreadable record, or bad arguments; DIR is left untouched in those cases.
dg_own_release() {
  local _dg_rc=1 _dg_abs
  if [ -n "$1" ] && [ -n "$2" ]; then
    _dg_own_abs "$1"
    _dg_abs=$_DG_ABS
    _dg_own_pause DG_PAUSE_before_release
    _dg_own_drop "$_dg_abs" "$2"
    _dg_rc=$?
  fi
  _dg_own_nap_close
  return "$_dg_rc"
}
