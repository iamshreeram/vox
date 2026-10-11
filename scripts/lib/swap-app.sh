# swap-app.sh -- helpers for the staged, rollback-safe macOS install.
#
# SOURCED by scripts/install-macos.sh. Must stay free of top-level side
# effects: no commands run at source time, no `set` changes, no `exit`.
# Bash 3.2 compatible (macOS /bin/bash): uses only plain indexed variables and
# portable constructs. Every path is quoted, because paths may contain spaces.
#
# All functions return 0 on success and non-zero on a real filesystem
# failure. They never print anything; the installer decides what to report.

# stage_app SRC STAGING
#   Copies SRC to STAGING. On failure, removes any partial STAGING and
#   returns non-zero. SRC is never modified.
stage_app() {
  local src="$1"
  local staging="$2"
  rm -rf "$staging" || return 1
  if ! cp -R "$src" "$staging"; then
    rm -rf "$staging"
    return 1
  fi
  return 0
}

# swap_in_app STAGING DEST BACKUP
#   Moves the current DEST (if any) to BACKUP, then moves STAGING into DEST.
#   A stale BACKUP from an earlier crashed run is removed first.
swap_in_app() {
  local staging="$1"
  local dest="$2"
  local backup="$3"
  rm -rf "$backup" || return 1
  if [ -e "$dest" ]; then
    mv "$dest" "$backup" || return 1
  fi
  mv "$staging" "$dest" || return 1
  return 0
}

# rollback_app DEST BACKUP HAD_PREVIOUS
#   Undoes swap_in_app. HAD_PREVIOUS=1: if BACKUP exists, DEST is replaced by
#   BACKUP. If BACKUP is absent, DEST is left alone, because the previous
#   install was never moved out of the way and must not be deleted.
#   HAD_PREVIOUS=0: DEST (the new first install) is removed.
#   Idempotent; missing paths are not an error.
rollback_app() {
  local dest="$1"
  local backup="$2"
  local had_previous="$3"
  if [ "$had_previous" = 1 ]; then
    if [ -e "$backup" ]; then
      rm -rf "$dest" || return 1
      mv "$backup" "$dest" || return 1
    fi
    return 0
  fi
  rm -rf "$dest" || return 1
  return 0
}

# discard_backup BACKUP
#   Removes the previous-install backup once the new install is live.
discard_backup() {
  rm -rf "$1" || return 1
  return 0
}
