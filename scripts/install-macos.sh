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
# Usage:
#   ./scripts/install-macos.sh [path/to/built/Vox.app]
# Defaults to the debug bundle `tauri build --debug` just produced.
set -euo pipefail
cd "$(dirname "$0")/.."

APP_NAME="Vox"
APP_NAME_LOWER="vox"
BUNDLE_ID="com.iamshreeram.vox"
AGENT_LABEL="${BUNDLE_ID}.agent"
DEST_APP="/Applications/${APP_NAME}.app"
SRC_APP="${1:-src-tauri/target/debug/bundle/macos/${APP_NAME}.app}"
AGENT_PLIST="$HOME/Library/LaunchAgents/${AGENT_LABEL}.plist"
UID_NUM="$(id -u)"

if [ ! -d "$SRC_APP" ]; then
  echo "Built app not found at $SRC_APP -- run 'bun run tauri build --debug' first." >&2
  exit 1
fi

echo "== Installing ${APP_NAME} =="

# ---- 1. Stop anything currently running, replace the bundle -------------
echo "Stopping any running instance..."
launchctl bootout "gui/${UID_NUM}/${AGENT_LABEL}" 2>/dev/null || true
pkill -f "${APP_NAME}.app/Contents/MacOS/" 2>/dev/null || true

echo "Copying ${SRC_APP} -> ${DEST_APP}..."
rm -rf "$DEST_APP"
cp -R "$SRC_APP" "$DEST_APP"

# ---- 2. Rename the real binary, install the launchd-kickstart wrapper ---
MACOS_DIR="${DEST_APP}/Contents/MacOS"
REAL_BIN="${MACOS_DIR}/${APP_NAME_LOWER}-bin"
WRAPPER="${MACOS_DIR}/${APP_NAME_LOWER}"

echo "Installing launchd-kickstart wrapper (see header comment for why)..."
mv "$WRAPPER" "$REAL_BIN"
cat > "$WRAPPER" <<WRAPPER_EOF
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
chmod +x "$WRAPPER"

# ---- 3. launchd user agent -----------------------------------------------
echo "Registering launchd user agent..."
mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
cat > "$AGENT_PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>${AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>${REAL_BIN}</string>
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
</dict>
</plist>
PLIST_EOF
launchctl bootstrap "gui/${UID_NUM}" "$AGENT_PLIST"
echo "  [ok] $AGENT_PLIST"

# ---- 4. Re-sign after structural changes ---------------------------------
# Uses the stable local "Vox Local Dev Codesign" identity (self-signed,
# created once via Keychain Access + `security import` + `security
# add-trusted-cert -p codeSign`) instead of plain ad-hoc (`-s -`).
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
# grants valid. Falls back to ad-hoc if the cert isn't present on this
# machine (e.g. a fresh clone) so the script still works, just with the
# same per-rebuild permission churn as before.
#
# Deliberately NOT using --options runtime (hardened runtime): combined
# with only ad-hoc/self-signed (non-notarized) signing, hardened runtime
# was observed to silently break the app's XPC calls into tccd entirely
# (TCCAccessRequest() fired client-side on every poll tick but tccd never
# logged receiving it) -- permission dialogs never had a chance to show
# at all. Not needed for a local, non-sandboxed dev build regardless.
echo "Re-signing..."
if security find-identity -v -p codesigning 2>/dev/null | grep -q "Vox Local Dev Codesign"; then
  SIGN_IDENTITY="Vox Local Dev Codesign"
else
  echo "  (no 'Vox Local Dev Codesign' identity found -- falling back to ad-hoc;"
  echo "   permissions will need re-granting after every future rebuild)"
  SIGN_IDENTITY="-"
fi
codesign --force --deep -s "$SIGN_IDENTITY" "$DEST_APP"

mdimport "$DEST_APP" >/dev/null 2>&1 || true

cat <<EOF

== Install complete ==

Launch any of these ways -- all now work, including Spotlight/Dock:
  - Spotlight: Cmd+Space, type "${APP_NAME}", hit Enter
  - Dock / Finder double-click
  - Terminal:  open "${DEST_APP}"

Re-run this script any time after a fresh 'bun run tauri build --debug'
to pick up new code -- it's fully idempotent.
EOF
