#!/usr/bin/env bash
# Ensures a stable, locally-trusted code-signing identity named
# "Vox Local Dev Codesign" exists in the login keychain, creating one if
# it's missing. install-macos.sh uses this identity instead of ad-hoc
# signing so macOS TCC (Accessibility/Microphone) permission grants
# survive rebuilds.
#
# Why this exists: an ad-hoc signature (`codesign -s -`) is just a hash of
# the binary's own content, so it changes on every single rebuild. macOS
# ties Accessibility/Microphone grants to the exact signature, so every
# rebuild silently invalidated any permission the user had already
# granted -- and left a stale duplicate row behind in TCC's database each
# time, which makes "granting access does nothing" look like a bug rather
# than a side effect. Before this script existed, install-macos.sh only
# used a *stable* identity if one already happened to exist in the
# developer's own keychain (created once, by hand, via Keychain Access) --
# anyone else building this project from a fresh clone silently fell back
# to ad-hoc and hit the exact same permission-reset-every-rebuild problem.
# This script makes the stable-identity path available on ANY machine,
# automatically.
#
# Safe to run any time -- idempotent, does nothing if the identity already
# exists. One-time cost on a fresh machine: macOS will show exactly ONE
# "trust settings" authorization prompt (Touch ID / login password) when
# the certificate is marked as trusted for code signing -- that's an
# unavoidable macOS security gate for changing trust settings, not a bug
# in this script. Everything before that prompt (key/cert generation,
# keychain import) is fully non-interactive.
#
# Usage:
#   ./scripts/setup-codesign-identity.sh
set -euo pipefail

IDENTITY_NAME="Vox Local Dev Codesign"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -v -p codesigning 2>/dev/null | grep -q "$IDENTITY_NAME"; then
  echo "== '$IDENTITY_NAME' already exists -- nothing to do =="
  exit 0
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl not found -- cannot create a self-signed identity automatically." >&2
  exit 1
fi

echo "== Creating '$IDENTITY_NAME' (one-time setup) =="

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

CONF="$WORKDIR/codesign.cnf"
cat > "$CONF" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $IDENTITY_NAME
[ext]
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
basicConstraints = critical, CA:false
EOF

openssl req -x509 -newkey rsa:2048 -keyout "$WORKDIR/key.pem" \
  -out "$WORKDIR/cert.pem" -days 3650 -nodes -config "$CONF" -extensions ext \
  >/dev/null 2>&1

# PKCS#12 export needs a password even for a throwaway local dev identity;
# the file is deleted (via the trap above) the moment this script exits.
openssl pkcs12 -export -out "$WORKDIR/identity.p12" \
  -inkey "$WORKDIR/key.pem" -in "$WORKDIR/cert.pem" -passout pass:vox-local-dev \
  >/dev/null 2>&1

echo "Importing into the login keychain..."
security import "$WORKDIR/identity.p12" -k "$KEYCHAIN" -P vox-local-dev \
  -T /usr/bin/codesign -T /usr/bin/security

echo "Trusting the certificate for code signing -- macOS will prompt ONCE now:"
security add-trusted-cert -d -r trustAsRoot -p codeSign -k "$KEYCHAIN" "$WORKDIR/cert.pem"

if security find-identity -v -p codesigning 2>/dev/null | grep -q "$IDENTITY_NAME"; then
  echo "== '$IDENTITY_NAME' created and trusted successfully =="
else
  echo "Identity creation finished but isn't showing up in 'security find-identity'." >&2
  echo "Check Keychain Access (login keychain) manually." >&2
  exit 1
fi
