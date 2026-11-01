# update-staging.sh - build and install phases of scripts/update-macos.sh.
#
# Meant to be SOURCED (bash 3.2 compatible). Defines functions and globals
# only; no `set` changes, no `exit`.
#
# Two-phase run (VOX_UPDATE_PHASE):
#   all      (default) the single-phase behaviour: build, install, done.
#   build    runs everything up to the bundle check, then stages a copy of the
#            fresh Vox.app under <state>/staged/<run_id>/, writes
#            <state>/pending.json and writes status `waiting` (91). It never
#            installs.
#   install  takes the same lock, reads pending.json and installs exactly the
#            staged app it names, after re-checking the staged copy. It runs no
#            git, bun or cargo.
#
# Environment read here:
#   VOX_UPDATE_PHASE          all | build | install (default all)
#   VOX_UPDATE_STATE_DIR      absolute, existing directory; required by build
#                             and install
#   VOX_UPDATE_PENDING_FILE   install only: absolute path of pending.json
#   VOX_UPDATE_PERCENT_FLOOR  install only: 0..100, default 0; the first status
#                             percent of the install run (the bar continues)
#   VOX_UPDATE_ORIGIN         build only: auto | manual (default manual)
#   VOX_PLUTIL, VOX_SHASUM    tool overrides (default plutil, shasum)
#
# pending.json, schema version 1 (build writes it, install reads it). Every
# field is mandatory; values are validated before they are written or used:
#   {"v":1,"run_id":"<16 hex>","target_full":"<40 hex>",
#    "staged_app":"<state>/staged/<run_id>/Vox.app","exe_name":"<name>",
#    "exe_sha256":"<64 hex>","info_plist_sha256":"<64 hex>",
#    "state":"built","origin":"auto|manual","ts":<unix seconds>}
# Before an install run, the committing side changes "state" to "committing"
# and adds "install_run_id":"<16 hex>". Install accepts only that state.
#
# Cleanup: a failed build or install removes pending.json and the staged
# directory of the run. A successful install removes both before `done`.

STAGING_PHASE="${VOX_UPDATE_PHASE:-all}"
STAGING_PENDING=""
STAGING_STAGED_DIR=""
INSTALL_TARGET=""
INSTALL_STAGED_APP=""

# staging_reject <reason>: short message on stderr, reason kept for the status
# file, returns 1.
staging_reject() {
  echo "$1" >&2
  STATUS_FAIL_REASON="$1"
  return 1
}

# staging_plain_abs_path <path>: 0 iff absolute and free of quotes, backslashes
# and control characters (so it can be written into JSON without escaping).
staging_plain_abs_path() {
  case "$1" in
    /*) ;;
    *) return 1 ;;
  esac
  case "$1" in
    *\"* | *\\*) return 1 ;;
  esac
  [ -z "$(printf '%s' "$1" | LC_ALL=C tr -d '[:print:]')" ]
}

# staging_refuse_symlinks <path>...: 1 if any of the paths is a symbolic link.
staging_refuse_symlinks() {
  local p
  for p in "$@"; do
    if [ -L "$p" ]; then
      echo "Refusing: $p is a symbolic link." >&2
      return 1
    fi
  done
  return 0
}

# staging_valid_exe_name <name>: 0 iff the name uses only the allowed charset.
staging_valid_exe_name() {
  local re='^[A-Za-z0-9._-]+$'
  [[ "$1" =~ $re ]]
}

# staging_regular_file <path>: 0 iff a regular file and not a symlink.
staging_regular_file() {
  [ -f "$1" ] && [ ! -L "$1" ]
}

# staging_sha256 <file>: prints the lowercase sha256 of exactly that file.
staging_sha256() {
  local out
  out="$("${VOX_SHASUM:-shasum}" -a 256 "$1" 2>/dev/null)" || return 1
  out="${out%% *}"
  if ! status_hex_of_len "$out" 64; then
    return 1
  fi
  printf '%s' "$out"
}

# staging_plist_value <file> <key>: the raw value, or nothing on failure.
staging_plist_value() {
  "${VOX_PLUTIL:-plutil}" -extract "$2" raw -o - "$1" 2>/dev/null
}

# staging_validate_setup: checks the phase environment. Runs before the lock.
# Returns 1 with a message for a bad setup. `all` needs nothing extra.
staging_validate_setup() {
  STAGING_PHASE="${VOX_UPDATE_PHASE:-all}"
  case "$STAGING_PHASE" in
    all) return 0 ;;
    build | install) ;;
    *)
      echo "VOX_UPDATE_PHASE must be all, build or install." >&2
      return 1
      ;;
  esac
  if ! status_enabled; then
    echo "VOX_UPDATE_PHASE=$STAGING_PHASE needs VOX_UPDATE_STATUS_FILE." >&2
    return 1
  fi
  if [ -z "${VOX_UPDATE_STATE_DIR:-}" ]; then
    echo "VOX_UPDATE_PHASE=$STAGING_PHASE needs VOX_UPDATE_STATE_DIR." >&2
    return 1
  fi
  if ! staging_plain_abs_path "$VOX_UPDATE_STATE_DIR" ||
    [ ! -d "$VOX_UPDATE_STATE_DIR" ] || [ -L "$VOX_UPDATE_STATE_DIR" ]; then
    echo "VOX_UPDATE_STATE_DIR must be an existing absolute directory (not a symlink)." >&2
    return 1
  fi
  if [ "$STAGING_PHASE" = build ]; then
    case "${VOX_UPDATE_ORIGIN:-manual}" in
      auto | manual) ;;
      *)
        echo "VOX_UPDATE_ORIGIN must be auto or manual." >&2
        return 1
        ;;
    esac
  fi
  if [ "$STAGING_PHASE" = install ]; then
    if ! staging_plain_abs_path "${VOX_UPDATE_PENDING_FILE:-}"; then
      echo "VOX_UPDATE_PENDING_FILE must be an absolute path." >&2
      return 1
    fi
    case "${VOX_UPDATE_PERCENT_FLOOR:-0}" in
      "" | *[!0-9]*)
        echo "VOX_UPDATE_PERCENT_FLOOR must be a number from 0 to 100." >&2
        return 1
        ;;
    esac
    if [ "${VOX_UPDATE_PERCENT_FLOOR:-0}" -gt 100 ]; then
      echo "VOX_UPDATE_PERCENT_FLOOR must be a number from 0 to 100." >&2
      return 1
    fi
  fi
  return 0
}

# staging_begin_phase: runs after the lock is ours. Build removes any stale
# pending.json first. Install starts the status percent at the floor.
staging_begin_phase() {
  case "$STAGING_PHASE" in
    build)
      STAGING_PENDING="$VOX_UPDATE_STATE_DIR/pending.json"
      rm -f "$STAGING_PENDING"
      ;;
    install)
      STAGING_PENDING="$VOX_UPDATE_PENDING_FILE"
      STATUS_LAST_PERCENT="${VOX_UPDATE_PERCENT_FLOOR:-0}"
      ;;
  esac
  return 0
}

# staging_write_pending <run_id> <target_full> <staged_app> <exe_name>
#                       <exe_sha256> <info_plist_sha256> <origin>
# Atomic (temp file + mv), mode 600. Values were validated by the caller.
staging_write_pending() {
  local file="$VOX_UPDATE_STATE_DIR/pending.json"
  local tmp="$VOX_UPDATE_STATE_DIR/.pending.json.tmp.$$"
  local ts line
  ts="$(date +%s)"
  line="{\"v\":1,\"run_id\":\"$1\",\"target_full\":\"$2\",\"staged_app\":\"$3\",\"exe_name\":\"$4\",\"exe_sha256\":\"$5\",\"info_plist_sha256\":\"$6\",\"state\":\"built\",\"origin\":\"$7\",\"ts\":$ts}"
  rm -f "$tmp"
  if (umask 077 && printf '%s\n' "$line" >"$tmp") && chmod 600 "$tmp" && mv -f "$tmp" "$file"; then
    return 0
  fi
  rm -f "$tmp"
  echo "Could not write $file -- not staging." >&2
  return 1
}

# staging_stage_build <built-app> <target_full>: copies the fresh bundle into
# the staged directory, checks it, writes pending.json, then writes `waiting`.
staging_stage_build() {
  local app="$1" target="$2"
  local state="$VOX_UPDATE_STATE_DIR"
  local staged="$state/staged"
  local dest="$state/staged/$VOX_UPDATE_RUN_ID"
  local name exe plist exe_sha plist_sha

  if ! status_hex_of_len "$target" 40; then
    echo "Target commit is not a 40-character SHA -- not staging." >&2
    return 1
  fi
  staging_refuse_symlinks "$state" "$staged" "$dest" || return 1
  mkdir -p "$staged" && chmod 700 "$staged" || return 1
  # The parent may have been created just now; refuse again before touching it.
  staging_refuse_symlinks "$staged" "$dest" || return 1
  STAGING_STAGED_DIR="$dest"
  rm -rf "$dest"
  mkdir "$dest" && chmod 700 "$dest" || return 1
  cp -R "$app" "$dest/Vox.app" || return 1

  name="$(staging_plist_value "$dest/Vox.app/Contents/Info.plist" CFBundleExecutable)"
  if ! staging_valid_exe_name "$name"; then
    echo "CFBundleExecutable is missing or has unsupported characters -- not staging." >&2
    return 1
  fi
  exe="$dest/Vox.app/Contents/MacOS/$name"
  plist="$dest/Vox.app/Contents/Info.plist"
  if ! staging_regular_file "$exe" || ! staging_regular_file "$plist"; then
    echo "The fresh bundle has no regular $name executable -- not staging." >&2
    return 1
  fi
  exe_sha="$(staging_sha256 "$exe")" || return 1
  plist_sha="$(staging_sha256 "$plist")" || return 1

  staging_write_pending "$VOX_UPDATE_RUN_ID" "$target" "$dest/Vox.app" \
    "$name" "$exe_sha" "$plist_sha" "${VOX_UPDATE_ORIGIN:-manual}" || return 1
  status_write waiting 91 "Ready to install" exact
  return 0
}

# staging_finish_build <built-app> <target_full>: the build phase's last step.
# On failure the caller exits non-zero; the EXIT cleanup removes the staged
# copy and pending.json.
staging_finish_build() {
  STATUS_FAIL_REASON="Could not stage the app for install"
  if ! staging_stage_build "$1" "$2"; then
    STATUS_FAIL_REASON="Could not stage the app for install"
    return 1
  fi
  return 0
}

# staging_validate_pending: strict checks of pending.json for the install
# phase. On success sets INSTALL_TARGET and INSTALL_STAGED_APP and records the
# staged directory for cleanup. On failure returns 1 with a short reason.
staging_validate_pending() {
  local file="$VOX_UPDATE_PENDING_FILE"
  local state="$VOX_UPDATE_STATE_DIR"
  local v st run_id ins target staged name exe_sha plist_sha origin ts
  local app dir exe plist want_exe want_plist

  if [ ! -f "$file" ] || [ -L "$file" ]; then
    staging_reject "No pending install (pending.json is missing)" || return 1
  fi
  v="$("${VOX_PLUTIL:-plutil}" -extract v raw -o - "$file" 2>/dev/null)" || v=""
  st="$("${VOX_PLUTIL:-plutil}" -extract state raw -o - "$file" 2>/dev/null)" || st=""
  run_id="$("${VOX_PLUTIL:-plutil}" -extract run_id raw -o - "$file" 2>/dev/null)" || run_id=""
  ins="$("${VOX_PLUTIL:-plutil}" -extract install_run_id raw -o - "$file" 2>/dev/null)" || ins=""
  target="$("${VOX_PLUTIL:-plutil}" -extract target_full raw -o - "$file" 2>/dev/null)" || target=""
  staged="$("${VOX_PLUTIL:-plutil}" -extract staged_app raw -o - "$file" 2>/dev/null)" || staged=""
  name="$("${VOX_PLUTIL:-plutil}" -extract exe_name raw -o - "$file" 2>/dev/null)" || name=""
  exe_sha="$("${VOX_PLUTIL:-plutil}" -extract exe_sha256 raw -o - "$file" 2>/dev/null)" || exe_sha=""
  plist_sha="$("${VOX_PLUTIL:-plutil}" -extract info_plist_sha256 raw -o - "$file" 2>/dev/null)" || plist_sha=""
  origin="$("${VOX_PLUTIL:-plutil}" -extract origin raw -o - "$file" 2>/dev/null)" || origin=""
  ts="$("${VOX_PLUTIL:-plutil}" -extract ts raw -o - "$file" 2>/dev/null)" || ts=""

  [ "$v" = "1" ] || staging_reject "Pending install has an unsupported version" || return 1
  [ "$st" = "committing" ] || staging_reject "Pending install is not committed" || return 1
  status_hex_of_len "$run_id" 16 || staging_reject "Pending install has a bad run id" || return 1
  if [ "$ins" != "$VOX_UPDATE_RUN_ID" ]; then
    # Another run's record: it is never this run's cleanup target (B6 minor 9).
    STAGING_PENDING=""
    staging_reject "Pending install belongs to another run" || return 1
  fi
  status_hex_of_len "$target" 40 || staging_reject "Pending install has a bad target" || return 1
  case "$origin" in
    auto | manual) ;;
    *) staging_reject "Pending install has a bad origin" || return 1 ;;
  esac
  case "$ts" in
    "" | *[!0-9]*) staging_reject "Pending install has a bad timestamp" || return 1 ;;
  esac

  # From here on the staged directory is known and may be cleaned up.
  dir="$state/staged/$run_id"
  STAGING_STAGED_DIR="$dir"
  app="$dir/Vox.app"
  if [ "$staged" != "$app" ]; then
    staging_reject "Pending install points outside the staged area" || return 1
  fi
  staging_refuse_symlinks "$state" "$state/staged" "$dir" "$app" ||
    staging_reject "Staged app path has a symbolic link" || return 1
  if [ ! -d "$app" ]; then
    staging_reject "Staged app is missing" || return 1
  fi
  if ! staging_valid_exe_name "$name"; then
    staging_reject "Pending install has a bad executable name" || return 1
  fi
  exe="$app/Contents/MacOS/$name"
  plist="$app/Contents/Info.plist"
  if ! staging_regular_file "$exe" || ! staging_regular_file "$plist"; then
    staging_reject "Staged app has no regular executable" || return 1
  fi
  want_exe="$(staging_sha256 "$exe")" || staging_reject "Could not hash the staged executable" || return 1
  want_plist="$(staging_sha256 "$plist")" || staging_reject "Could not hash the staged Info.plist" || return 1
  if [ "$want_exe" != "$exe_sha" ] || [ "$want_plist" != "$plist_sha" ]; then
    staging_reject "Staged app does not match the recorded build (digest mismatch)" || return 1
  fi

  INSTALL_TARGET="$target"
  INSTALL_STAGED_APP="$app"
  return 0
}

# staging_cleanup: removes pending.json and this run's staged directory. Used on
# failure (build and install) and after an install. A no-op for `all`.
staging_cleanup() {
  case "$STAGING_PHASE" in
    build | install) ;;
    *) return 0 ;;
  esac
  if [ -n "$STAGING_PENDING" ]; then
    rm -f "$STAGING_PENDING" 2>/dev/null || true
  fi
  if [ -n "$STAGING_STAGED_DIR" ]; then
    rm -rf "$STAGING_STAGED_DIR" 2>/dev/null || true
  fi
  return 0
}

# staging_on_exit <rc>: EXIT-trap part for the phases. A failed build cleans up.
# An install always cleans up. A build that ended in `waiting` (rc 0) keeps its
# staged copy and pending.json.
staging_on_exit() {
  case "$STAGING_PHASE" in
    build)
      if [ "$1" -ne 0 ]; then
        staging_cleanup
      fi
      ;;
    install) staging_cleanup ;;
  esac
  return 0
}

# staging_run_install: the install phase after the lock is taken. Returns the
# installer's status; the caller exits with it. The EXIT trap writes `failed`.
staging_run_install() {
  local rc=0
  if ! staging_validate_pending; then
    return 1
  fi
  status_set_target "$INSTALL_TARGET" ""
  export VOX_BUILD_COMMIT="$INSTALL_TARGET" VOX_SOURCE_UPDATES=1
  status_step installing 92 "Installing Vox" "Install failed"
  echo "Installing the staged build ${INSTALL_TARGET}..."
  ./scripts/install-macos.sh "$INSTALL_STAGED_APP" || rc=$?
  if [ "$rc" -ne 0 ]; then
    STATUS_FAIL_REASON="Install failed"
    staging_cleanup
    return "$rc"
  fi
  staging_cleanup
  status_step restarting 97 "Finishing the update" "Update failed after the install"
  echo ""
  echo "== Install complete: ${INSTALL_TARGET:0:7} =="
  # done is the very last status action; the lock is released after this.
  status_write done 100 "Update complete" exact
  return 0
}
