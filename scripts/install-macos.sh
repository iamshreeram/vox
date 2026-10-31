#!/usr/bin/env bash
# One command to (re)install Vox.app as a properly launchable macOS app:
# findable in Spotlight, launchable from Dock/Finder with a real window,
# AND not re-broken by the next `tauri build`.
#
# Why this script exists at all (two distinct, previously-confused bugs --
# see the full war story in AGENTS.md/session notes if curious):
#
#   1. A debug-only typescript-bindings-export panic in lib.rs used to
#      crash the app instantly on any launch whose working directory
#      wasn't the cargo project root (i.e. every real-world launch except
#      a shell happening to cd into this repo first). Fixed at the source
#      now (src-tauri/src/lib.rs) -- that part needs no workaround here.
#
#   2. This app is only ad-hoc signed (no paid Apple Developer ID, no
#      notarization). On top of plain Gatekeeper rejection, macOS
#      RunningBoard silently denies such apps the ability to present ANY
#      UI when launched via LaunchServices (Finder/Dock/Spotlight/open) --
#      the process runs with zero errors, it just can never show a window
#      or tray icon. No local workaround (self-signed trusted certs,
#      `spctl --add`, `spctl --master-disable`) gets around this on recent
#      macOS -- confirmed exhaustively. The only way out: never let the
#      real binary be launched *through* LaunchServices at all. A launchd
#      user agent runs it directly instead, which never triggers that
#      check. This exact pattern is already proven out in the sibling
#      Python vox project's own scripts/install.sh -- this is the same
#      fix, transplanted for the Rust/Tauri build.
#
# Staged, rollback-safe install (why it is two phases):
#
#   Replacing a running app in place is the risky part. A failure halfway
#   through used to leave /Applications/Vox.app half-built, the LaunchAgent
#   removed, and nothing running. So the install now works in two phases:
#
#   Phase A -- staging, NO effect on the live install. The fresh build is
#   copied to a hidden sibling of the destination,
#   $(dirname DEST)/.Vox.staging.app. The launcher wrapper is installed and
#   the staged bundle is signed there. If anything fails in phase A, the
#   staging copy is removed and the live app, its LaunchAgent plist and the
#   running agent are left exactly as they were. launchctl is not called.
#
#   Phase B -- critical section. Before the first mutating step, a rollback
#   trap is installed and the prior state is recorded: whether the agent was
#   loaded, whether the app was running, whether a previous app bundle
#   exists, and a byte-exact copy of the old LaunchAgent plist. Then the
#   agent is booted out, the staged bundle is swapped in (the previous app
#   is moved to $(dirname DEST)/.Vox.previous.app first), the new plist is
#   written and the agent is bootstrapped and, if it was running before,
#   kickstarted. On any failure in phase B the trap rolls back, step by
#   step and best-effort: it boots out whatever is registered, restores the
#   previous app bundle and the old plist bytes, re-bootstraps the old agent
#   if it was loaded and kickstarts it if it was running. Each rollback step
#   is guarded so that one failing step does not stop the others. Failures
#   are reported with the word "rollback". The script still exits with the
#   ORIGINAL failure status, never with the rollback's status. On success the
#   previous-app backup is discarded.
#
#   The staging, swap and rollback helpers live in scripts/lib/swap-app.sh.
#
# Usage:
#   ./scripts/install-macos.sh [path/to/built/Vox.app]
# Defaults to the debug bundle `tauri build --debug` just produced.
#
# Environment:
#   VOX_INSTALL_DEST_APP  Override the destination bundle path (used by the
#                         test harness, scripts/test-install-macos.sh). The
#                         default is /Applications/Vox.app.
#   VOX_UPDATE_NONINTERACTIVE=1
#                         Unattended mode (set by Vox for its own updates): never
#                         creates a signing identity, never falls back to ad-hoc
#                         signing, fails fast before any change to the live app
#                         when the stable identity is missing.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

APP_NAME="Vox"
APP_NAME_LOWER="vox"
BUNDLE_ID="com.iamshreeram.vox"
AGENT_LABEL="${BUNDLE_ID}.agent"
DEST_APP="${VOX_INSTALL_DEST_APP:-/Applications/${APP_NAME}.app}"
STAGING="$(dirname "$DEST_APP")/.${APP_NAME}.staging.app"
BACKUP="$(dirname "$DEST_APP")/.${APP_NAME}.previous.app"
SRC_APP="${1:-src-tauri/target/debug/bundle/macos/${APP_NAME}.app}"
AGENT_PLIST="$HOME/Library/LaunchAgents/${AGENT_LABEL}.plist"
UID_NUM="$(id -u)"

if [ ! -d "$SRC_APP" ]; then
  echo "Built app not found at $SRC_APP -- run 'bun run tauri build --debug' first." >&2
  exit 1
fi

# Guard against a real, previously-confirmed footgun: if $SRC_APP is itself
# an already-wrapped install (e.g. someone points this script at an existing
# /Applications/Vox.app instead of a genuine fresh `tauri build` output --
# this has actually happened, e.g. while testing this very script) then its
# Contents/MacOS/${APP_NAME_LOWER} is just the thin launcher shell script,
# not the real multi-hundred-megabyte compiled binary. Blindly `mv`-ing it
# onto the destination's "-bin" path below would silently overwrite and
# destroy the real binary there, leaving two tiny scripts that call each
# other via launchd forever and nothing that actually runs the app --
# confirmed to happen silently, with no error anywhere, until the app
# mysteriously stops launching. Detect a shebang ("#!") at the start of the
# source binary -- a real Mach-O binary never starts with those two bytes
# -- and refuse to proceed if found, BEFORE touching the destination at all.
SRC_BIN="${SRC_APP}/Contents/MacOS/${APP_NAME_LOWER}"
if [ "$(head -c 2 "$SRC_BIN" 2>/dev/null)" = "#!" ]; then
  echo "ERROR: ${SRC_BIN} is a shell script, not a real compiled binary." >&2
  echo "This means \$SRC_APP is already a wrapped/installed app, not a fresh build -- running this" >&2
  echo "script against it would destroy the real binary. Run 'bun run tauri build --debug' to produce a" >&2
  echo "genuine fresh build, then re-run this script (with no argument, or pointing at that fresh build)." >&2
  exit 1
fi

# ---- Shared state for the EXIT trap ---------------------------------------
# PHASE: A (staging, no live effect), B (critical section), done (success).
PHASE="A"
MUTATED=0          # set just before the first launchctl/app mutation
SWAP_STARTED=0     # set just before swap_in_app
PLIST_CHANGED=0    # set just before the new plist is written
WAS_LOADED=0       # agent was loaded before this install
WAS_RUNNING=false  # app was running before this install
HAD_PREVIOUS=0     # an app bundle existed at DEST before this install
HAD_PLIST=0        # an old LaunchAgent plist existed before this install
OLD_PLIST_COPY=""  # temp file holding the byte-exact old plist

# rollback_install: best-effort restore of the previous state. Runs only from
# the EXIT trap after a failure in phase B. Every step is guarded (set -e is
# still active inside the trap) so one failure does not stop the others.
rollback_install() {
  echo "Install failed -- rolling back to the previous state..." >&2
  if [ "$MUTATED" = 1 ]; then
    launchctl bootout "gui/${UID_NUM}/${AGENT_LABEL}" >/dev/null 2>&1 || true
    if [ "$SWAP_STARTED" = 1 ]; then
      rollback_app "$DEST_APP" "$BACKUP" "$HAD_PREVIOUS" \
        || echo "rollback: could not restore the previous app bundle at $DEST_APP" >&2
    fi
    if [ "$PLIST_CHANGED" = 1 ]; then
      if [ "$HAD_PLIST" = 1 ]; then
        cp "$OLD_PLIST_COPY" "$AGENT_PLIST" \
          || echo "rollback: could not restore the previous LaunchAgent plist" >&2
      else
        rm -f "$AGENT_PLIST" \
          || echo "rollback: could not remove the new LaunchAgent plist" >&2
      fi
    fi
    if [ "$WAS_LOADED" = 1 ] && [ -f "$AGENT_PLIST" ]; then
      launchctl bootstrap "gui/${UID_NUM}" "$AGENT_PLIST" >/dev/null 2>&1 \
        || echo "rollback: could not re-bootstrap the previous agent" >&2
    fi
    if [ "$WAS_RUNNING" = true ]; then
      launchctl kickstart -k "gui/${UID_NUM}/${AGENT_LABEL}" >/dev/null 2>&1 \
        || echo "rollback: could not restart the previous app" >&2
    fi
  fi
  rm -rf "$STAGING" 2>/dev/null \
    || echo "rollback: could not remove the staging copy at $STAGING" >&2
}

# on_exit: runs on every exit. The original status is captured first and is
# always the status this script exits with.
on_exit() {
  local status=$?
  trap - EXIT
  if [ "$status" -ne 0 ]; then
    if [ "$PHASE" = "B" ]; then
      rollback_install
    else
      rm -rf "$STAGING" 2>/dev/null || true
    fi
  fi
  if [ -n "$OLD_PLIST_COPY" ]; then
    rm -f "$OLD_PLIST_COPY" 2>/dev/null || true
  fi
  exit "$status"
}
trap on_exit EXIT

. "$SCRIPT_DIR/lib/swap-app.sh"

echo "== Installing ${APP_NAME} =="

# ---- Phase A: stage the build (no effect on the live install) ------------
echo "Staging ${SRC_APP} -> ${STAGING}..."
stage_app "$SRC_APP" "$STAGING"

# ---- 2. Rename the real binary, install the launchd-kickstart wrapper ---
# Done inside the STAGED bundle. The plist written in phase B points at the
# bundle's final location in DEST_APP.
STAGED_MACOS_DIR="${STAGING}/Contents/MacOS"
STAGED_REAL_BIN="${STAGED_MACOS_DIR}/${APP_NAME_LOWER}-bin"
STAGED_WRAPPER="${STAGED_MACOS_DIR}/${APP_NAME_LOWER}"
DEST_REAL_BIN="${DEST_APP}/Contents/MacOS/${APP_NAME_LOWER}-bin"

echo "Installing launchd-kickstart wrapper (see header comment for why)..."
mv "$STAGED_WRAPPER" "$STAGED_REAL_BIN"
cat > "$STAGED_WRAPPER" <<WRAPPER_EOF
#!/usr/bin/env bash
# Thin launcher, NOT the real ${APP_NAME} binary (that's ./${APP_NAME_LOWER}-bin).
# Delegates to a launchd user agent so the real process never launches
# through LaunchServices -- see scripts/install-macos.sh's header comment
# for the full explanation.
AGENT_LABEL="${AGENT_LABEL}"
UID_NUM="\$(id -u)"
if ! launchctl kickstart -k "gui/\${UID_NUM}/\${AGENT_LABEL}" 2>/tmp/${APP_NAME_LOWER}_launchd_err.log; then
    ERR="\$(cat /tmp/${APP_NAME_LOWER}_launchd_err.log 2>/dev/null)"
    osascript -e "display notification \"\${ERR}\" with title \"${APP_NAME} failed to start (LaunchAgent missing?)\"" 2>/dev/null || true
    exit 1
fi
WRAPPER_EOF
chmod +x "$STAGED_WRAPPER"

# ---- 4. Re-sign the staged bundle ----------------------------------------
# Uses the stable local "Vox Local Dev Codesign" identity (self-signed,
# auto-created on first run by ./scripts/setup-codesign-identity.sh if it
# doesn't already exist -- see that script's header for the full why)
# instead of plain ad-hoc (`-s -`).
#
# This matters a lot more than it sounds: ad-hoc signing has NO stable
# identity across builds -- it's just a hash of the binary's own content.
# Every rebuild produces a different signature, and TCC tracks
# Accessibility/Microphone grants per-signature for ad-hoc apps. The
# practical effect (confirmed repeatedly): every single rebuild silently
# invalidated every permission you'd already granted, and worse, left
# behind a stale duplicate TCC row each time, which made "waiting
# forever" on a fresh grant look like a bug rather than a side effect of
# the previous re-sign. A real (even self-signed) certificate has its own
# persistent identity, so the same cert across rebuilds keeps TCC's
# grants valid. Auto-creates that identity on first run via
# setup-codesign-identity.sh (one-time trust-settings prompt); only falls
# back to ad-hoc if that creation itself fails for some reason (e.g. no
# openssl, or the user declines the trust prompt), so the script still
# works end to end either way.
#
# Deliberately NOT using --options runtime (hardened runtime): combined
# with only ad-hoc/self-signed (non-notarized) signing, hardened runtime
# was observed to silently break the app's XPC calls into tccd entirely
# (TCCAccessRequest() fired client-side on every poll tick but tccd never
# logged receiving it) -- permission dialogs never had a chance to show
# at all. Not needed for a local, non-sandboxed dev build regardless.
echo "Re-signing..."
# Unattended runs (VOX_UPDATE_NONINTERACTIVE=1, launched by Vox) never create an
# identity, never open a dialog and never fall back to ad-hoc signing: an ad-hoc
# signature resets the Accessibility/Microphone grants. They fail here, in phase
# A, so the staging copy is removed and the live app is not touched.
if [ "${VOX_UPDATE_NONINTERACTIVE:-0}" = "1" ] && ! security find-identity -v -p codesigning 2>/dev/null | grep -q "Vox Local Dev Codesign"; then
  echo "ERROR: the stable 'Vox Local Dev Codesign' identity is not available, and an unattended" >&2
  echo "update will not create it or sign ad-hoc. Nothing was changed. Run" >&2
  echo "./scripts/setup-codesign-identity.sh once by hand, then retry the update." >&2
  exit 1
fi
if ! security find-identity -v -p codesigning 2>/dev/null | grep -q "Vox Local Dev Codesign"; then
  echo "  No stable 'Vox Local Dev Codesign' identity found yet -- creating one now (one-time setup)"
  echo "  so Accessibility/Microphone grants survive future rebuilds. See"
  echo "  scripts/setup-codesign-identity.sh for exactly what this does."
  if ! "$SCRIPT_DIR/setup-codesign-identity.sh"; then
    echo "  Could not create a stable identity automatically -- falling back to" >&2
    echo "  ad-hoc; permissions will need re-granting after every future rebuild." >&2
    echo "  Run ./scripts/setup-codesign-identity.sh by hand to retry." >&2
  fi
fi
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Vox Local Dev Codesign"; then
  SIGN_IDENTITY="Vox Local Dev Codesign"
else
  SIGN_IDENTITY="-"
fi
if [ "${VOX_UPDATE_NONINTERACTIVE:-0}" = "1" ] && [ "$SIGN_IDENTITY" = "-" ]; then
  echo "ERROR: no stable signing identity; refusing ad-hoc signing in an unattended update. Nothing was changed." >&2
  exit 1
fi
codesign --force --deep -s "$SIGN_IDENTITY" "$STAGING"

# ---- Phase B: critical section (rollback trap is armed) ------------------
PHASE="B"

# Record the prior state so a failure can restore it exactly.
if pgrep -f "${APP_NAME}.app/Contents/MacOS/${APP_NAME_LOWER}-bin" >/dev/null 2>&1; then
  WAS_RUNNING=true
fi
if launchctl print "gui/${UID_NUM}/${AGENT_LABEL}" >/dev/null 2>&1; then
  WAS_LOADED=1
fi
if [ -e "$DEST_APP" ]; then
  HAD_PREVIOUS=1
fi
if [ -f "$AGENT_PLIST" ]; then
  HAD_PLIST=1
  OLD_PLIST_COPY="$(mktemp "${TMPDIR:-/tmp}/${APP_NAME_LOWER}-agent-plist.XXXXXX")"
  cp "$AGENT_PLIST" "$OLD_PLIST_COPY"
fi

# ---- 1. Stop anything currently running ----------------------------------
# Remember whether it was actually running (WAS_RUNNING above) so the
# restart step can bring it back -- re-running this script should always
# leave you running the build you just installed.
echo "Stopping any running instance..."
MUTATED=1
launchctl bootout "gui/${UID_NUM}/${AGENT_LABEL}" 2>/dev/null || true
pkill -f "${APP_NAME}.app/Contents/MacOS/" 2>/dev/null || true

# ---- 1b. Swap the staged bundle into place -------------------------------
# Clear any stale backup from an earlier crashed run here, not inside the
# swap, so that a failure while clearing it cannot be mistaken for a
# swap that needs rolling back.
echo "Replacing ${DEST_APP}..."
rm -rf "$BACKUP"
SWAP_STARTED=1
swap_in_app "$STAGING" "$DEST_APP" "$BACKUP"

mdimport "$DEST_APP" >/dev/null 2>&1 || true

# ---- 3. launchd user agent -----------------------------------------------
echo "Registering launchd user agent..."
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"

# launchd agents start with a near-empty environment (just PATH) -- they do
# NOT inherit proxy settings exported by your shell's profile, even though
# an interactive terminal session usually has them. On a network that needs
# a proxy for outbound HTTPS (e.g. a corporate network), this silently
# broke the wake-word model download: it worked fine from a shell, timed
# out after 60s when attempted by the actual running app. Fix: forward
# whatever proxy variables happen to be set in THIS install script's own
# environment into the agent's plist, generically -- this script has no
# opinion on what network you're on, it just passes through what's already
# configured (or nothing, if you're not behind a proxy). A proxy that was
# trusted blindly was confirmed to actively break the app rather than help
# it, so verify HTTPS connectivity through it before forwarding any proxy
# settings; a misconfigured or unreachable proxy should not be baked in.
PROXY_ENV_XML=""
HTTPS_PROXY_URL="${HTTPS_PROXY:-${https_proxy:-}}"
if [ -n "$HTTPS_PROXY_URL" ]; then
  if curl --proxy "$HTTPS_PROXY_URL" --silent --head --fail --max-time 5 https://huggingface.co >/dev/null 2>&1; then
    for var in HTTP_PROXY HTTPS_PROXY NO_PROXY http_proxy https_proxy no_proxy; do
      value="${!var:-}"
      if [ -n "$value" ]; then
        PROXY_ENV_XML="${PROXY_ENV_XML}        <key>${var}</key>
        <string>${value}</string>
"
      fi
    done
  else
    echo "Note: HTTPS_PROXY is set but a live connectivity check through it failed -- not forwarding it to the app (it will connect directly instead)."
  fi
fi

PLIST_CHANGED=1
cat > "$AGENT_PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${DEST_REAL_BIN}</string>
    </array>
    <key>RunAtLoad</key>
    <false/>
    <key>KeepAlive</key>
    <false/>
    <key>StandardOutPath</key>
    <string>$HOME/Library/Logs/${APP_NAME_LOWER}-launchd.out.log</string>
    <key>StandardErrorPath</key>
    <string>$HOME/Library/Logs/${APP_NAME_LOWER}-launchd.err.log</string>
    <key>ProcessType</key>
    <string>Interactive</string>
$([ -n "$PROXY_ENV_XML" ] && printf '    <key>EnvironmentVariables</key>\n    <dict>\n%s    </dict>\n' "$PROXY_ENV_XML")</dict>
</plist>
PLIST_EOF
launchctl bootstrap "gui/${UID_NUM}" "$AGENT_PLIST"
echo "  [ok] $AGENT_PLIST"

# ---- 5. Restart, if it was running before this script stopped it --------
# Restart through launchd (kickstart), never through `open`: LaunchServices
# launches are exactly what the header comment explains must be avoided.
if [ "$WAS_RUNNING" = true ]; then
  echo "Restarting (it was running before this install)..."
  launchctl kickstart -k "gui/${UID_NUM}/${AGENT_LABEL}"
fi

# ---- Success: the new install is live ------------------------------------
discard_backup "$BACKUP" || echo "Warning: could not remove the previous-app backup at $BACKUP" >&2
PHASE="done"

cat <<EOF

== Install complete ==

Launch any of these ways -- all now work, including Spotlight/Dock:
  - Spotlight: Cmd+Space, type "${APP_NAME}", hit Enter
  - Dock / Finder double-click
  - Terminal:  open "${DEST_APP}"

Re-run this script any time after a fresh 'bun run tauri build --debug'
to pick up new code -- it's fully idempotent. If it was already running,
it's been restarted automatically; otherwise it's left stopped, same as
you left it.
EOF
