# Worker brief: process guard

This is cooperative admission control and evidence collection, not a sandbox or an account-wide process governor.

GUARD: Run cargo commands only through cargo-guard.sh, bun commands only through bun-guard.sh, and every other test or build command only through guard-run.sh. Read every line that starts with `guard:` and act on it. If the guard refuses a run (exit 75), report that and stop; do not work around it. You must never raise limits (ulimit, kern.maxproc, kern.maxprocperuid or any guard setting). You must never kill processes by name or pattern; kill only pids you started yourself.

Report any `guard:` line in your final report, copied word for word.
