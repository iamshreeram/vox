#!/bin/bash
# Tests for the update status file, the build progress parser, the target pin,
# the lock override and the status environment checks of scripts/update-macos.sh
# (with scripts/lib/update-status.sh). Bash 3.2 compatible; python3 is used to
# read the status file (json.loads) while a run is in progress and to check the
# history of written status lines.
#
# Each scenario gets its own sandbox (mktemp -d) with an origin, a checkout that
# receives the REAL update script and libraries, a STUB install-macos.sh and a
# STUB bun on PATH that records every call. Nothing real is built or installed.
#
# Captured real build output used for replays lives in
# scripts/testdata/update-status/ (clean-build.capture.txt, incremental-build.capture.txt).
#
#   bash scripts/test-update-status.sh

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CAPTURE_DIR="$ROOT/scripts/testdata/update-status"

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
unset VOX_UPDATE_ALLOW_ORIGIN VOX_SOURCE_UPDATE VOX_SOURCE_UPDATES
unset VOX_BUILD_COMMIT VOX_BUILD_BRANCH FAKE_INSTALL_FAIL FAKE_BUILD_RC FAKE_PROGRESS FAKE_INSTALLER_RC
unset VOX_UPDATE_STATUS_FILE VOX_UPDATE_RUN_ID VOX_UPDATE_STATE_DIR VOX_UPDATE_STATUS_HISTORY
unset VOX_UPDATE_TARGET_SHA VOX_UPDATE_LOCK_DIR VOX_UPDATE_NONINTERACTIVE VOX_UPDATE_ESTIMATE_AFTER

BUNDLE_REL="src-tauri/target/release/bundle/macos/Vox.app"
CONFIG_REL="src-tauri/tauri.ci-unsigned.conf.json"

SANDBOX_BASE="$(mktemp -d)" || { echo "cannot create sandbox base" >&2; exit 1; }
trap 'rm -rf "$SANDBOX_BASE"' EXIT

PASSED=0
FAILED=0
REASON_FILE=""
READER=""

# Per-scenario globals.
SB=""
ORIGIN=""
CHECKOUT=""
PUSHER=""
STUBDIR=""
STUB_LOG=""
RUN_OUT=""
RUN_RC=""
STATUS=""
HIST=""
HELPERS=""
ORIGIN_NEW_SHA=""

# ---- helpers -----------------------------------------------------------------

fail() {
  echo "$*" >>"$REASON_FILE"
  exit 1
}

check() {
  local desc="$1"
  shift
  if "$@"; then
    return 0
  fi
  fail "assertion failed: $desc"
}

gitc() {
  git -c user.name=t -c user.email=t@example.com -c commit.gpgsign=false "$@"
}

log_has() {
  [ -f "$STUB_LOG" ] && grep -F -q -- "$1" "$STUB_LOG"
}

no_log() {
  ! { [ -f "$STUB_LOG" ] && grep -F -q -- "$1" "$STUB_LOG"; }
}

# status_phase / status_percent: values of the final status file.
status_phase() {
  sed -n 's/.*"phase":"\([a-z]*\)".*/\1/p' "$STATUS" 2>/dev/null | head -1
}

status_percent() {
  sed -n 's/.*"percent":\([0-9]*\).*/\1/p' "$STATUS" 2>/dev/null | head -1
}

# history_ok <history> [--partial]: the python checker finds no problems.
# --partial skips the "ends with a terminal phase" rule (replays of build output
# are not whole runs).
history_ok() {
  local out
  out="$(python3 "$HELPERS/check_history.py" "$@" 2>&1)"
  case "$out" in
    *"problems=0"*) return 0 ;;
  esac
  echo "$out" >&2
  return 1
}

# write_helpers: python programs used by the scenarios, written into the sandbox.
write_helpers() {
  HELPERS="$SB/helpers"
  mkdir -p "$HELPERS"
  cat >"$HELPERS/reader.py" <<'PY_EOF'
import json, os, sys, time
path, stop, report = sys.argv[1], sys.argv[2], sys.argv[3]
samples = invalid = nonmono = premature = 0
last = -1
phases = []
def look():
    global samples, invalid, nonmono, premature, last
    try:
        raw = open(path).read()
    except (IOError, OSError):
        return
    samples += 1
    try:
        d = json.loads(raw)
    except ValueError:
        invalid += 1
        return
    pct = d["percent"]
    if pct < last:
        nonmono += 1
    if d["phase"] != "done" and pct >= 100:
        premature += 1
    if not phases or phases[-1] != d["phase"]:
        phases.append(d["phase"])
    last = max(last, pct)
while not os.path.exists(stop):
    look()
    time.sleep(0.01)
look()
out = open(report, "w")
out.write("samples=%d invalid=%d nonmono=%d premature=%d phases=%s\n" % (
    samples, invalid, nonmono, premature, ",".join(phases)))
out.close()
PY_EOF
  cat >"$HELPERS/check_history.py" <<'PY_EOF'
import json, sys
# Checks every status line written during a run (the history file).
bands = {"preparing": (0, 4), "dependencies": (4, 10), "compiling": (10, 87),
         "bundling": (88, 92), "installing": (92, 96), "restarting": (97, 99),
         "done": (100, 100)}
partial = "--partial" in sys.argv
lines = [l for l in open(sys.argv[1]).read().split("\n") if l.strip()]
problems = []
last = -1
terminal_at = None
for index, line in enumerate(lines):
    try:
        d = json.loads(line)
    except ValueError:
        problems.append("invalid json line %d" % index)
        continue
    pct, phase = d["percent"], d["phase"]
    if d["v"] != 1 or len(d["run_id"]) != 16 or len(d["target"]) != 7:
        problems.append("bad header fields line %d" % index)
    if not isinstance(pct, int) or pct < 0 or pct > 100:
        problems.append("percent out of range line %d" % index)
    if pct < last:
        problems.append("percent decreased %d->%d line %d" % (last, pct, index))
    last = max(last, pct)
    if phase != "done" and pct >= 100:
        problems.append("premature 100 line %d" % index)
    if phase in bands and phase != "failed":
        lo, hi = bands[phase]
        if not (lo <= pct <= hi):
            problems.append("band %s violated: %d line %d" % (phase, pct, index))
    if phase in ("done", "failed"):
        terminal_at = index
if not partial:
    if terminal_at is None:
        problems.append("no terminal phase")
    elif terminal_at != len(lines) - 1:
        problems.append("terminal phase is not the last write")
print("lines=%d problems=%d" % (len(lines), len(problems)))
for p in problems:
    print("PROBLEM " + p)
PY_EOF
  cat >"$HELPERS/check_json.py" <<'PY_EOF'
import json, sys
json.load(open(sys.argv[1]))
PY_EOF
}

# setup_env <sandbox>: origin with main, checkout, pusher and stubs.
setup_env() {
  SB="$1"
  ORIGIN="$SB/origin.git"
  local seed="$SB/seed"
  CHECKOUT="$SB/checkout"
  PUSHER="$SB/pusher"
  STUBDIR="$SB/stubs"
  STUB_LOG="$SB/stub.log"
  RUN_OUT="$SB/run.out"
  STATUS="$SB/status/update.json"
  HIST="$SB/status/history.txt"
  mkdir -p "$SB/tmp" "$STUBDIR" "$SB/status"

  git init -q --bare "$ORIGIN" || return 1
  git --git-dir="$ORIGIN" symbolic-ref HEAD refs/heads/main || return 1
  git init -q "$seed" || return 1
  git -C "$seed" symbolic-ref HEAD refs/heads/main || return 1
  mkdir -p "$seed/scripts"
  cp "$ROOT/scripts/update-macos.sh" "$seed/scripts/update-macos.sh" || return 1
  chmod +x "$seed/scripts/update-macos.sh"
  cp -R "$ROOT/scripts/lib" "$seed/scripts/lib" || return 1
  cat >"$seed/scripts/install-macos.sh" <<'EOF'
#!/bin/bash
# STUB install-macos.sh: records the status phase it sees and its own exit.
echo "install-ran $1" >>"$STUB_LOG"
phase="$(sed -n 's/.*"phase":"\([a-z]*\)".*/\1/p' "$VOX_UPDATE_STATUS_FILE" 2>/dev/null | head -1)"
echo "install-sees ${phase:-none}" >>"$STUB_LOG"
if [ "${FAKE_INSTALLER_RC:-0}" != 0 ]; then exit "$FAKE_INSTALLER_RC"; fi
exit 0
EOF
  chmod +x "$seed/scripts/install-macos.sh"
  echo "seed readme" >"$seed/README.txt"
  gitc -C "$seed" add -A || return 1
  gitc -C "$seed" commit -q -m "seed" || return 1
  git -C "$seed" remote add origin "$ORIGIN" || return 1
  git -C "$seed" push -q origin main || return 1
  git clone -q "$ORIGIN" "$CHECKOUT" || return 1

  cat >"$STUBDIR/bun" <<'EOF'
#!/bin/bash
# STUB bun: records calls; `tauri build` can emit cargo-style progress with CR.
echo "bun $*" >>"$STUB_LOG"
if [ "$1" = "install" ]; then
  if [ "${FAKE_INSTALL_FAIL:-0}" = "1" ]; then exit 1; fi
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "tauri" ] && [ "$3" = "build" ]; then
  shift 3
  echo "tauri-build $*" >>"$STUB_LOG"
  echo "env VOX_BUILD_COMMIT=$VOX_BUILD_COMMIT GIT_TERMINAL_PROMPT=${GIT_TERMINAL_PROMPT:-unset}" >>"$STUB_LOG"
  if [ "${FAKE_PROGRESS:-0}" = "1" ]; then
    for n in 1 2 3 5 8 10; do
      printf '   Compiling dep%s v1.0.0\n' "$n"
      printf '    Building [=====>      ]  %s/10: crate%s\r' "$n" "$n"
      sleep 0.4
    done
    printf '\n    Finished `release` profile\n'
  fi
  if [ "${FAKE_BUILD_RC:-0}" != "0" ]; then exit "$FAKE_BUILD_RC"; fi
  mkdir -p src-tauri/target/release/bundle/macos/Vox.app
  echo dummy >src-tauri/target/release/bundle/macos/Vox.app/dummy
  exit 0
fi
echo "bun: unexpected args: $*" >>"$STUB_LOG"
exit 2
EOF
  chmod +x "$STUBDIR/bun"
  write_helpers
  return 0
}

advance_origin() {
  rm -rf "$PUSHER"
  git clone -q "$ORIGIN" "$PUSHER" || fail "could not clone pusher"
  echo "upstream change" >"$PUSHER/upstream.txt"
  gitc -C "$PUSHER" add upstream.txt || fail "pusher add failed"
  gitc -C "$PUSHER" commit -q -m "upstream change" || fail "pusher commit failed"
  git -C "$PUSHER" push -q origin main || fail "pusher push failed"
  ORIGIN_NEW_SHA="$(git -C "$ORIGIN" rev-parse main)"
}

# run_update: runs the script under test with the current exported environment.
run_update() {
  (
    cd "$CHECKOUT" &&
      PATH="$STUBDIR:$PATH" TMPDIR="$SB/tmp" STUB_LOG="$STUB_LOG" \
        bash scripts/update-macos.sh
  ) >"$RUN_OUT" 2>&1 </dev/null
  RUN_RC=$?
}

# run_update_with_reader: run_update with the 10 ms status reader alongside.
run_update_with_reader() {
  local stop="$SB/reader.stop" report="$SB/reader.report" pid
  rm -f "$stop" "$report"
  python3 "$HELPERS/reader.py" "$STATUS" "$stop" "$report" &
  pid=$!
  run_update
  : >"$stop"
  wait "$pid" 2>/dev/null
  READER="$(cat "$report" 2>/dev/null)"
}

lock_path() {
  echo "$SB/tmp/vox-update.lock"
}

export_status() {
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_STATUS_FILE="$STATUS"
  export VOX_UPDATE_RUN_ID="0123456789abcdef"
  export VOX_UPDATE_STATUS_HISTORY="$HIST"
}

# ---- scenarios: progress and status file -------------------------------------

scenario_status_success_full_run() {
  setup_env "$1" || fail "sandbox setup failed"
  export_status
  export FAKE_PROGRESS=1
  advance_origin
  run_update_with_reader
  check "exit 0 (got $RUN_RC; $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "reader: no invalid JSON, no decrease, no early 100 ($READER)" \
    bash -c 'case "$1" in *invalid=0*nonmono=0*premature=0*) exit 0;; esac; exit 1' _ "$READER"
  check "reader saw done last ($READER)" bash -c 'case "$1" in *phases=*done) exit 0;; esac; exit 1' _ "$READER"
  check "history passes the band and order checks" history_ok "$HIST"
  check "final phase is done" [ "$(status_phase)" = "done" ]
  check "final percent 100" [ "$(status_percent)" = "100" ]
  check "installer saw the installing phase" log_has "install-sees installing"
  check "compiling lines were written from cargo N/M" grep -q '"message":"Compiling 10/10 crates"' "$HIST"
  check "compiling used exact method" grep -q '"method":"exact"' "$HIST"
  check "done is the last history line" bash -c 'tail -n 1 "$1" | grep -q "\"phase\":\"done\""' _ "$HIST"
  check "the status run_id is the one passed in" grep -q '"run_id":"0123456789abcdef"' "$STATUS"
  check "target_full is the new origin HEAD" grep -q "\"target_full\":\"$ORIGIN_NEW_SHA\"" "$STATUS"
  check "lock released" [ ! -e "$(lock_path)" ]
  check "status file is valid JSON" python3 "$HELPERS/check_json.py" "$STATUS"
}

scenario_status_build_failure() {
  setup_env "$1" || fail "sandbox setup failed"
  export_status
  export FAKE_PROGRESS=1 FAKE_BUILD_RC=3
  advance_origin
  run_update_with_reader
  check "exit non-zero on build failure (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "status phase is failed" [ "$(status_phase)" = "failed" ]
  check "failed keeps the last percent below 100" [ "$(status_percent)" -lt 100 ]
  check "failed reason names the build" grep -q "Build failed (exit status 3)" "$STATUS"
  check "installer NOT run after build failure" no_log "install-ran"
  check "failed is the last history line" bash -c 'tail -n 1 "$1" | grep -q "\"phase\":\"failed\""' _ "$HIST"
  check "history passes the checks" history_ok "$HIST"
  check "lock released" [ ! -e "$(lock_path)" ]
}

scenario_status_installer_failure() {
  setup_env "$1" || fail "sandbox setup failed"
  export_status
  export FAKE_INSTALLER_RC=5
  advance_origin
  run_update_with_reader
  check "exit non-zero when the installer fails (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "status phase is failed, never done" [ "$(status_phase)" = "failed" ]
  check "no done line was written" bash -c '! grep -q "\"phase\":\"done\"" "$1"' _ "$HIST"
  check "lock released" [ ! -e "$(lock_path)" ]
}

scenario_status_no_progress_output() {
  setup_env "$1" || fail "sandbox setup failed"
  export_status
  advance_origin
  run_update
  check "exit 0 without progress output (got $RUN_RC)" [ "$RUN_RC" -eq 0 ]
  check "history passes the checks" history_ok "$HIST"
  check "done written" [ "$(status_phase)" = "done" ]
}

# ---- scenarios: parser on real output and synthetic output -------------------

# replay_into_history <input> [paced]: feeds the parser and records the history.
# A paced replay sleeps between chunks so the 300 ms throttle lets updates through.
replay_into_history() {
  local input="$1" paced="${2:-}"
  rm -f "$HIST"
  (
    export VOX_UPDATE_STATUS_FILE="$STATUS" VOX_UPDATE_RUN_ID="0123456789abcdef"
    export VOX_UPDATE_STATUS_HISTORY="$HIST" VOX_UPDATE_ESTIMATE_AFTER="${EST_AFTER:-20}"
    export VOX_UPDATE_STATE_DIR="$SB/status"
    . "$ROOT/scripts/lib/update-status.sh"
    STATUS_LAST_PERCENT=10
    if [ -n "$paced" ]; then
      while IFS= read -r -d $'\r' chunk || [ -n "$chunk" ]; do
        printf '%s\r' "$chunk"
        sleep 0.35
      done <"$input" | status_progress_consume
    else
      status_progress_consume <"$input"
    fi
  )
}

scenario_parser_on_captured_clean_build() {
  setup_env "$1" || fail "sandbox setup failed"
  check "captured clean log exists" [ -f "$CAPTURE_DIR/clean-build.capture.txt" ]
  # Every progress line of the real capture parses; the denominator is 1050.
  local summary
  summary="$(
    . "$ROOT/scripts/lib/update-status.sh"
    tr '\r' '\n' <"$CAPTURE_DIR/clean-build.capture.txt" | while IFS= read -r line; do
      if status_parse_progress "$line"; then
        echo "$PROG_N $PROG_M"
      fi
    done | awk '{ if ($1 > max) max = $1; d[$2] = 1; n++ } END { c = 0; for (k in d) { c++; last = k }; print n, max, c, last }'
  )"
  check "all progress lines parse, one denominator (got: $summary)" \
    bash -c '[ "$1" = "1039 1049 1 1050" ]' _ "$summary"
  replay_into_history "$CAPTURE_DIR/clean-build.capture.txt"
  check "replay of the clean capture passes the checks" history_ok "$HIST" --partial
  check "replay wrote exact compiling updates" grep -q '"method":"exact"' "$HIST"
  check "replay never wrote 88 or more while compiling" \
    bash -c '! grep "\"phase\":\"compiling\"" "$1" | grep -q "\"percent\":\(8[89]\|9[0-9]\|100\)"' _ "$HIST"
}

scenario_parser_on_captured_incremental_build() {
  setup_env "$1" || fail "sandbox setup failed"
  check "captured incremental log exists" [ -f "$CAPTURE_DIR/incremental-build.capture.txt" ]
  replay_into_history "$CAPTURE_DIR/incremental-build.capture.txt"
  check "replay of the incremental capture passes the checks" history_ok "$HIST" --partial
  check "replay wrote exact compiling updates" grep -q '"method":"exact"' "$HIST"
}

scenario_parser_changing_denominator() {
  setup_env "$1" || fail "sandbox setup failed"
  printf 'Building [=>  ] 1/10: a\r' >"$SB/denom.txt"
  printf 'Building [==>  ] 5/10: b\r' >>"$SB/denom.txt"
  printf 'Building [==>  ] 2/40: c\r' >>"$SB/denom.txt"
  printf 'Building [===> ] 30/40: d\r' >>"$SB/denom.txt"
  printf 'Building [===> ] 70/200: e\r' >>"$SB/denom.txt"
  printf 'Building [===> ] 199/200: f\r' >>"$SB/denom.txt"
  printf 'Building [===> ] 200/200: g\n' >>"$SB/denom.txt"
  printf 'Building [===> ] 900/10: bogus-over-denominator\n' >>"$SB/denom.txt"
  replay_into_history "$SB/denom.txt" paced
  check "several updates were written" [ "$(wc -l <"$HIST" | tr -d ' ')" -ge 3 ]
  check "changing denominators pass the checks" history_ok "$HIST" --partial
  check "the percent never exceeds 87 while compiling" \
    bash -c '! grep -q "\"percent\":\(8[89]\|9[0-9]\|100\)" "$1"' _ "$HIST"
}

scenario_parser_long_silence_estimates() {
  setup_env "$1" || fail "sandbox setup failed"
  # Expected duration 2 s; estimate after 1 s of silence; 4 s of silence in all.
  echo 2 >"$SB/status/last_build_seconds"
  (
    export VOX_UPDATE_STATUS_FILE="$STATUS" VOX_UPDATE_RUN_ID="0123456789abcdef"
    export VOX_UPDATE_STATUS_HISTORY="$HIST" VOX_UPDATE_ESTIMATE_AFTER=1
    export VOX_UPDATE_STATE_DIR="$SB/status"
    . "$ROOT/scripts/lib/update-status.sh"
    STATUS_LAST_PERCENT=10
    (printf 'Compiling only, no counter\n'; sleep 4; printf 'done-silence\n') | status_progress_consume
  )
  check "long silence produced estimated updates" grep -q '"method":"estimated"' "$HIST"
  check "estimated updates pass the checks" history_ok "$HIST" --partial
  check "estimate stays at or below 87" \
    bash -c '! grep -q "\"percent\":\(8[89]\|9[0-9]\|100\)" "$1"' _ "$HIST"
}

# ---- scenarios: status environment -------------------------------------------

scenario_status_bad_setup_writes_nothing() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_RUN_ID="0123456789abcdef"
  export VOX_UPDATE_STATUS_FILE="relative/update.json"
  run_update
  check "relative status path refused (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
  export VOX_UPDATE_STATUS_FILE="$SB/missing-dir/update.json"
  run_update
  check "missing parent dir refused (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "no bun call for missing dir" no_log "bun "
  export VOX_UPDATE_STATUS_FILE="$STATUS"
  export VOX_UPDATE_RUN_ID="ABCDEF0123456789"
  run_update
  check "uppercase run id refused (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "no bun call for bad run id" no_log "bun "
  check "no status file written for a bad run id" [ ! -e "$STATUS" ]
}

# ---- scenarios: target pin ---------------------------------------------------

scenario_pin_valid_newer_origin() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  advance_origin
  export VOX_UPDATE_TARGET_SHA="$ORIGIN_NEW_SHA"
  run_update
  check "exit 0 for a valid pin (got $RUN_RC; $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "built exactly the target" log_has "VOX_BUILD_COMMIT=$ORIGIN_NEW_SHA"
  check "install ran" log_has "install-ran $BUNDLE_REL"
}

scenario_pin_origin_moved_past_target() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  local old
  old="$(git -C "$CHECKOUT" rev-parse HEAD)"
  advance_origin
  export VOX_UPDATE_TARGET_SHA="$old"
  run_update
  check "exit 0 when origin is ahead of the target (got $RUN_RC)" [ "$RUN_RC" -eq 0 ]
  check "built the older target, not origin" log_has "VOX_BUILD_COMMIT=$old"
  check "local HEAD equals the target" [ "$(git -C "$CHECKOUT" rev-parse HEAD)" = "$old" ]
}

# pin_refused <label>: the run must refuse before any bun, cargo or install call.
pin_refused() {
  check "$1: exit non-zero (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "$1: no bun call" no_log "bun "
  check "$1: no install call" no_log "install-ran"
  check "$1: lock released" [ ! -e "$(lock_path)" ]
}

scenario_pin_malformed() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_STATUS_FILE="$STATUS" VOX_UPDATE_RUN_ID="0123456789abcdef"
  local bad
  for bad in "ABCDEF0123456789ABCDEF0123456789ABCDEF01" "abc123" "" \
    "g234567890123456789012345678901234567890" "a234567890123456789012345678901234567890a"; do
    export VOX_UPDATE_TARGET_SHA="$bad"
    run_update
    pin_refused "malformed pin '${bad:0:12}'"
  done
  check "status failed for a malformed pin" [ "$(status_phase)" = "failed" ]
}

scenario_pin_unreachable() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_TARGET_SHA="1234567890abcdef1234567890abcdef12345678"
  run_update
  pin_refused "unreachable target"
}

scenario_pin_head_ahead() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  local old
  old="$(git -C "$CHECKOUT" rev-parse HEAD)"
  echo "local" >"$CHECKOUT/local.txt"
  gitc -C "$CHECKOUT" add local.txt || fail "add failed"
  gitc -C "$CHECKOUT" commit -q -m "local ahead" || fail "commit failed"
  export VOX_UPDATE_TARGET_SHA="$old"
  run_update
  pin_refused "HEAD ahead of target"
  check "local HEAD kept (not reset to the target)" [ "$(git -C "$CHECKOUT" rev-parse HEAD)" != "$old" ]
}

scenario_pin_diverged() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  echo "local" >"$CHECKOUT/local.txt"
  gitc -C "$CHECKOUT" add local.txt || fail "add failed"
  gitc -C "$CHECKOUT" commit -q -m "local diverged" || fail "commit failed"
  advance_origin
  export VOX_UPDATE_TARGET_SHA="$ORIGIN_NEW_SHA"
  run_update
  pin_refused "diverged from target"
}

# ---- scenarios: lock override and non-interactive mode -----------------------

scenario_lock_override_used() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_LOCK_DIR="$SB/custom.lock"
  advance_origin
  run_update
  check "exit 0 with the lock override (got $RUN_RC)" [ "$RUN_RC" -eq 0 ]
  check "default lock dir never created" [ ! -e "$(lock_path)" ]
  check "override lock dir released" [ ! -e "$SB/custom.lock" ]
}

scenario_lock_override_stale_untouched() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_LOCK_DIR="$SB/custom.lock"
  mkdir "$SB/custom.lock" || fail "could not pre-create the stale lock"
  echo "99999-stale" >"$SB/custom.lock/owner"
  local ino
  ino="$(ls -di "$SB/custom.lock" | awk '{print $1}')"
  run_update
  check "exit non-zero with a stale lock (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "stale lock same inode" [ "$(ls -di "$SB/custom.lock" | awk '{print $1}')" = "$ino" ]
  check "stale owner file untouched" [ "$(cat "$SB/custom.lock/owner")" = "99999-stale" ]
  check "no bun call" no_log "bun "
}

scenario_lock_override_relative_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_LOCK_DIR="relative.lock"
  run_update
  check "relative lock dir refused (got $RUN_RC)" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
}

scenario_noninteractive_full_run() {
  setup_env "$1" || fail "sandbox setup failed"
  export_status
  export VOX_UPDATE_NONINTERACTIVE=1
  advance_origin
  run_update
  check "exit 0 with stdin closed in non-interactive mode (got $RUN_RC; $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "GIT_TERMINAL_PROMPT=0 set for the build" log_has "GIT_TERMINAL_PROMPT=0"
  check "done written" [ "$(status_phase)" = "done" ]
}

scenario_unset_variables_no_status_file() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  advance_origin
  run_update
  check "exit 0 with nothing set (got $RUN_RC)" [ "$RUN_RC" -eq 0 ]
  check "no status file or state file in the sandbox" \
    bash -c '! find "$1" -name "*.json" -o -name "last_build_seconds" | grep -q .' _ "$SB/status"
  check "no progress lines in the output" bash -c '! grep -q "Compiling" "$1"' _ "$RUN_OUT"
  check "build run with the plain command" log_has "tauri-build --bundles app --config $CONFIG_REL"
  check "GIT_TERMINAL_PROMPT not forced" log_has "GIT_TERMINAL_PROMPT=unset"
}

# ---- runner ------------------------------------------------------------------

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

run_scenario "(S1) full run with cargo progress: valid JSON, monotonic, bands, done last" scenario_status_success_full_run
run_scenario "(S2) build failure: failed, non-zero, installer not called" scenario_status_build_failure
run_scenario "(S3) installer failure: failed, never done" scenario_status_installer_failure
run_scenario "(S4) no progress output: still valid, done written" scenario_status_no_progress_output
run_scenario "(S5) bad status setup (relative, missing dir, bad run id): nothing runs" scenario_status_bad_setup_writes_nothing
run_scenario "(P1) parser: every progress line of the captured clean build parses" scenario_parser_on_captured_clean_build
run_scenario "(P2) parser: captured incremental build replays monotonic and bounded" scenario_parser_on_captured_incremental_build
run_scenario "(P3) parser: changing denominators stay monotonic and bounded" scenario_parser_changing_denominator
run_scenario "(P4) parser: long silence, estimate bounded and monotonic" scenario_parser_long_silence_estimates
run_scenario "(T1) target pin valid, origin moved ahead: builds the target" scenario_pin_valid_newer_origin
run_scenario "(T2) target pin older than origin: builds the target exactly" scenario_pin_origin_moved_past_target
run_scenario "(T3) target pin malformed: refused before any bun call" scenario_pin_malformed
run_scenario "(T4) target pin unreachable: refused before any bun call" scenario_pin_unreachable
run_scenario "(T5) target pin with local HEAD ahead: refused" scenario_pin_head_ahead
run_scenario "(T6) target pin with diverged history: refused" scenario_pin_diverged
run_scenario "(L1) lock override used, default lock never created" scenario_lock_override_used
run_scenario "(L2) lock override with a stale lock: untouched and refused" scenario_lock_override_stale_untouched
run_scenario "(L3) lock override must be absolute" scenario_lock_override_relative_refused
run_scenario "(N1) non-interactive full run, GIT_TERMINAL_PROMPT=0, done" scenario_noninteractive_full_run
run_scenario "(U1) variables unset: no status file, plain build command" scenario_unset_variables_no_status_file

echo "$PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
