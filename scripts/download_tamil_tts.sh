#!/usr/bin/env bash
# Download the Tamil neural TTS pack for Vox.
# If your network uses a proxy that blocks large downloads, download the files in a browser
# from https://huggingface.co/prasadvittaldev/pocket-tts-tamil-onnx-int4 and place them in
# the folder below (the file names must match).
#
# Pack:    prasadvittaldev/pocket-tts-tamil-onnx-int4  (CC-BY-4.0, ~111 MB, 9 files)
# Where:   "$HOME/Library/Application Support/com.iamshreeram.vox/tts_models/pocket-tts-tamil-int4"
# Safety:  every file is checked against its published size and SHA-256 before it is
#          kept; a bad or partial file is deleted and the script exits non-zero.
#          Re-running is safe: files that already verify are skipped.
#          Nothing here changes any Vox setting.
set -euo pipefail

REPO="prasadvittaldev/pocket-tts-tamil-onnx-int4"
BASE="https://huggingface.co/$REPO/resolve/main"
DEST="${VOX_TAMIL_TTS_DIR:-$HOME/Library/Application Support/com.iamshreeram.vox/tts_models/pocket-tts-tamil-int4}"

# name|size in bytes|sha256 (bundle.json is small and not an LFS file, so it has no hash)
FILES=(
  "bos_before_voice.npy|4224|70394ef2b594f2a2a17b73b10854be18983b3423bb1ed15dd0e140363ce533e4"
  "bundle.json|24361|"
  "flow_lm_flow_int8.onnx|9962530|3fa26714fb5691600f35e946bd5b90e52238cc4b06d0468e2d84bc965b7ce15c"
  "flow_lm_main_int4.onnx|40802614|f2a82004c75b1312f4a5ae9da03040d246dfe6db1057860f422818fd6a4cc119"
  "mimi_decoder_int8.onnx|22684077|ff381f4d83486e29f802f79771271c2758068d96937b3eebfc258d566ac2d658"
  "mimi_encoder_int8.onnx|20802362|1014f91ac923cffc88a837c2bdb45c755615c5698e3779c85299ea6e621168c0"
  "sample_geetha.wav|276524|16478c6a28e9befca26e3cf288f2dc85a03064ae811c2d8e72b5cb846a3a5848"
  "text_conditioner.onnx|16388344|8248f6e5028586dba66975899b49c956065292cc5963db634222e6c618020f52"
  "tokenizer.model|345594|c71caecb74631c4660fd639882afa39ee2ea6689f6cd94d5fdcfc46bb4af8dcf"
)

sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
size_of()   { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

mkdir -p "$DEST"
echo "Destination: $DEST"
failed=0
for row in "${FILES[@]}"; do
  IFS='|' read -r name size sha <<<"$row"
  target="$DEST/$name"
  if [ -f "$target" ] && [ "$(size_of "$target")" = "$size" ] && { [ -z "$sha" ] || [ "$(sha256_of "$target")" = "$sha" ]; }; then
    echo "ok (already verified)  $name"
    continue
  fi
  echo "downloading            $name  ($((size / 1024)) KB)"
  # -C - resumes a partial download; -f fails on HTTP errors instead of saving an error page.
  if ! curl -fL --retry 3 --retry-delay 2 -C - -o "$target.part" "$BASE/$name"; then
    echo "FAILED to download $name (a proxy that asks for sign-in, HTTP 407, can block this; see the note at the top)" >&2
    rm -f "$target.part"; failed=1; continue
  fi
  if [ "$(size_of "$target.part")" != "$size" ]; then
    echo "FAILED size check for $name: expected $size, got $(size_of "$target.part")" >&2
    rm -f "$target.part"; failed=1; continue
  fi
  if [ -n "$sha" ] && [ "$(sha256_of "$target.part")" != "$sha" ]; then
    echo "FAILED SHA-256 check for $name" >&2
    rm -f "$target.part"; failed=1; continue
  fi
  mv "$target.part" "$target"
  echo "verified               $name"
done

if [ "$failed" -ne 0 ]; then
  echo "One or more files failed. Fix the network and run this script again." >&2
  exit 1
fi
echo
echo "All files verified in: $DEST"
ls -la "$DEST"
