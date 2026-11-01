#!/bin/bash
# Tests for the build and install phases of scripts/update-macos.sh
# (VOX_UPDATE_PHASE=build|install, scripts/lib/update-staging.sh). Bash 3.2
# compatible. python3 reads the JSON files and computes the expected digests.
#
# Each scenario gets its own sandbox (mktemp -d) with an origin, a checkout that
# receives the REAL update script and libraries, a STUB install-macos.sh and a
# STUB bun on PATH. The stub `bun run tauri build` writes a fake fresh bundle
# (Vox.app/Contents/Info.plist with CFBundleExecutable, Contents/MacOS/<name>).
# Nothing real is built or installed. plutil/shasum shims are used only when the
# real tools are missing.
#
#   bash scripts/test-update-staging.sh

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
unset VOX_UPDATE_ALLOW_ORIGIN VOX_SOURCE_UPDATE VOX_SOURCE_UPDATES
unset VOX_BUILD_COMMIT VOX_BUILD_BRANCH FAKE_INSTALLER_RC FAKE_BUILD_RC FAKE_EXE_SYMLINK FAKE_EXE_NAME
unset VOX_UPDATE_STATUS_FILE VOX_UPDATE_RUN_ID VOX_UPDATE_STATE_DIR VOX_UPDATE_STATUS_HISTORY
unset VOX_UPDATE_TARGET_SHA VOX_UPDATE_LOCK_DIR VOX_UPDATE_NONINTERACTIVE
unset VOX_UPDATE_PHASE VOX_UPDATE_PENDING_FILE VOX_UPDATE_PERCENT_FLOOR VOX_UPDATE_ORIGIN
unset VOX_PLUTIL VOX_SHASUM CARGO_HOME RUSTUP_HOME BUN_INSTALL_CACHE_DIR

SANDBOX_BASE="$(mktemp -d)" || { echo "cannot create sandbox base" >&2; exit 1; }
trap 'rm -rf "$SANDBOX_BASE"' EXIT

PASSED=0
FAILED=0
REASON_FILE=""
RUN_RC=""
RUN_OUT=""

BUILD_RUN_ID="0123456789abcdef"
INSTALL_RUN_ID="fedcba9876543210"

# Per-scenario globals.
SB=""
ORIGIN=""
CHECKOUT=""
STUBDIR=""
STUB_LOG=""
STATE=""
STATUS=""
HIST=""
PENDING=""
STAGED=""

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
  ! log_has "$1"
}

# file_mode <path>: octal permission bits (BSD and GNU stat).
file_mode() {
  stat -f %Lp "$1" 2>/dev/null || stat -c %a "$1"
}

# status_field <status-file> <key>: one value of the status JSON.
status_field() {
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2" 2>/dev/null
}

# sha_of <file>: sha256 computed by python (independent of the script's tools).
sha_of() {
  python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$1"
}

# json_set <file> <key> <value>: edits one key of a JSON file (v and ts are numbers).
json_set() {
  python3 - "$1" "$2" "$3" <<'PY_EOF'
import json, sys
path, key, value = sys.argv[1:4]
d = json.load(open(path))
d[key] = int(value) if key in ("v", "ts") else value
json.dump(d, open(path, "w"))
PY_EOF
}

# json_has_exact_keys <file>: the pending file has exactly the schema fields.
json_has_exact_keys() {
  python3 - "$1" <<'PY_EOF'
import json, sys
want = {"v", "run_id", "target_full", "staged_app", "exe_name", "exe_sha256",
        "info_plist_sha256", "state", "origin", "ts"}
d = json.load(open(sys.argv[1]))
sys.exit(0 if set(d) == want else 1)
PY_EOF
}

# hist_ok <history> <floor> <build|install>: monotonic, never below the floor,
# no `failed`, and the last line is the expected resting or terminal state.
hist_ok() {
  python3 - "$1" "$2" "$3" <<'PY_EOF'
import json, sys
path, floor, mode = sys.argv[1], int(sys.argv[2]), sys.argv[3]
rows = [json.loads(line) for line in open(path) if line.strip()]
bad = []
last = -1
for row in rows:
    if row["percent"] < last:
        bad.append("decrease")
    if row["percent"] < floor:
        bad.append("below floor %d" % row["percent"])
    if row["phase"] == "failed":
        bad.append("failed written")
    last = row["percent"]
if not rows:
    bad.append("no rows")
elif mode == "build":
    if rows[-1]["phase"] != "waiting" or rows[-1]["percent"] != 91:
        bad.append("build does not end at waiting 91")
else:
    if rows[-1]["phase"] != "done" or rows[-1]["percent"] != 100:
        bad.append("install does not end at done 100")
print("problems=%d %s" % (len(bad), bad))
sys.exit(1 if bad else 0)
PY_EOF
}

# write_stubs: the stub bun, install-macos.sh, and shims for missing tools.
write_stubs() {
  mkdir -p "$STUBDIR"
  cat >"$STUBDIR/bun" <<'EOF'
#!/bin/bash
# STUB bun: records calls; `run tauri build` writes a fake fresh bundle.
echo "bun $*" >>"$STUB_LOG"
if [ "$1" = "install" ]; then exit 0; fi
if [ "$1" = "run" ] && [ "$2" = "tauri" ] && [ "$3" = "build" ]; then
  echo "tauri-build" >>"$STUB_LOG"
  if [ "${FAKE_BUILD_RC:-0}" != "0" ]; then exit "$FAKE_BUILD_RC"; fi
  app=src-tauri/target/release/bundle/macos/Vox.app
  mkdir -p "$app/Contents/MacOS"
  printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' \
    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
    '<plist version="1.0"><dict>' \
    '<key>CFBundleExecutable</key><string>'"${FAKE_EXE_NAME:-vox}"'</string>' \
    '</dict></plist>' >"$app/Contents/Info.plist"
  if [ "${FAKE_EXE_SYMLINK:-0}" = "1" ]; then
    ln -s /bin/sh "$app/Contents/MacOS/${FAKE_EXE_NAME:-vox}"
  else
    printf 'binary\n' >"$app/Contents/MacOS/${FAKE_EXE_NAME:-vox}"
  fi
  exit 0
fi
echo "bun: unexpected args: $*" >>"$STUB_LOG"
exit 2
EOF
  chmod +x "$STUBDIR/bun"
  if ! command -v plutil >/dev/null 2>&1; then
    cat >"$STUBDIR/plutil" <<'EOF'
#!/bin/bash
# STUB plutil (only when the real tool is missing): -extract KEY raw -o - FILE
[ "$1" = "-extract" ] || exit 2
python3 - "$2" "$6" <<'PY_EOF'
import json, plistlib, sys
key, path = sys.argv[1], sys.argv[2]
raw = open(path, "rb").read()
try:
    data = json.loads(raw.decode())
except ValueError:
    data = plistlib.loads(raw)
if key not in data:
    sys.exit(1)
print(data[key])
PY_EOF
EOF
    chmod +x "$STUBDIR/plutil"
  fi
  if ! command -v shasum >/dev/null 2>&1; then
    cat >"$STUBDIR/shasum" <<'EOF'
#!/bin/bash
# STUB shasum (only when the real tool is missing): shasum -a 256 FILE
shift 2
exec sha256sum "$1"
EOF
    chmod +x "$STUBDIR/shasum"
  fi
}

# setup_env <sandbox>: origin with main, checkout with the real scripts, stubs.
setup_env() {
  SB="$1"
  ORIGIN="$SB/origin.git"
  local seed="$SB/seed"
  CHECKOUT="$SB/checkout"
  STUBDIR="$SB/stubs"
  STUB_LOG="$SB/stub.log"
  STATE="$SB/state"
  STATUS="$SB/status/build.json"
  HIST="$SB/status/build-history.txt"
  PENDING="$STATE/pending.json"
  STAGED="$STATE/staged/$BUILD_RUN_ID/Vox.app"
  mkdir -p "$SB/tmp" "$STATE" "$SB/status" "$STUBDIR"

  git init -q --bare "$ORIGIN" || return 1
  git --git-dir="$ORIGIN" symbolic-ref HEAD refs/heads/main || return 1
  git init -q "$seed" || return 1
  git -C "$seed" symbolic-ref HEAD refs/heads/main || return 1
  mkdir -p "$seed/scripts"
  cp "$ROOT/scripts/update-macos.sh" "$seed/scripts/update-macos.sh" || return 1
  cp -R "$ROOT/scripts/lib" "$seed/scripts/lib" || return 1
  cat >"$seed/scripts/install-macos.sh" <<'EOF'
#!/bin/bash
# STUB install-macos.sh: records the staged path it was given and its commit.
echo "install-ran $1 commit=${VOX_BUILD_COMMIT:-unset}" >>"$STUB_LOG"
if [ "${FAKE_INSTALLER_RC:-0}" != 0 ]; then exit "$FAKE_INSTALLER_RC"; fi
exit 0
EOF
  chmod +x "$seed/scripts/install-macos.sh" "$seed/scripts/update-macos.sh"
  echo "seed readme" >"$seed/README.txt"
  gitc -C "$seed" add -A || return 1
  gitc -C "$seed" commit -q -m "seed" || return 1
  git -C "$seed" remote add origin "$ORIGIN" || return 1
  git -C "$seed" push -q origin main || return 1
  git clone -q "$ORIGIN" "$CHECKOUT" || return 1
  write_stubs
  return 0
}

# export_build: environment of a build-phase run.
export_build() {
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_PHASE=build
  export VOX_UPDATE_STATE_DIR="$STATE"
  export VOX_UPDATE_STATUS_FILE="$STATUS"
  export VOX_UPDATE_RUN_ID="$BUILD_RUN_ID"
  export VOX_UPDATE_STATUS_HISTORY="$HIST"
  export VOX_UPDATE_LOCK_DIR="$SB/tmp/vox-update.lock"
}

# run_script: runs scripts/update-macos.sh in the checkout with the current env.
run_script() {
  (
    cd "$CHECKOUT" &&
      PATH="$STUBDIR:$PATH" TMPDIR="$SB/tmp" STUB_LOG="$STUB_LOG" \
        bash scripts/update-macos.sh
  ) >"$SB/run.out" 2>&1 </dev/null
  RUN_RC=$?
  RUN_OUT="$SB/run.out"
}

# prepare_built: a successful build phase, so pending.json and the staged copy exist.
prepare_built() {
  export_build
  run_script
  if [ "$RUN_RC" -ne 0 ]; then
    fail "prepare: build phase exited $RUN_RC: $(tr '\n' ' ' <"$RUN_OUT")"
  fi
  [ -f "$PENDING" ] || fail "prepare: no pending.json after the build"
}

# commit_pending: what the committing side does before the install run.
commit_pending() {
  json_set "$PENDING" state committing || fail "could not set committing"
  json_set "$PENDING" install_run_id "$INSTALL_RUN_ID" || fail "could not set install_run_id"
}

# export_install: environment of an install-phase run (a fresh status file).
export_install() {
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_PHASE=install
  export VOX_UPDATE_STATE_DIR="$STATE"
  export VOX_UPDATE_PENDING_FILE="$PENDING"
  export VOX_UPDATE_PERCENT_FLOOR=91
  export VOX_UPDATE_STATUS_FILE="$SB/status/install.json"
  export VOX_UPDATE_RUN_ID="$INSTALL_RUN_ID"
  export VOX_UPDATE_STATUS_HISTORY="$SB/status/install-history.txt"
  export VOX_UPDATE_LOCK_DIR="$SB/tmp/vox-update.lock"
  STATUS="$SB/status/install.json"
  HIST="$SB/status/install-history.txt"
  : >"$STUB_LOG"
}

# run_install: install phase with a clean stub log.
run_install() {
  export_install
  run_script
}

# ---- scenarios: build phase --------------------------------------------------

scenario_build_happy_path() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  run_script
  check "build exit 0 (got $RUN_RC: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "pending.json written" [ -f "$PENDING" ]
  check "pending.json mode 600" [ "$(file_mode "$PENDING")" = "600" ]
  check "pending.json has exactly the schema fields" json_has_exact_keys "$PENDING"
  local target
  target="$(git -C "$CHECKOUT" rev-parse HEAD)"
  check "v is 1" [ "$(status_field "$PENDING" v)" = "1" ]
  check "run_id is the build run id" [ "$(status_field "$PENDING" run_id)" = "$BUILD_RUN_ID" ]
  check "target_full is HEAD" [ "$(status_field "$PENDING" target_full)" = "$target" ]
  check "staged_app is the canonical staged path" \
    [ "$(status_field "$PENDING" staged_app)" = "$STAGED" ]
  check "exe_name is vox" [ "$(status_field "$PENDING" exe_name)" = "vox" ]
  check "state is built" [ "$(status_field "$PENDING" state)" = "built" ]
  check "origin defaults to manual" [ "$(status_field "$PENDING" origin)" = "manual" ]
  check "ts is a number" python3 -c 'import json,sys; sys.exit(0 if isinstance(json.load(open(sys.argv[1]))["ts"], int) else 1)' "$PENDING"
  check "exe digest equals the staged executable" \
    [ "$(status_field "$PENDING" exe_sha256)" = "$(sha_of "$STAGED/Contents/MacOS/vox")" ]
  check "plist digest equals the staged Info.plist" \
    [ "$(status_field "$PENDING" info_plist_sha256)" = "$(sha_of "$STAGED/Contents/Info.plist")" ]
  check "staged run dir mode 700" [ "$(file_mode "$STATE/staged/$BUILD_RUN_ID")" = "700" ]
  check "staged app exists" [ -d "$STAGED" ]
  check "status is waiting" [ "$(status_field "$STATUS" phase)" = "waiting" ]
  check "status percent 91" [ "$(status_field "$STATUS" percent)" = "91" ]
  check "history ends at waiting 91, monotonic, no failed" hist_ok "$HIST" 0 build
  check "install was not called" no_log "install-ran"
  check "lock released" [ ! -e "$SB/tmp/vox-update.lock" ]
}

scenario_build_failure_cleans_up() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  export FAKE_BUILD_RC=3
  run_script
  check "build failure exits non-zero" [ "$RUN_RC" -ne 0 ]
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "no pending.json" [ ! -e "$PENDING" ]
  check "no staged run dir" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
  check "install was not called" no_log "install-ran"
  check "lock released" [ ! -e "$SB/tmp/vox-update.lock" ]
}

scenario_build_stale_pending_removed() {
  setup_env "$1" || fail "sandbox setup failed"
  printf '%s\n' '{"v":1,"run_id":"aaaaaaaaaaaaaaaa","state":"built"}' >"$PENDING"
  export_build
  export FAKE_BUILD_RC=3
  run_script
  check "a stale pending.json does not survive a failed build" [ ! -e "$PENDING" ]
  check "the build did run" log_has "tauri-build"
}

scenario_build_stale_pending_replaced() {
  setup_env "$1" || fail "sandbox setup failed"
  printf '%s\n' '{"v":1,"run_id":"aaaaaaaaaaaaaaaa","state":"built"}' >"$PENDING"
  export_build
  run_script
  check "build exit 0 (got $RUN_RC: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "pending.json is the new one" [ "$(status_field "$PENDING" run_id)" = "$BUILD_RUN_ID" ]
}

scenario_build_symlinked_staged_parent_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  mkdir -p "$SB/elsewhere"
  ln -s "$SB/elsewhere" "$STATE/staged"
  export_build
  run_script
  check "symlinked staged parent: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "nothing staged through the link" [ ! -e "$SB/elsewhere/$BUILD_RUN_ID" ]
  check "no pending.json" [ ! -e "$PENDING" ]
}

scenario_build_bad_origin_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  export VOX_UPDATE_ORIGIN=bogus
  run_script
  check "bad VOX_UPDATE_ORIGIN: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
  check "no pending.json" [ ! -e "$PENDING" ]
}

scenario_build_bad_phase_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  export VOX_UPDATE_PHASE=bogus
  run_script
  check "bad VOX_UPDATE_PHASE: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
}

scenario_build_needs_state_dir() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  unset VOX_UPDATE_STATE_DIR
  run_script
  check "build without VOX_UPDATE_STATE_DIR: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
}

scenario_build_symlinked_exe_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  export FAKE_EXE_SYMLINK=1
  run_script
  check "symlinked executable: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "no pending.json" [ ! -e "$PENDING" ]
  check "no staged run dir" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
}

scenario_build_lock_held_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export_build
  mkdir "$SB/tmp/vox-update.lock"
  run_script
  check "held lock: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "no bun call" no_log "bun "
  check "held lock left in place" [ -d "$SB/tmp/vox-update.lock" ]
}

scenario_all_default_still_installs() {
  setup_env "$1" || fail "sandbox setup failed"
  export VOX_UPDATE_ALLOW_ORIGIN="$ORIGIN"
  export VOX_UPDATE_STATUS_FILE="$STATUS"
  export VOX_UPDATE_RUN_ID="$BUILD_RUN_ID"
  export VOX_UPDATE_STATUS_HISTORY="$HIST"
  export VOX_UPDATE_LOCK_DIR="$SB/tmp/vox-update.lock"
  run_script
  check "default phase exit 0 (got $RUN_RC: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "default phase installs the bundle path it built" \
    log_has "install-ran src-tauri/target/release/bundle/macos/Vox.app"
  check "default phase ends done" [ "$(status_field "$STATUS" phase)" = "done" ]
  check "default phase stages nothing" [ ! -e "$STATE/staged" ]
  check "default phase writes no pending.json" [ ! -e "$PENDING" ]
}

# ---- scenarios: install phase ------------------------------------------------

scenario_install_happy_path() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  local target
  target="$(git -C "$CHECKOUT" rev-parse HEAD)"
  commit_pending
  run_install
  check "install exit 0 (got $RUN_RC: $(tr '\n' ' ' <"$RUN_OUT"))" [ "$RUN_RC" -eq 0 ]
  check "installer got the staged path and the target commit" \
    log_has "install-ran $STAGED commit=$target"
  check "install run made no bun call" no_log "bun "
  check "pending.json removed" [ ! -e "$PENDING" ]
  check "staged run dir removed" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
  check "status is done" [ "$(status_field "$STATUS" phase)" = "done" ]
  check "history: floor 91, monotonic, done 100 last" hist_ok "$HIST" 91 install
  check "lock released" [ ! -e "$SB/tmp/vox-update.lock" ]
}

scenario_install_tampered_exe_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  printf 'tampered\n' >>"$STAGED/Contents/MacOS/vox"
  run_install
  check "tampered executable: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "pending.json removed" [ ! -e "$PENDING" ]
  check "staged run dir removed" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
}

scenario_install_tampered_plist_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  printf '<!-- tampered -->\n' >>"$STAGED/Contents/Info.plist"
  run_install
  check "tampered Info.plist: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json removed" [ ! -e "$PENDING" ]
  check "staged run dir removed" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
}

scenario_install_state_built_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  run_install
  check "state still built: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "pending.json removed" [ ! -e "$PENDING" ]
}

scenario_install_wrong_run_id_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  export_install
  export VOX_UPDATE_RUN_ID="0000000000000000"
  run_script
  check "wrong run id: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json removed" [ ! -e "$PENDING" ]
}

scenario_install_noncanonical_staged_app_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  mkdir -p "$SB/evil"
  cp -R "$STAGED" "$SB/evil/Vox.app"
  json_set "$PENDING" staged_app "$SB/evil/Vox.app"
  run_install
  check "staged_app outside the canonical path: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json removed" [ ! -e "$PENDING" ]
  check "the other copy was not touched" [ -d "$SB/evil/Vox.app" ]
}

scenario_install_symlinked_staged_dir_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  mv "$STATE/staged/$BUILD_RUN_ID" "$SB/moved"
  ln -s "$SB/moved" "$STATE/staged/$BUILD_RUN_ID"
  run_install
  check "symlinked staged dir: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "the real directory behind the link is intact" [ -d "$SB/moved/Vox.app" ]
}

scenario_install_missing_pending_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  export_install
  run_script
  check "missing pending.json: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
}

scenario_install_installer_failure_cleans_up() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  export FAKE_INSTALLER_RC=1
  run_install
  check "installer failure: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer was called" log_has "install-ran $STAGED"
  check "status is failed" [ "$(status_field "$STATUS" phase)" = "failed" ]
  check "pending.json removed" [ ! -e "$PENDING" ]
  check "staged run dir removed" [ ! -e "$STATE/staged/$BUILD_RUN_ID" ]
}

scenario_install_bad_target_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  json_set "$PENDING" target_full "abc"
  run_install
  check "bad target_full: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json removed" [ ! -e "$PENDING" ]
}

scenario_install_bad_exe_name_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  json_set "$PENDING" exe_name "../vox"
  run_install
  check "bad exe_name: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json removed" [ ! -e "$PENDING" ]
}

scenario_install_bad_version_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  json_set "$PENDING" v 2
  run_install
  check "unsupported version: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
}

scenario_install_bad_floor_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  export_install
  export VOX_UPDATE_PERCENT_FLOOR=abc
  run_script
  check "bad floor: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
  check "pending.json untouched by a refused setup" [ -f "$PENDING" ]
}

scenario_install_relative_pending_refused() {
  setup_env "$1" || fail "sandbox setup failed"
  prepare_built
  commit_pending
  export_install
  export VOX_UPDATE_PENDING_FILE="pending.json"
  run_script
  check "relative pending path: non-zero exit" [ "$RUN_RC" -ne 0 ]
  check "installer not called" no_log "install-ran"
}

# ---- run ---------------------------------------------------------------------

REASON_FILE=""
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
  rm -f "$sb.out" "$REASON_FILE"
}

run_scenario "(B1) build: pending.json complete and digest-true, waiting 91, no install" scenario_build_happy_path
run_scenario "(B2) build failure: failed, nothing left behind" scenario_build_failure_cleans_up
run_scenario "(B3) stale pending.json does not survive a failed build" scenario_build_stale_pending_removed
run_scenario "(B4) stale pending.json replaced by a good build" scenario_build_stale_pending_replaced
run_scenario "(B5) symlinked staged parent refused" scenario_build_symlinked_staged_parent_refused
run_scenario "(B6) bad VOX_UPDATE_ORIGIN refused before any command" scenario_build_bad_origin_refused
run_scenario "(B7) bad VOX_UPDATE_PHASE refused before any command" scenario_build_bad_phase_refused
run_scenario "(B8) build without VOX_UPDATE_STATE_DIR refused" scenario_build_needs_state_dir
run_scenario "(B9) symlinked executable in the fresh bundle refused" scenario_build_symlinked_exe_refused
run_scenario "(B10) held lock: refused and the lock is left alone" scenario_build_lock_held_refused
run_scenario "(B11) default phase unchanged: installs, done, stages nothing" scenario_all_default_still_installs
run_scenario "(I1) install happy path: exact staged path, done last, files removed, floor 91" scenario_install_happy_path
run_scenario "(I2) tampered executable: digest mismatch, failed, no install" scenario_install_tampered_exe_refused
run_scenario "(I3) tampered Info.plist: digest mismatch, failed, no install" scenario_install_tampered_plist_refused
run_scenario "(I4) state still built: failed, no install" scenario_install_state_built_refused
run_scenario "(I5) wrong run id: failed, no install" scenario_install_wrong_run_id_refused
run_scenario "(I6) staged_app not the canonical path: failed, no install" scenario_install_noncanonical_staged_app_refused
run_scenario "(I7) symlinked staged directory: failed, no install, link target intact" scenario_install_symlinked_staged_dir_refused
run_scenario "(I8) missing pending.json: failed, no install" scenario_install_missing_pending_refused
run_scenario "(I9) installer failure: failed, files removed" scenario_install_installer_failure_cleans_up
run_scenario "(I10) bad target_full: failed, no install" scenario_install_bad_target_refused
run_scenario "(I11) bad exe_name: failed, no install" scenario_install_bad_exe_name_refused
run_scenario "(I12) unsupported pending version: failed, no install" scenario_install_bad_version_refused
run_scenario "(I13) bad percent floor: refused before the lock" scenario_install_bad_floor_refused
run_scenario "(I14) relative VOX_UPDATE_PENDING_FILE refused" scenario_install_relative_pending_refused

echo "$PASSED passed, $FAILED failed"
[ "$FAILED" -eq 0 ]
