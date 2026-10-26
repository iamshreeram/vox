#!/bin/bash
# bun-guard.sh ARGS...: runs bun through guard-run.sh, one bun at a time across
# every worktree and agent (lock "bun"). Arguments pass through unchanged.
# DG_BUN_TIMEOUT sets the timeout (default 3600 s).
#
# Scope: cooperative admission control, not a sandbox. See docs/dev-guard.md.
# Bash 3.2 compatible (macOS /bin/bash).

_BG_DIR=$(cd "$(dirname "$0")" && pwd) || exit 64
exec "$_BG_DIR/guard-run.sh" --lock bun --timeout "${DG_BUN_TIMEOUT:-3600}" -- bun "$@"
