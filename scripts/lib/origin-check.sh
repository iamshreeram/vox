# origin-check.sh - decides whether a git remote URL may be used by
# scripts/update-macos.sh.
#
# Meant to be SOURCED (bash 3.2 compatible). Defines functions only; no
# top-level side effects, no `set` changes, no `exit`.
#
#   origin_allowed <url>
#     Returns 0 iff <url> equals EXACTLY one of the official Vox remotes, or
#     equals the non-empty value of VOX_UPDATE_ALLOW_ORIGIN (intended for
#     forks / local testing). Otherwise returns 1. Comparisons are literal
#     (case patterns contain no glob characters; the override is compared
#     with a quoted string), so no trimming and no pattern matching happens.

origin_allowed() {
  local url="$1" allowed=""

  if [ -z "$url" ]; then
    return 1
  fi

  case "$url" in
    "https://github.com/iamshreeram/vox" | \
      "https://github.com/iamshreeram/vox.git" | \
      "git@github.com:iamshreeram/vox" | \
      "git@github.com:iamshreeram/vox.git" | \
      "ssh://git@github.com/iamshreeram/vox" | \
      "ssh://git@github.com/iamshreeram/vox.git")
      return 0
      ;;
  esac

  allowed="${VOX_UPDATE_ALLOW_ORIGIN:-}"
  if [ -n "$allowed" ] && [ "$url" = "$allowed" ]; then
    return 0
  fi

  return 1
}
