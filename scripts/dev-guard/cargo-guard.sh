#!/bin/bash
# cargo-guard.sh ARGS...: runs cargo through guard-run.sh, one cargo at a time
# across every worktree and agent (lock "cargo"). Arguments pass through
# unchanged. RUST_TEST_THREADS and CARGO_BUILD_JOBS default to 4 and keep any
# value the caller already set. DG_CARGO_TIMEOUT sets the timeout (default 3600 s).
#
# Scope: cooperative admission control, not a sandbox. See docs/dev-guard.md.
# Bash 3.2 compatible (macOS /bin/bash).

_CG_DIR=$(cd "$(dirname "$0")" && pwd) || exit 64
export RUST_TEST_THREADS=${RUST_TEST_THREADS:-4}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
exec "$_CG_DIR/guard-run.sh" --lock cargo --timeout "${DG_CARGO_TIMEOUT:-3600}" -- cargo "$@"
