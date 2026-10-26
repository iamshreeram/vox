# Dev process guard

This page explains the dev process guard. It lives in `scripts/dev-guard/`.

This is cooperative admission control and evidence collection, not a sandbox or an account-wide process governor.

## What happened

The Mac hit its per-user process limit and rebooted. The cause is not confirmed. The best guess is orphaned child processes from test fixtures, plus heavy parallel cargo runs. The guard does three things. It limits how many cargo and bun runs happen at once. It refuses to start new work when the process count is already high. It records evidence when processes are left behind.

## Install and daily use

Start the watcher once per session:

```sh
scripts/dev-guard/proc-guard.sh watch start
```

Check the count before you start heavy work:

```sh
scripts/dev-guard/proc-guard.sh check
```

`check` prints `procs=N level=L`. Exit codes are 0 for ok, 10 for warn, 20 for hard and 30 for unknown.

Run cargo through the wrapper:

```sh
scripts/dev-guard/cargo-guard.sh test --lib --locked
```

Run any other command through `guard-run.sh`:

```sh
scripts/dev-guard/guard-run.sh --timeout 600 -- ./some-tool
```

`guard-run.sh` takes `--timeout SECS`, `--lock NAME` and `--headroom N`. Exit 75 means the guard refused the run. Exit 124 means the timeout hit. guard-run exits 126 for setup failures BEFORE the command starts; once the command has started its own 126/127 status is passed through unchanged. Note that 127 can also mean the exec itself failed after GO (command not found or not executable), so a 127 does not prove that the command ran.

INT handling requires the guard-run process to have SIGINT enabled. A job started with `&` from a non-interactive shell inherits SIGINT as ignored, and bash cannot trap it. From agents, use TERM (exit 143) to cancel.

## What is enforced by the machine

- One cargo or bun run at a time, through the `cargo` and `bun` locks.
- A refusal before start when the count is warn, hard or unknown.
- A process limit inside the child, set from the current count plus a headroom. The guard never raises a limit. It only lowers the child's limit.
- A group kill on timeout, and at exit for the run's own process group.
- An orphan census. It records new processes that were reparented to launchd.

## What is only an instruction

- That agents and people use the wrappers.
- That ram-ai runs `check` before it dispatches sub-agents.
- That nobody runs background commands without `guard-run.sh`.
- That nobody raises limits or kills processes by name.

Other tools and apps can bypass the wrappers. The guard cannot stop that.

## Reading the census lines

After a guarded run, the guard may print this line:

```text
guard: unattributed new launchd-parented process(es): ...
```

This is a best-effort observation. The guard cannot tie these processes to the command it just ran. Short-lived processes can be missed. Legitimate launchd and GUI helpers also show up here. Treat the line as a clue to investigate, not as proof of a leak.

`ppid==1` means the parent process has exited, so launchd adopted the process. Many normal Mac helpers look the same. Use `proc-guard.sh orphans` to list them by command name. The command shows no arguments and no environment. Killing anything it lists is a manual decision by the user.

## Files in DG_HOME

`DG_HOME` defaults to `~/.vox-guard`. It must be a real directory owned by you with mode 0700.

- `proc-watch.log`: one line per sample, `ISO COUNT LEVEL` or `ISO unknown`. It also holds diagnostic blocks and ALERT lines. It rotates at 5 MB and keeps `.1` and `.2`.
- `log.bytes`: the approximate size of `proc-watch.log`, kept by the guard so an ordinary tick needs no stat call. It is seeded from the file size only when missing or garbled, and it resets to 0 at each rotation.
- `guard-run.log`: one line per guarded run. It holds the exit code, the duration and the census count.
- `locks/NAME.lock`: a lock directory. It holds an `owner` file with the pid, a token and the start time.
- `locks/watch.lock`: the watcher's lock. Its owner pid is the watcher's own pid.

The log holds only timestamps, counts, levels and command names. It never holds environment variables or full command lines.

## Manual recovery

If a lock stays busy, check the owner record first. If the owner pid is gone, the guard reclaims the lock on its own. If the owner record is missing, the guard waits and then gives up with this message:

```text
lock NAME has no owner record; if no guard process is running, remove DG_HOME/locks/NAME.lock manually
```

Only remove the directory after you confirm that no guard process is running. Check with `ps`.

## Limits

- Once no process slots remain, no shell guard can guarantee recovery. Some actions use builtins only, so they need fewer new processes. That is the best the guard can do.
- The guard does not change kernel limits such as `kern.maxproc` or `kern.maxprocperuid`.
- The inherited limit is a ceiling for the child. Other processes of the same user still use the same count.
- The scripts target macOS `/bin/bash` 3.2. They avoid bash 4 features.
- The `.reclaim-NAME` stale-lock retirement is check-then-act. This is a documented residual race that can at worst let two cargo runs overlap.

## Agent policy

- Check with `proc-guard.sh check` before you dispatch any sub-agent. Do not dispatch when the level is not ok.
- Run every cargo, bun, test and build command through `cargo-guard.sh`, `bun-guard.sh` or `guard-run.sh`.
- Do not start background commands without `guard-run.sh`.
- Read every `guard:` line after a run and report it.
- Never raise limits.
- Never kill processes by name or pattern. Kill only pids you started.
