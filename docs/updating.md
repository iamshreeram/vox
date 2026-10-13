# Updating Vox from source ("new version available")

Vox can tell you when the `main` branch on GitHub is ahead of the build you are
running, and rebuild itself from that code with one click.

This applies to builds made by `scripts/update-macos.sh` on macOS. Builds that
do not come from that script (for example a downloaded release) keep using the
Tauri updater and are not affected.

## How it works

1. **Opt in at build time.** `scripts/update-macos.sh` builds with
   `VOX_SOURCE_UPDATES=1`, and records the exact commit (`VOX_BUILD_COMMIT`),
   branch and checkout directory in the binary. A plain `cargo build` or
   `bun run tauri build` embeds nothing and Vox stays in the Tauri-updater mode.
2. **Check.** On startup, every six hours, from the footer, and from the tray's
   "Check for updates" item, Vox asks the public GitHub API how far `main` is
   ahead of the embedded commit. It looks at the newest 100 commits first and
   falls back to GitHub's compare endpoint for older builds. No account or
   token is used. The check respects the "Check for updates" setting.
3. **Notify.** The footer shows "Update available" and, while an update is
   pending, the tray menu gains an "Update available" item that opens the
   Vox window. Nothing is installed without a confirmation dialog that lists
   the newest changes and links to the full comparison on GitHub.
4. **Install.** Confirming opens Terminal and runs `scripts/update-macos.sh`
   in your checkout. The script fast-forwards `main`, rebuilds, and installs the
   new app. Vox restarts when it is done.

## When the Update button is disabled

The dialog explains why. Typical reasons: the checkout has uncommitted changes
to tracked files, it is not on `main`, `origin` is not the official repository,
the checkout folder was moved or deleted, or another update holds the lock.

Forks can allow their own remote by exporting the exact URL before running the
script: `VOX_UPDATE_ALLOW_ORIGIN=git@github.com:me/vox.git ./scripts/update-macos.sh`.

## Safety properties

- Only an exact allow-list of the official repository's URLs is accepted as
  `origin`; look-alike hosts are rejected.
- The update only ever fast-forwards. A diverged local `main` is reported and
  left untouched; the script never creates a merge.
- A failed build never reaches the installer (the old script ignored build
  errors). A stale bundle from an earlier build is deleted before building.
- The installer stages the new app next to the old one and swaps them in, and
  restores the previous app, LaunchAgent and running state if anything fails.
- Only one update can run at a time, using a lock at
  `${TMPDIR:-/tmp}/vox-update.lock`. A leftover lock is never removed
  automatically. If an update was interrupted, the message printed tells you the
  exact `rm -rf` to run once you are sure nothing is still updating.

## Developer notes

- Logic: `src-tauri/src/managers/source_update.rs` (pure functions are unit
  tested; `cargo test --lib --offline live_github -- --ignored` also checks the
  parsers against the real GitHub API).
- UI state machine: `src/components/update-checker/sourceUpdateState.ts`
  (`bun run test:source-update`).
- Shell: `bun run test:scripts` runs the lock, update-script and installer
  harnesses (all external commands are stubbed; nothing touches `/Applications`).
