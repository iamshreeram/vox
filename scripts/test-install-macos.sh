#!/bin/bash
# Tests for the staged, rollback-safe macOS installer (scripts/install-macos.sh)
# and its helper library (scripts/lib/swap-app.sh).
#
# Bash 3.2 compatible (macOS /bin/bash). Every scenario runs in a fresh sandbox
# with HOME, VOX_INSTALL_DEST_APP and TMPDIR inside the sandbox, and a stub
# directory first in PATH for every external command that could touch the
# machine. Real /Applications, ~/Library, launchctl and processes are never used.
#
# INSTALLER_OVERRIDE=<dir> points the harness at another directory containing
# install-macos.sh and lib/swap-app.sh (used to validate the harness itself).
#
# Installer-running scenarios are refused (reported as FAIL) if the installer
# does not honor VOX_INSTALL_DEST_APP, because the installer would otherwise
# operate on the real /Applications/Vox.app.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER_DIR="${INSTALLER_OVERRIDE:-$ROOT/scripts}"
INSTALLER="$INSTALLER_DIR/install-macos.sh"
LIB="$INSTALLER_DIR/lib/swap-app.sh"
PLIST_BASENAME="com.iamshreeram.vox.agent.plist"

PASS_N=0
FAIL_N=0
SKIP_N=0

TESTROOT="$(mktemp -d "${TMPDIR:-/tmp}/vox-install-tests.XXXXXX")" || exit 1
trap 'chmod -R u+rwx "$TESTROOT" 2>/dev/null; rm -rf "$TESTROOT"' EXIT

# ---------------------------------------------------------------- stubs ----

write_stublib() {
  cat > "$SB/stubs/.stublib" <<'LIB_EOF'
stub_name="$(basename "$0")"
LAST_CALL=""
log_line() {
  LAST_CALL="$stub_name"
  for a in "$@"; do LAST_CALL="$LAST_CALL $a"; done
  printf '%s\n' "$LAST_CALL" >> "$STUB_LOG"
}
# maybe_fail <subcommand-or-empty>
# Fails (exit FAIL_CODE) when FAIL_AT has a token for this command, once per
# token unless FAIL_AT_ALWAYS=1.
maybe_fail() {
  for tok in ${FAIL_AT:-}; do
    hit=0
    if [ "$tok" = "$stub_name" ]; then hit=1; fi
    if [ -n "${1:-}" ] && [ "$tok" = "$stub_name:$1" ]; then hit=1; fi
    if [ "$hit" = 1 ]; then
      key=$(printf '%s' "$tok" | tr ':' '_')
      if [ "${FAIL_AT_ALWAYS:-0}" = 1 ] || [ ! -e "$STUB_STATE/fired.$key" ]; then
        : > "$STUB_STATE/fired.$key"
        printf '%s FAILED\n' "$LAST_CALL" >> "$STUB_LOG"
        exit "${FAIL_CODE:-7}"
      fi
    fi
  done
}
LIB_EOF
}

write_generic_stub() {
  cat > "$SB/stubs/$1" <<'GEN_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
maybe_fail ""
exit 0
GEN_EOF
  chmod +x "$SB/stubs/$1"
}

make_stubs() {
  mkdir -p "$SB/stubs"
  write_stublib
  for name in codesign xattr osascript open mdimport plutil defaults sw_vers \
              spctl killall tccutil mdutil lsregister xcrun; do
    write_generic_stub "$name"
  done

  cat > "$SB/stubs/launchctl" <<'LC_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
sub="${1:-}"
maybe_fail "$sub"
case "$sub" in
  print)
    if [ -e "$STUB_STATE/loaded" ]; then exit 0; fi
    exit 113 ;;
  bootout)
    rm -f "$STUB_STATE/loaded" "$STUB_STATE/running"; exit 0 ;;
  bootstrap)
    : > "$STUB_STATE/loaded"; exit 0 ;;
  kickstart)
    : > "$STUB_STATE/running"; exit 0 ;;
esac
exit 0
LC_EOF
  chmod +x "$SB/stubs/launchctl"

  cat > "$SB/stubs/pgrep" <<'PG_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
maybe_fail ""
if [ -e "$STUB_STATE/running" ]; then exit 0; fi
exit 1
PG_EOF
  chmod +x "$SB/stubs/pgrep"

  cat > "$SB/stubs/pkill" <<'PK_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
maybe_fail ""
rm -f "$STUB_STATE/running"
exit 0
PK_EOF
  chmod +x "$SB/stubs/pkill"

  cat > "$SB/stubs/security" <<'SEC_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
maybe_fail "${1:-}"
case "${1:-}" in
  find-identity)
    echo '  1) 0123456789ABCDEF0123456789ABCDEF01234567 "Vox Local Dev Codesign"'
    echo '     1 valid identities found' ;;
esac
exit 0
SEC_EOF
  chmod +x "$SB/stubs/security"

  cat > "$SB/stubs/curl" <<'CURL_EOF'
#!/bin/bash
. "$(dirname "$0")/.stublib"
log_line "$@"
maybe_fail ""
exit 1
CURL_EOF
  chmod +x "$SB/stubs/curl"
}

# ------------------------------------------------------------ fixtures -----

# make_app APP_PATH : fake freshly built app (new build)
make_app() {
  mkdir -p "$1/Contents/MacOS" "$1/Contents/Resources"
  printf 'MACHO-NEW' > "$1/Contents/MacOS/vox"
  printf '<?xml version="1.0"?><plist version="1.0"><dict/></plist>\n' > "$1/Contents/Info.plist"
  printf 'new\n' > "$1/Contents/Resources/marker-new"
  printf 'locked\n' > "$1/Contents/Resources/locked"
}

# make_old_app APP_PATH : previously installed (already wrapped) app
make_old_app() {
  mkdir -p "$1/Contents/MacOS" "$1/Contents/Resources"
  printf 'OLD-BIN' > "$1/Contents/MacOS/vox-bin"
  printf '#!/usr/bin/env bash\necho old-launcher\n' > "$1/Contents/MacOS/vox"
  chmod +x "$1/Contents/MacOS/vox"
  printf '<?xml version="1.0"?><plist version="1.0"><dict>old</dict></plist>\n' > "$1/Contents/Info.plist"
  printf 'old\n' > "$1/Contents/Resources/marker-old"
}

# make_previous : old install at DEST plus old LaunchAgent plist
make_previous() {
  make_old_app "$DEST"
  mkdir -p "$(dirname "$PLIST")"
  printf '<plist>OLD-PLIST-BYTES</plist>\n' > "$PLIST"
  cp "$DEST/Contents/MacOS/vox-bin" "$SB/old-bin.ref"
  OLD_PLIST_SUM="$(cksum < "$PLIST")"
}

# set_state LOADED RUNNING   (1 or 0)
set_state() {
  rm -f "$STATE/loaded" "$STATE/running"
  if [ "$1" = 1 ]; then : > "$STATE/loaded"; fi
  if [ "$2" = 1 ]; then : > "$STATE/running"; fi
  return 0
}

# ------------------------------------------------------------- helpers -----

add_fail() {
  SCN_FAILS="${SCN_FAILS}${SCN_FAILS:+; }$1"
}

# chk DESC CMD... : record failure of CMD (run in current shell) under DESC
chk() {
  local desc="$1"
  shift
  if ! "$@"; then add_fail "$desc"; fi
}

not() { ! "$@"; }

file_is() { [ "$(cat "$1" 2>/dev/null)" = "$2" ]; }
first2_is() { [ "$(head -c 2 "$1" 2>/dev/null)" = "$2" ]; }
same_plist() { [ "$(cksum < "$PLIST" 2>/dev/null)" = "$OLD_PLIST_SUM" ]; }
log_has() { grep -q -- "$1" "$LOG"; }
log_count() { grep -c -- "$1" "$LOG"; }
# ordered PAT1 PAT2 ...: first occurrences appear in this order in the log
ordered() {
  prev=0
  for p in "$@"; do
    cur=$(grep -n -- "$p" "$LOG" | head -1 | cut -d: -f1)
    if [ -z "$cur" ]; then return 1; fi
    if [ "$cur" -le "$prev" ]; then return 1; fi
    prev=$cur
  done
  return 0
}
# last_line_after PAT_A PAT_B: last line matching PAT_B comes after first PAT_A
after_failure() {
  fl=$(grep -n -- "$1" "$LOG" | head -1 | cut -d: -f1)
  ll=$(grep -n -- "$2" "$LOG" | tail -1 | cut -d: -f1)
  [ -n "$fl" ] && [ -n "$ll" ] && [ "$ll" -gt "$fl" ]
}
no_leftovers() { [ ! -e "$STAGING" ] && [ ! -e "$BACKUP" ]; }
both_absent() { [ ! -e "$1" ] && [ ! -e "$2" ]; }
dest_is_old() {
  [ -f "$DEST/Contents/Resources/marker-old" ] && [ ! -e "$DEST/Contents/Resources/marker-new" ] \
    && cmp -s "$DEST/Contents/MacOS/vox-bin" "$SB/old-bin.ref"
}

# run_installer SRC : runs the installer inside the sandbox
run_installer() {
  env -i PATH="$SB/stubs:/usr/bin:/bin:/usr/sbin:/sbin" \
    HOME="$SB/home" TMPDIR="$SB/tmp" \
    VOX_INSTALL_DEST_APP="$DEST" \
    STUB_LOG="$LOG" STUB_STATE="$STATE" \
    FAIL_AT="$FAIL_AT_V" FAIL_CODE="$FAIL_CODE_V" FAIL_AT_ALWAYS="$FAIL_ALWAYS_V" \
    /bin/bash "$INSTALLER" "$1" > "$OUT_FILE" 2>&1
  RC=$?
  OUT="$(cat "$OUT_FILE")"
}

require_installer() {
  if [ ! -f "$INSTALLER" ]; then
    add_fail "installer missing: $INSTALLER"
    return 1
  fi
  if ! grep -q 'VOX_INSTALL_DEST_APP' "$INSTALLER"; then
    add_fail "installer ignores VOX_INSTALL_DEST_APP; refusing to run it (it would touch the real /Applications)"
    return 1
  fi
  return 0
}

require_lib() {
  if [ ! -f "$LIB" ]; then
    add_fail "lib missing: $LIB"
    return 1
  fi
  return 0
}

begin() {
  SCN="$1"
  SCN_FAILS=""
  SCN_SKIP=0
  SCN_SKIP_REASON=""
  SB="$(mktemp -d "$TESTROOT/s.XXXXXX")"
  DEST="$SB/Applications/Vox.app"
  STAGING="$SB/Applications/.Vox.staging.app"
  BACKUP="$SB/Applications/.Vox.previous.app"
  PLIST="$SB/home/Library/LaunchAgents/$PLIST_BASENAME"
  LOG="$SB/stub.log"
  STATE="$SB/state"
  SRC="$SB/build/Vox.app"
  OUT_FILE="$SB/out.txt"
  OUT=""
  OLD_PLIST_SUM=""
  FAIL_AT_V=""
  FAIL_CODE_V=7
  FAIL_ALWAYS_V=0
  RC=0
  mkdir -p "$SB/home" "$SB/tmp" "$STATE" "$SB/Applications" "$SB/stubs"
  : > "$LOG"
  make_stubs
  make_app "$SRC"
}

end_scenario() {
  if [ "$SCN_SKIP" = 1 ]; then
    SKIP_N=$((SKIP_N + 1))
    echo "SKIP: $SCN -- $SCN_SKIP_REASON"
  elif [ -z "$SCN_FAILS" ]; then
    PASS_N=$((PASS_N + 1))
    echo "PASS: $SCN"
  else
    FAIL_N=$((FAIL_N + 1))
    echo "FAIL: $SCN -- $SCN_FAILS"
  fi
  chmod -R u+rwx "$SB" 2>/dev/null
  rm -rf "$SB"
}

# ----------------------------------------------------- installer scenarios --

s01_success_previous_running() {
  begin "1 success with previous install (running)"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  run_installer "$SRC"
  chk "exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" [ -f "$DEST/Contents/Resources/marker-new" ]
  chk "DEST has no marker-old" [ ! -e "$DEST/Contents/Resources/marker-old" ]
  chk "vox-bin is new binary" file_is "$DEST/Contents/MacOS/vox-bin" "MACHO-NEW"
  chk "vox is a bash launcher" first2_is "$DEST/Contents/MacOS/vox" "#!"
  chk "no staging/backup dirs remain" no_leftovers
  chk "new plist exists" [ -s "$PLIST" ]
  chk "new plist differs from old" not same_plist
  chk "log order bootout < bootstrap < kickstart" ordered '^launchctl bootout' '^launchctl bootstrap' '^launchctl kickstart'
  end_scenario
}

s02_success_first_install() {
  begin "2 success first install (nothing present)"
  require_installer || { end_scenario; return; }
  set_state 0 0
  run_installer "$SRC"
  chk "exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" [ -f "$DEST/Contents/Resources/marker-new" ]
  chk "log has bootstrap" log_has '^launchctl bootstrap'
  chk "no kickstart (was not running)" not log_has '^launchctl kickstart'
  chk "no staging/backup dirs remain" no_leftovers
  end_scenario
}

s03_phase_a_codesign_failure() {
  begin "3 phase A failure at codesign leaves live install untouched"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  FAIL_AT_V="codesign"
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "DEST still old (marker-old, vox-bin byte-identical)" dest_is_old
  chk "old plist checksum unchanged" same_plist
  chk "loaded state present" [ -e "$STATE/loaded" ]
  chk "running state present" [ -e "$STATE/running" ]
  chk "no launchctl bootout call" not log_has '^launchctl bootout'
  chk "no staging dir remains" [ ! -e "$STAGING" ]
  chk "no backup dir remains" [ ! -e "$BACKUP" ]
  end_scenario
}

s04_phase_a_copy_failure() {
  begin "4 phase A failure from unreadable file in SRC (copy error)"
  require_installer || { end_scenario; return; }
  if [ "$(id -u)" = 0 ]; then
    SCN_SKIP=1
    SCN_SKIP_REASON="running as root; chmod 000 is not enforced"
    end_scenario
    return
  fi
  make_previous
  set_state 1 1
  chmod 000 "$SRC/Contents/Resources/locked"
  run_installer "$SRC"
  chk "exit non-zero (got $RC)" [ "$RC" -ne 0 ]
  chk "DEST still old (marker-old, vox-bin byte-identical)" dest_is_old
  chk "old plist checksum unchanged" same_plist
  chk "loaded state present" [ -e "$STATE/loaded" ]
  chk "running state present" [ -e "$STATE/running" ]
  chk "no launchctl bootout call" not log_has '^launchctl bootout'
  chk "no staging dir remains" [ ! -e "$STAGING" ]
  chk "no backup dir remains" [ ! -e "$BACKUP" ]
  end_scenario
}

s05_phase_b_bootstrap_failure() {
  begin "5 phase B failure at launchctl bootstrap rolls back"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  FAIL_AT_V="launchctl:bootstrap"
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "DEST restored (marker-old, vox-bin OLD-BIN)" dest_is_old
  chk "old plist checksum identical" same_plist
  chk "old plist re-bootstrapped after the failure" after_failure 'launchctl bootstrap.*FAILED' '^launchctl bootstrap'
  chk "loaded state present after rollback" [ -e "$STATE/loaded" ]
  chk "running state present after rollback (kickstart)" [ -e "$STATE/running" ]
  chk "no backup/staging dirs left" no_leftovers
  end_scenario
}

s06_phase_b_kickstart_failure() {
  begin "6 phase B failure at launchctl kickstart rolls back"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  FAIL_AT_V="launchctl:kickstart"
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "DEST restored (marker-old, vox-bin OLD-BIN)" dest_is_old
  chk "old plist checksum identical" same_plist
  chk "old plist re-bootstrapped during rollback" after_failure 'launchctl kickstart.*FAILED' '^launchctl bootstrap'
  chk "no backup/staging dirs left" no_leftovers
  end_scenario
}

s07_first_install_bootstrap_failure() {
  begin "7 first install failure at launchctl bootstrap rolls back to nothing"
  require_installer || { end_scenario; return; }
  set_state 0 0
  FAIL_AT_V="launchctl:bootstrap"
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "no DEST remains" [ ! -e "$DEST" ]
  chk "no new plist remains" [ ! -e "$PLIST" ]
  chk "no staging/backup dirs left" no_leftovers
  end_scenario
}

s08_rollback_itself_fails() {
  begin "8 rollback itself fails: original status kept, rollback reported"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  FAIL_AT_V="launchctl:bootstrap"
  FAIL_ALWAYS_V=1
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "output mentions rollback" grep -qi 'rollback' "$OUT_FILE"
  chk "old DEST content still restored on disk" dest_is_old
  end_scenario
}

s09_rollback_not_running_no_kickstart() {
  begin "9 previous install not running, phase B failure: no kickstart"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 0
  FAIL_AT_V="launchctl:bootstrap"
  run_installer "$SRC"
  chk "exit == FAIL_CODE 7 (got $RC)" [ "$RC" -eq 7 ]
  chk "DEST restored (marker-old, vox-bin OLD-BIN)" dest_is_old
  chk "old plist checksum identical" same_plist
  chk "no kickstart logged at all" not log_has '^launchctl kickstart'
  chk "no backup/staging dirs left" no_leftovers
  end_scenario
}

s10a_rerun_twice() {
  begin "10a installer run successfully twice in a row"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  run_installer "$SRC"
  chk "first run exit 0 (got $RC)" [ "$RC" -eq 0 ]
  run_installer "$SRC"
  chk "second run exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new after second run" [ -f "$DEST/Contents/Resources/marker-new" ]
  chk "no marker-old after second run" [ ! -e "$DEST/Contents/Resources/marker-old" ]
  chk "no staging/backup dirs remain" no_leftovers
  end_scenario
}

s10b_stale_backup_from_crashed_run() {
  begin "10b stale .Vox.previous.app from earlier crash does not break a run"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  mkdir -p "$BACKUP/junk-dir"
  printf 'stale\n' > "$BACKUP/junk-dir/stale"
  run_installer "$SRC"
  chk "exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" [ -f "$DEST/Contents/Resources/marker-new" ]
  chk "no staging/backup dirs remain" no_leftovers
  end_scenario
}

s11_shebang_guard() {
  begin "11 shebang guard refuses wrapped SRC before touching DEST or launchctl"
  require_installer || { end_scenario; return; }
  make_previous
  set_state 1 1
  printf '#!/usr/bin/env bash\necho wrapped\n' > "$SRC/Contents/MacOS/vox"
  run_installer "$SRC"
  chk "exit non-zero (got $RC)" [ "$RC" -ne 0 ]
  chk "DEST untouched (marker-old, vox-bin OLD-BIN)" dest_is_old
  chk "old plist checksum unchanged" same_plist
  chk "no launchctl call at all" not log_has '^launchctl'
  chk "no staging dir remains" [ ! -e "$STAGING" ]
  end_scenario
}

# ------------------------------------------------------- lib unit scenarios --

s12_lib_stage_success() {
  begin "12a lib: stage_app copies SRC to STAGING"
  require_lib || { end_scenario; return; }
  ( . "$LIB" && stage_app "$SRC" "$STAGING" ); RC=$?
  chk "stage_app exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "staging has marker-new" file_is "$STAGING/Contents/Resources/marker-new" "new"
  chk "SRC left intact" [ -f "$SRC/Contents/Resources/marker-new" ]
  end_scenario
}

s12b_lib_stage_copy_failure() {
  begin "12b lib: stage_app copy failure is non-zero and leaves no STAGING"
  require_lib || { end_scenario; return; }
  if [ "$(id -u)" = 0 ]; then
    SCN_SKIP=1
    SCN_SKIP_REASON="running as root; chmod 000 is not enforced"
    end_scenario
    return
  fi
  chmod 000 "$SRC/Contents/Resources/locked"
  ( . "$LIB" && stage_app "$SRC" "$STAGING" ); RC=$?
  chk "stage_app exit non-zero (got $RC)" [ "$RC" -ne 0 ]
  chk "no STAGING left" [ ! -e "$STAGING" ]
  end_scenario
}

s12c_lib_swap_with_previous() {
  begin "12c lib: swap_in_app moves old DEST to BACKUP and STAGING to DEST"
  require_lib || { end_scenario; return; }
  make_previous
  ( . "$LIB" && stage_app "$SRC" "$STAGING" && swap_in_app "$STAGING" "$DEST" "$BACKUP" ); RC=$?
  chk "swap exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" file_is "$DEST/Contents/Resources/marker-new" "new"
  chk "BACKUP has marker-old" file_is "$BACKUP/Contents/Resources/marker-old" "old"
  chk "STAGING gone" [ ! -e "$STAGING" ]
  end_scenario
}

s12d_lib_swap_first_install() {
  begin "12d lib: swap_in_app works when DEST does not exist"
  require_lib || { end_scenario; return; }
  ( . "$LIB" && stage_app "$SRC" "$STAGING" && swap_in_app "$STAGING" "$DEST" "$BACKUP" ); RC=$?
  chk "swap exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" file_is "$DEST/Contents/Resources/marker-new" "new"
  chk "STAGING gone" [ ! -e "$STAGING" ]
  chk "no BACKUP created" [ ! -e "$BACKUP" ]
  end_scenario
}

s12e_lib_swap_replaces_stale_backup() {
  begin "12e lib: swap_in_app replaces a stale BACKUP"
  require_lib || { end_scenario; return; }
  make_previous
  mkdir -p "$BACKUP/junk-dir"
  printf 'stale\n' > "$BACKUP/junk-dir/stale"
  ( . "$LIB" && stage_app "$SRC" "$STAGING" && swap_in_app "$STAGING" "$DEST" "$BACKUP" ); RC=$?
  chk "swap exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "BACKUP has marker-old" file_is "$BACKUP/Contents/Resources/marker-old" "old"
  chk "stale file gone from BACKUP" [ ! -e "$BACKUP/junk-dir/stale" ]
  end_scenario
}

s12f_lib_rollback_previous_idempotent() {
  begin "12f lib: rollback_app HAD_PREVIOUS=1 restores old and is idempotent"
  require_lib || { end_scenario; return; }
  make_previous
  ( . "$LIB" && stage_app "$SRC" "$STAGING" && swap_in_app "$STAGING" "$DEST" "$BACKUP" ) >/dev/null 2>&1
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 1 ); RC=$?
  chk "first rollback exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST restored to marker-old" file_is "$DEST/Contents/Resources/marker-old" "old"
  chk "BACKUP gone after rollback" [ ! -e "$BACKUP" ]
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 1 ); RC=$?
  chk "second rollback exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "second rollback does not destroy restored old DEST" file_is "$DEST/Contents/Resources/marker-old" "old"
  end_scenario
}

s12g_lib_rollback_first_install_idempotent() {
  begin "12g lib: rollback_app HAD_PREVIOUS=0 removes new DEST and is idempotent"
  require_lib || { end_scenario; return; }
  ( . "$LIB" && stage_app "$SRC" "$STAGING" && swap_in_app "$STAGING" "$DEST" "$BACKUP" ) >/dev/null 2>&1
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 0 ); RC=$?
  chk "first rollback exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST removed" [ ! -e "$DEST" ]
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 0 ); RC=$?
  chk "second rollback exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST still absent" [ ! -e "$DEST" ]
  end_scenario
}

s12h_lib_rollback_nothing_present() {
  begin "12h lib: rollback_app with DEST and BACKUP missing returns 0"
  require_lib || { end_scenario; return; }
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 1 ); RC=$?
  chk "HAD_PREVIOUS=1 exit 0 (got $RC)" [ "$RC" -eq 0 ]
  ( . "$LIB" && rollback_app "$DEST" "$BACKUP" 0 ); RC=$?
  chk "HAD_PREVIOUS=0 exit 0 (got $RC)" [ "$RC" -eq 0 ]
  end_scenario
}

s12i_lib_discard_backup() {
  begin "12i lib: discard_backup removes BACKUP"
  require_lib || { end_scenario; return; }
  make_old_app "$BACKUP"
  ( . "$LIB" && discard_backup "$BACKUP" ); RC=$?
  chk "discard exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "BACKUP removed" [ ! -e "$BACKUP" ]
  end_scenario
}

s12j_lib_paths_with_spaces() {
  begin "12j lib: stage/swap/rollback work with spaces in paths"
  require_lib || { end_scenario; return; }
  local base="$SB/with space"
  local src="$base/src dir/Vox.app"
  local dst="$base/Apps dir/Vox.app"
  local stg="$base/Apps dir/.Vox.staging.app"
  local bak="$base/Apps dir/.Vox.previous.app"
  make_app "$src"
  make_old_app "$dst"
  ( . "$LIB" && stage_app "$src" "$stg" && swap_in_app "$stg" "$dst" "$bak" ); RC=$?
  chk "stage+swap exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST has marker-new" file_is "$dst/Contents/Resources/marker-new" "new"
  ( . "$LIB" && rollback_app "$dst" "$bak" 1 ); RC=$?
  chk "rollback exit 0 (got $RC)" [ "$RC" -eq 0 ]
  chk "DEST restored to marker-old" file_is "$dst/Contents/Resources/marker-old" "old"
  chk "no STAGING/BACKUP left" both_absent "$stg" "$bak" ]
  end_scenario
}

# ------------------------------------------------------------------- main ---

s01_success_previous_running
s02_success_first_install
s03_phase_a_codesign_failure
s04_phase_a_copy_failure
s05_phase_b_bootstrap_failure
s06_phase_b_kickstart_failure
s07_first_install_bootstrap_failure
s08_rollback_itself_fails
s09_rollback_not_running_no_kickstart
s10a_rerun_twice
s10b_stale_backup_from_crashed_run
s11_shebang_guard
s12_lib_stage_success
s12b_lib_stage_copy_failure
s12c_lib_swap_with_previous
s12d_lib_swap_first_install
s12e_lib_swap_replaces_stale_backup
s12f_lib_rollback_previous_idempotent
s12g_lib_rollback_first_install_idempotent
s12h_lib_rollback_nothing_present
s12i_lib_discard_backup
s12j_lib_paths_with_spaces

if [ "$SKIP_N" -gt 0 ]; then echo "($SKIP_N skipped)"; fi
echo "$PASS_N passed, $FAIL_N failed"
if [ "$FAIL_N" -gt 0 ]; then exit 1; fi
exit 0
