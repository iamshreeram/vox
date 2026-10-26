#!/bin/bash
# Runs the dev-guard shell test suite (bash 3.2 compatible).
# Usage: /bin/bash scripts/dev-guard/tests/test-guard.sh
# Exit 0 = all tests pass; non-zero = at least one failure (or missing lib.sh).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DG_LIB_PATH="$SCRIPT_DIR/../lib.sh"
export SCRIPT_DIR DG_LIB_PATH
export DG_TEST=1

# Keep the caller's environment from leaking into the tests.
unset DG_WARN DG_HARD DG_RESERVE DG_FAKE_COUNT DG_FAKE_PS_FAIL

. "$SCRIPT_DIR/helpers.sh"
# EXIT handling: helpers.sh installs dg_run_exit (chains dg_on_exit hooks, then cleanup_tmp).

# Sourced in sorted (glob) order: every t_*.sh in this directory.
for t in "$SCRIPT_DIR"/t_*.sh; do
  if [ -f "$t" ]; then
    . "$t"
  fi
done

finish
