# update-lock.sh - mutual exclusion for scripts/update-macos.sh.
#
# Meant to be SOURCED (bash 3.2 compatible). Defines functions only; no
# top-level side effects, no `set` changes, no `exit`.
#
#   acquire_update_lock <lockdir>
#     Atomically creates <lockdir> with mkdir. On success writes
#     <lockdir>/owner (via a temp file + mv, so it appears atomically),
#     sets UPDATE_LOCK_DIR and UPDATE_LOCK_TOKEN, returns 0.
#     If <lockdir> already exists for ANY reason it is never removed, renamed
#     or modified; a message with the path and a manual `rm -rf` hint is
#     printed to stderr and 1 is returned. There is deliberately NO automatic
#     stale-lock recovery: it cannot be made race-free.
#
#   release_update_lock
#     Removes UPDATE_LOCK_DIR only if <dir>/owner still contains
#     UPDATE_LOCK_TOKEN. Otherwise leaves the directory alone. Always returns 0
#     (also when nothing was acquired). Unsets the globals afterwards.

acquire_update_lock() {
  local dir="$1" token tmp
  if ! mkdir "$dir" 2>/dev/null; then
    if [ -e "$dir" ]; then
      echo "Another Vox update may be running (lock: $dir)." >&2
      echo "If you are sure none is, remove it with: rm -rf '$dir'" >&2
    else
      echo "Could not create update lock directory: $dir" >&2
    fi
    return 1
  fi

  # We created the directory, so we own it. Write the owner token atomically.
  token="$$-$(date +%s)-$RANDOM$RANDOM"
  tmp="$dir/.owner.$$.$RANDOM"
  if ! printf '%s\n' "$token" >"$tmp" || ! mv -f "$tmp" "$dir/owner"; then
    rm -f "$tmp" "$dir/owner"
    rmdir "$dir" 2>/dev/null
    echo "Could not write owner token in update lock: $dir" >&2
    return 1
  fi

  UPDATE_LOCK_DIR="$dir"
  UPDATE_LOCK_TOKEN="$token"
  return 0
}

release_update_lock() {
  local content=""
  if [ -n "${UPDATE_LOCK_DIR:-}" ] && [ -f "$UPDATE_LOCK_DIR/owner" ]; then
    content="$(cat "$UPDATE_LOCK_DIR/owner" 2>/dev/null || true)"
    if [ -n "${UPDATE_LOCK_TOKEN:-}" ] && [ "$content" = "$UPDATE_LOCK_TOKEN" ]; then
      rm -f "$UPDATE_LOCK_DIR/owner" 2>/dev/null || true
      rmdir "$UPDATE_LOCK_DIR" 2>/dev/null || true
    fi
  fi
  unset UPDATE_LOCK_DIR UPDATE_LOCK_TOKEN
  return 0
}
