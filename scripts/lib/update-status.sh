# update-status.sh - progress status file and build-output parser for
# scripts/update-macos.sh.
#
# Meant to be SOURCED (bash 3.2 compatible). Defines functions and globals
# only; no top-level side effects, no `set` changes, no `exit`.
#
# Active only when VOX_UPDATE_STATUS_FILE is set (absolute path, parent
# directory must exist). When it is unset every function here is a no-op or
# returns without output, so the script's default behaviour is unchanged.
#
# Status file (JSON, one object, rewritten atomically: temp file in the same
# directory, then mv):
#   {"v":1,"run_id":"<16 hex>","phase":"<phase>","percent":<0..100>,
#    "target":"<7 hex>","target_full":"<40 hex or empty>",
#    "message":"<<=120 chars, JSON-escaped>","method":"exact|estimated",
#    "ts":<unix seconds>}
# phase is one of: preparing dependencies compiling bundling installing
#                  restarting done failed waiting
# waiting is a RESTING, non-terminal phase: a build-only run (VOX_UPDATE_PHASE
# =build) has staged its app and stops there until an install is started.
# percent is monotonic within a run, never 100 before `done`.
#
# Bands: preparing 0-4, dependencies 4-10, compiling 10-87 (cargo N/M is
# primary; a time-based estimate takes over after VOX_UPDATE_ESTIMATE_AFTER
# seconds, default 20, without a new N/M), bundling 88-92, waiting 91,
# installing 92-96, restarting 97-99, done 100, failed keeps the last percent.
#
# Environment read here:
#   VOX_UPDATE_STATUS_FILE   absolute path of the status file (enables it)
#   VOX_UPDATE_RUN_ID        16 lowercase hex characters (required with it)
#   VOX_UPDATE_STATE_DIR     absolute dir for last_build_seconds (default: the
#                            directory of the status file)
#   VOX_UPDATE_STATUS_HISTORY optional path; every written status line is
#                            appended there (diagnostics and tests)
#   VOX_UPDATE_ESTIMATE_AFTER seconds without N/M before estimating (default 20)

STATUS_LAST_PERCENT=0
STATUS_TERMINAL=0
STATUS_TARGET_SHORT="0000000"
STATUS_TARGET_FULL=""
STATUS_FAIL_REASON=""
STATUS_LAST_WRITE_MS=0
PROG_N=0
PROG_M=0

# status_enabled: 0 when the status file is requested.
status_enabled() {
  [ -n "${VOX_UPDATE_STATUS_FILE:-}" ]
}

# status_hex_of_len <value> <length>: 0 iff value is exactly <length> lowercase
# hex characters.
status_hex_of_len() {
  case "$1" in
    "" | *[!0-9a-f]*) return 1 ;;
  esac
  [ "${#1}" -eq "$2" ]
}

# status_validate_setup: checks the status environment. Prints a message and
# returns 1 on a bad setup. Returns 0 when status is disabled.
status_validate_setup() {
  local dir
  if ! status_enabled; then
    return 0
  fi
  case "$VOX_UPDATE_STATUS_FILE" in
    /*) ;;
    *)
      echo "VOX_UPDATE_STATUS_FILE must be an absolute path." >&2
      return 1
      ;;
  esac
  dir="$(dirname "$VOX_UPDATE_STATUS_FILE")"
  if [ ! -d "$dir" ]; then
    echo "VOX_UPDATE_STATUS_FILE parent directory does not exist: $dir" >&2
    return 1
  fi
  if ! status_hex_of_len "${VOX_UPDATE_RUN_ID:-}" 16; then
    echo "VOX_UPDATE_RUN_ID must be 16 lowercase hex characters." >&2
    return 1
  fi
  if [ -n "${VOX_UPDATE_STATE_DIR:-}" ]; then
    case "$VOX_UPDATE_STATE_DIR" in
      /*) ;;
      *)
        echo "VOX_UPDATE_STATE_DIR must be an absolute path." >&2
        return 1
        ;;
    esac
  fi
  return 0
}

# status_set_target <full-or-empty> <short-fallback>: sets the target fields
# used by later status writes. The short form is always 7 hex characters.
status_set_target() {
  local full="$1" short="$2"
  if status_hex_of_len "$full" 40; then
    STATUS_TARGET_FULL="$full"
    STATUS_TARGET_SHORT="$(printf '%s' "$full" | cut -c1-7)"
  else
    STATUS_TARGET_FULL=""
    if status_hex_of_len "$short" 7; then
      STATUS_TARGET_SHORT="$short"
    else
      STATUS_TARGET_SHORT="0000000"
    fi
  fi
}

# status_json_escape <text>: strips control characters, truncates to 120
# characters, escapes backslash and double quote. Prints the result.
status_json_escape() {
  local text
  text="$(printf '%s' "$1" | LC_ALL=C tr -d '\000-\037\177')"
  text="${text:0:120}"
  printf '%s' "$text" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# status_phase_valid <phase>: 0 iff the phase is one of the known phases.
status_phase_valid() {
  case "$1" in
    preparing | dependencies | compiling | bundling | installing | restarting | done | failed | waiting) return 0 ;;
  esac
  return 1
}

# status_write <phase> <percent> <message> [exact|estimated]
# Writes the status file atomically. Percent is clamped to the monotonic and
# never-100-before-done rules. Failures are reported on stderr and never abort
# the caller. Writes after a terminal phase are ignored. Unknown phases are
# refused (nothing is written).
status_write() {
  local phase="$1" pct="$2" msg="$3" method="${4:-exact}"
  local file dir tmp ts esc line
  if ! status_enabled || [ "$STATUS_TERMINAL" = 1 ]; then
    return 0
  fi
  if ! status_phase_valid "$phase"; then
    echo "Internal error: unknown status phase '$phase' -- not written." >&2
    return 0
  fi
  file="$VOX_UPDATE_STATUS_FILE"
  dir="$(dirname "$file")"
  case "$pct" in
    "" | *[!0-9]*) pct=0 ;;
  esac
  if [ "$pct" -gt 100 ]; then
    pct=100
  fi
  if [ "$phase" = "done" ]; then
    pct=100
  elif [ "$pct" -gt 99 ]; then
    pct=99
  fi
  if [ "$pct" -lt "$STATUS_LAST_PERCENT" ]; then
    pct="$STATUS_LAST_PERCENT"
  fi
  if [ "$phase" = "done" ] || [ "$phase" = "failed" ]; then
    STATUS_TERMINAL=1
  fi
  ts="$(date +%s)"
  esc="$(status_json_escape "$msg")"
  line="{\"v\":1,\"run_id\":\"${VOX_UPDATE_RUN_ID}\",\"phase\":\"${phase}\",\"percent\":${pct},\"target\":\"${STATUS_TARGET_SHORT}\",\"target_full\":\"${STATUS_TARGET_FULL}\",\"message\":\"${esc}\",\"method\":\"${method}\",\"ts\":${ts}}"
  tmp="$dir/.$(basename "$file").tmp.$$"
  if printf '%s\n' "$line" >"$tmp" && mv -f "$tmp" "$file"; then
    STATUS_LAST_PERCENT="$pct"
    if [ -n "${VOX_UPDATE_STATUS_HISTORY:-}" ]; then
      printf '%s\n' "$line" >>"$VOX_UPDATE_STATUS_HISTORY" 2>/dev/null || true
    fi
  else
    rm -f "$tmp" 2>/dev/null || true
    echo "Warning: could not write the update status file $file" >&2
  fi
  return 0
}

# status_step <phase> <percent> <message> <failure-reason>
# Writes a phase and records the reason used if the run later fails.
status_step() {
  STATUS_FAIL_REASON="$4"
  status_write "$1" "$2" "$3" exact
}

# status_fail_if_open <fallback-reason>: writes `failed` unless a terminal
# phase was already written. Used by the EXIT trap.
status_fail_if_open() {
  local reason="${STATUS_FAIL_REASON:-$1}"
  if status_enabled && [ "$STATUS_TERMINAL" = 0 ]; then
    status_write failed "$STATUS_LAST_PERCENT" "$reason" exact
  fi
  return 0
}

# status_reload_percent: after a pipeline ran a status-writing subshell, the
# parent re-reads the last written percent so monotonic clamping stays right.
status_reload_percent() {
  local value
  if ! status_enabled || [ ! -f "$VOX_UPDATE_STATUS_FILE" ]; then
    return 0
  fi
  value="$(sed -n 's/.*"percent":\([0-9][0-9]*\).*/\1/p' "$VOX_UPDATE_STATUS_FILE" | head -1)"
  case "$value" in
    "" | *[!0-9]*) ;;
    *) STATUS_LAST_PERCENT="$value" ;;
  esac
  return 0
}

# status_now_ms: milliseconds, for throttling and the estimate.
status_now_ms() {
  if [ -n "${EPOCHREALTIME:-}" ]; then
    local secs="${EPOCHREALTIME%.*}" frac="${EPOCHREALTIME#*.}"
    echo $((10#$secs * 1000 + 10#$frac / 1000))
  else
    perl -MTime::HiRes=time -e 'printf "%d\n", time() * 1000'
  fi
}

# status_expected_seconds: the previous successful build duration, or 420.
status_expected_seconds() {
  local dir value
  dir="${VOX_UPDATE_STATE_DIR:-$(dirname "$VOX_UPDATE_STATUS_FILE")}"
  value="$(cat "$dir/last_build_seconds" 2>/dev/null || true)"
  case "$value" in
    "" | *[!0-9]*) echo 420 ;;
    *)
      if [ "$value" -le 0 ]; then
        echo 420
      else
        echo "$value"
      fi
      ;;
  esac
}

# status_save_build_seconds <seconds>: records a successful build duration for
# the next estimate. Atomic; failures are ignored.
status_save_build_seconds() {
  local dir tmp
  if ! status_enabled; then
    return 0
  fi
  dir="${VOX_UPDATE_STATE_DIR:-$(dirname "$VOX_UPDATE_STATUS_FILE")}"
  tmp="$dir/.last_build_seconds.tmp.$$"
  if printf '%s\n' "$1" >"$tmp" && mv -f "$tmp" "$dir/last_build_seconds"; then
    return 0
  fi
  rm -f "$tmp" 2>/dev/null || true
  return 0
}

# status_parse_progress <line>: if the line is a cargo progress line
# `Building [=>   ]  N/M: crate`, sets PROG_N and PROG_M and returns 0.
status_parse_progress() {
  local re='Building \[[^]]*\][[:space:]]+([0-9]+)/([0-9]+)'
  PROG_N=0
  PROG_M=0
  if [[ "$1" =~ $re ]]; then
    PROG_N="${BASH_REMATCH[1]}"
    PROG_M="${BASH_REMATCH[2]}"
    if [ "$PROG_M" -gt 0 ]; then
      if [ "$PROG_N" -gt "$PROG_M" ]; then
        PROG_N="$PROG_M"
      fi
      return 0
    fi
  fi
  return 1
}

# status_progress_maybe_write <pct> <message> <method>: writes the compile
# phase only when the percent moved by >= 1 point and >= 300 ms passed.
status_progress_maybe_write() {
  local pct="$1" msg="$2" method="$3" now
  if [ "$pct" -lt $((STATUS_LAST_PERCENT + 1)) ]; then
    return 0
  fi
  now="$(status_now_ms)"
  if [ $((now - STATUS_LAST_WRITE_MS)) -lt 300 ]; then
    return 0
  fi
  STATUS_LAST_WRITE_MS="$now"
  status_write compiling "$pct" "$msg" "$method"
}

# status_progress_consume: reads build output on stdin and writes compile-band
# progress. Runs until EOF. Never fails: the build's own exit status decides.
#
# Input is split on CR (cargo ends every progress update with CR; reading in
# CR-terminated chunks avoids a `tr` stage, which block-buffers when its output
# is a pipe and would hide the progress). Each chunk is matched as a whole, so
# LF-separated lines inside a chunk are handled too.
#
# Timeouts: bash 3.2 returns 1 from `read -t` both at EOF and on timeout, so a
# timeout is recognised by a read that took at least one whole second on the
# SECONDS clock (no subprocess per read). An immediate failing read is EOF.
status_progress_consume() {
  local chunk="" rc before_s start_s last_nm_s since elapsed expected est_after frac pct
  start_s=$SECONDS
  last_nm_s=""
  STATUS_LAST_WRITE_MS=0
  expected="$(status_expected_seconds)"
  est_after="${VOX_UPDATE_ESTIMATE_AFTER:-20}"
  while :; do
    rc=0
    chunk=""
    before_s=$SECONDS
    IFS= read -r -t 1 -d $'\r' chunk || rc=$?
    if [ -n "$chunk" ] && status_parse_progress "$chunk"; then
      pct=$((10 + 78 * PROG_N / PROG_M))
      if [ "$pct" -gt 87 ]; then
        pct=87
      fi
      last_nm_s=$SECONDS
      status_progress_maybe_write "$pct" "Compiling ${PROG_N}/${PROG_M} crates" exact
    fi
    if [ "$rc" -eq 0 ]; then
      continue
    fi
    if [ $((SECONDS - before_s)) -lt 1 ]; then
      break
    fi
    # Timeout: nothing arrived for a second. Estimate once the output has been
    # silent for long enough.
    if [ -n "$last_nm_s" ]; then
      since=$((SECONDS - last_nm_s))
    else
      since=$((SECONDS - start_s))
    fi
    if [ "$since" -ge "$est_after" ]; then
      elapsed=$((SECONDS - start_s))
      frac=$((elapsed * 1000 / expected))
      if [ "$frac" -gt 970 ]; then
        frac=970
      fi
      pct=$((10 + 77 * frac / 1000))
      if [ "$pct" -gt 87 ]; then
        pct=87
      fi
      status_progress_maybe_write "$pct" "Compiling (estimated)" estimated
    fi
  done
  return 0
}
