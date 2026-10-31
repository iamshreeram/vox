#!/usr/bin/env bash
# Download the Tamil speech-recognition pack for Vox.
# If your network uses a proxy that blocks large downloads (HTTP 407 means the proxy wants a
# sign-in), run this from a network without that proxy, or download the four files in a
# browser from https://huggingface.co/yashwork-byte/indic-conformer-600m-int8-onnx (revision
# 9c590ef62f537c30de47df62e525539938549f13, folder assets/) and place them in the folder below.
# The file names must match.
#
# Model:   AI4Bharat IndicConformer 600M multilingual (MIT); community int8 ONNX repackage
#          (MIT). About 676 MB in total, four files.
# Purpose: Tamil speech recognition in Vox. The pack is selected by choosing Tamil as the
#          speech language; nothing here changes any Vox setting.
# Where:   "$HOME/Library/Application Support/com.iamshreeram.vox/aux_models/indic_conformer_int8"
#          (override with VOX_INDIC_DIR). Files are stored flat, without the assets/ prefix.
# Safety:  every file is checked against its size and SHA-256 before it is kept; a bad or
#          partial file is deleted and the script exits non-zero, naming the file. Files are
#          first downloaded as <name>.part and moved into place only after they verify.
#          Re-running is safe: files that already verify are skipped.
# Options: --check   download nothing; report ok / missing / wrong for each file.
set -euo pipefail

REPO="yashwork-byte/indic-conformer-600m-int8-onnx"
REV="9c590ef62f537c30de47df62e525539938549f13"
BASE="https://huggingface.co/$REPO/resolve/$REV/assets"
DEST="${VOX_INDIC_DIR:-$HOME/Library/Application Support/com.iamshreeram.vox/aux_models/indic_conformer_int8}"

# name|size in bytes|sha256
FILES=(
  "encoder.onnx|652623111|2c6804fa6bc7c8b7326f0aefa78fd26699412032d882a898542f95a51b52dfa0"
  "ctc_decoder.onnx|23095918|9b79abea81daf271e77fcc0e24c9fb76f79e50323d771e1bb6049c972c18f6ea"
  "vocab.json|96743|eda380b3ba4507cb0850b6d0666298babe07eaf3f299b2d08f91db4b491a59bf"
  "language_masks.json|862011|de02a9384ef897b7f97cd416d7f27238e0cb9bf252c56e4a34db562eccf75a63"
)

sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
size_of()   { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

# Prints ok, missing or wrong for one file in $DEST and returns 0 only when it is ok.
file_state() {
  local name="$1" size="$2" sha="$3" target="$DEST/$1"
  if [ ! -f "$target" ]; then
    echo "missing  $name"
    return 1
  fi
  if [ "$(size_of "$target")" != "$size" ] || [ "$(sha256_of "$target")" != "$sha" ]; then
    echo "wrong    $name"
    return 1
  fi
  echo "ok       $name"
  return 0
}

if [ "${1:-}" = "--check" ]; then
  echo "Destination: $DEST"
  check_failed=0
  for row in "${FILES[@]}"; do
    IFS='|' read -r name size sha <<<"$row"
    file_state "$name" "$size" "$sha" || check_failed=1
  done
  if [ "$check_failed" -ne 0 ]; then
    echo "Not all files are present and verified. Run this script without --check to fetch them." >&2
    exit 1
  fi
  echo "All four files verified in: $DEST"
  exit 0
fi

if [ $# -gt 0 ]; then
  echo "Unknown option: $1 (the only option is --check)" >&2
  exit 2
fi

mkdir -p "$DEST"
echo "Destination: $DEST"
for row in "${FILES[@]}"; do
  IFS='|' read -r name size sha <<<"$row"
  target="$DEST/$name"
  if file_state "$name" "$size" "$sha" >/dev/null; then
    echo "ok (already verified)  $name"
    continue
  fi
  echo "downloading            $name  ($((size / 1024)) KB)"
  # -C - resumes a partial download; -f fails on HTTP errors instead of saving an error page.
  if ! curl -fL --retry 3 -C - -o "$target.part" "$BASE/$name"; then
    rm -f "$target.part"
    echo "FAILED to download $name (HTTP 407 means a proxy wants a sign-in; run from a network without it)" >&2
    exit 1
  fi
  if [ "$(size_of "$target.part")" != "$size" ]; then
    echo "FAILED size check for $name: expected $size, got $(size_of "$target.part")" >&2
    rm -f "$target.part"
    exit 1
  fi
  if [ "$(sha256_of "$target.part")" != "$sha" ]; then
    echo "FAILED SHA-256 check for $name" >&2
    rm -f "$target.part"
    exit 1
  fi
  mv "$target.part" "$target"
  echo "verified               $name"
done

echo "Tamil speech pack ready: $DEST"
