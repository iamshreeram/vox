#!/usr/bin/env bash
# One script that fetches the optional voice models Vox can use, puts each one in
# the folder Vox reads it from, verifies it, and logs what happened to CONTEXT.md.
#
# Usage:
#   ./scripts/setup-voice-models.sh [options]
#
# Options:
#   --only LIST      comma-separated subset of: phonon,kokoro,smart-turn,speaker
#                    (default: all four)
#   --data-dir DIR   Vox data folder (default: ~/Library/Application Support/com.iamshreeram.vox,
#                    or the VOX_DATA_DIR environment variable)
#   --context FILE   where the log goes (default: CONTEXT.md in the repository root,
#                    or the VOX_CONTEXT_FILE environment variable)
#   --direct         ignore HTTP(S) proxy settings in your shell and connect straight
#                    to the model hosts. Use it only if your network policy allows it.
#                    Without it the script uses your shell's proxy settings like curl does.
#   --dry-run        show what would happen and change nothing (no log is written)
#   -h, --help       show this help
#
# Where each model goes (under the Vox data folder):
#   phonon      phonon_models/phonon-31m-en/       (4 files + READY marker)
#   kokoro      tts_models/kokoro-v1.0/            (2 files + READY marker)
#   smart-turn  aux_models/smart_turn/             (1 file)
#   speaker     aux_models/speaker_resnet34/       (1 file)
#
# Safety:
#   - Every file is checked against its pinned size and SHA-256 before it is used.
#   - Downloads go to a hidden staging folder next to the destination. An existing
#     install is only replaced after the whole new set has verified.
#   - Safe to re-run: a model that already verifies is skipped, an interrupted
#     download resumes, and nothing is deleted except a staged file that is wrong.
#   - HTTPS only. Checksums and sources match src-tauri/src/managers/natural_model.rs
#     and aux_model.rs; see docs/model-provenance.md.
#   - The log never records proxy addresses, credentials or signed download links.
#
# The wake-word model is not handled here: Vox downloads it itself and its files
# are not pinned by checksum.
#
# After a successful run, quit Vox from the tray and open it again so it rescans.
set -uo pipefail

SCRIPT_PATH="${BASH_SOURCE[0]}"
REPO_ROOT="$(cd "$(dirname "$SCRIPT_PATH")/.." && pwd)"
DATA_DIR="${VOX_DATA_DIR:-$HOME/Library/Application Support/com.iamshreeram.vox}"
CONTEXT_FILE="${VOX_CONTEXT_FILE:-$REPO_ROOT/CONTEXT.md}"
PROTO='=https'
ALL_MODELS="phonon kokoro smart-turn speaker"

PHONON_BASE="https://huggingface.co/gradium/phonon-31m-en/resolve/541f5752bc3c79583068bb205bfda5eb39aaa08a"
KOKORO_BASE="https://github.com/mzdk100/kokoro/releases/download/V1.0"

DIRECT=0
DRY_RUN=0
ONLY=""
RUN_LOG=""
TMP_ERR=""
FAILED=0
FETCH_URL=""
LAST_REASON=""

# ---------------------------------------------------------------- model table

model_title() {
  case "$1" in
    phonon) echo "Phonon text-to-speech (English)" ;;
    kokoro) echo "Natural voice (Kokoro)" ;;
    smart-turn) echo "Smart turn detection" ;;
    speaker) echo "Speaker filter (voice match)" ;;
  esac
}

# folder relative to the Vox data folder
model_dest() {
  case "$1" in
    phonon) echo "phonon_models/phonon-31m-en" ;;
    kokoro) echo "tts_models/kokoro-v1.0" ;;
    smart-turn) echo "aux_models/smart_turn" ;;
    speaker) echo "aux_models/speaker_resnet34" ;;
  esac
}

# 1 when Vox expects an empty READY marker file, written last
model_ready() {
  case "$1" in
    phonon | kokoro) echo 1 ;;
    *) echo 0 ;;
  esac
}

# one line per file: name size sha256 url[|fallback-url...]
model_specs() {
  case "$1" in
    phonon)
      cat <<EOF
model.q8.gguf 64512352 a7d9aad2cd6dc66473f24a08bc6d2939030ef4d751ef82349361fbef3272dbae $PHONON_BASE/model.q8.gguf
default-voice.safetensors 256136 f739e8c4c76ee7f92e7c94a82002748c8cd22e99582ac49f7b9b94cd63786e18 $PHONON_BASE/default-voice.safetensors
tokenizer.json 277967 691a00068fa7b989f2f3e5ae5daa2d1a5bb319f2d5360deafcfb19a8bb6e7936 $PHONON_BASE/tokenizer.json
config.json 4634 b5d0f88353638370d94035b7d73d7d6577c0554d78f8d7ecfd40490aae39ae85 $PHONON_BASE/config.json
EOF
      ;;
    kokoro)
      cat <<EOF
kokoro-v1.0.int8.onnx 92361271 6e742170d309016e5891a994e1ce1559c702a2ccd0075e67ef7157974f6406cb $KOKORO_BASE/kokoro-v1.0.int8.onnx
voices.bin 28311781 988549ccf1d68a80a31de1b9d00ac59d5dceb69743cbc607d1de17c7c4cdccf4 $KOKORO_BASE/voices.bin
EOF
      ;;
    smart-turn)
      cat <<EOF
smart-turn-v3.2-cpu.onnx 8679182 2bb026316b14a660486a75b1733cd3fbab8c2fd0314dc9af7be49f8cca967e4f https://raw.githubusercontent.com/pipecat-ai/pipecat/main/src/pipecat/audio/turn/smart_turn/data/smart-turn-v3.2-cpu.onnx|https://huggingface.co/pipecat-ai/smart-turn-v3/resolve/main/smart-turn-v3.2-cpu.onnx
EOF
      ;;
    speaker)
      cat <<EOF
wespeaker_en_voxceleb_resnet34.onnx 26534365 5ef208a9da1453335308a6b6f4e6dfbd7e183a38b604de0a57664f45d257fe94 https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet34.onnx
EOF
      ;;
  esac
}

# --------------------------------------------------------------------- helpers

file_size() { wc -c <"$1" | tr -d ' '; }
file_sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# true when $1 is a regular file of size $2 with SHA-256 $3
verifies() {
  [ -f "$1" ] && [ ! -L "$1" ] && [ "$(file_size "$1")" = "$2" ] && [ "$(file_sha "$1")" = "$3" ]
}

# print a path with the home folder shortened to ~
tilde() { printf '%s' "$1" | sed "s|^$HOME|~|"; }

# drop URLs from an error text so no download link ever reaches the log
scrub() { tr '\n' ' ' | sed -E 's#https?://[^ ]*#<url>#g; s/ +$//' | cut -c1-160; }

log() { [ -n "$RUN_LOG" ] && printf '%s\n' "$1" >>"$RUN_LOG"; return 0; }

proxy_mode() {
  if [ "$DIRECT" = "1" ]; then
    echo "direct (proxy settings ignored)"
  elif [ -n "${https_proxy:-}${HTTPS_PROXY:-}${all_proxy:-}${ALL_PROXY:-}" ]; then
    echo "shell proxy settings in use (address not logged)"
  else
    echo "no proxy configured"
  fi
}

# Downloads one file into $1 (the staging folder) and verifies it.
# args: stage name size sha urls(|-separated). Sets FETCH_URL / LAST_REASON.
fetch_file() {
  local stage=$1 name=$2 size=$3 sha=$4 urls=$5
  local target="$stage/$name" part="$stage/$name.part" url old_ifs have
  FETCH_URL=""
  LAST_REASON=""

  if verifies "$target" "$size" "$sha"; then
    FETCH_URL="(reused earlier download)"
    return 0
  fi
  rm -f "$target"
  if verifies "$part" "$size" "$sha"; then
    mv "$part" "$target" && FETCH_URL="(completed from earlier download)" && return 0
  fi

  local opts=(--fail --location --silent --show-error
    --proto "$PROTO" --proto-redir "$PROTO"
    --connect-timeout 20 --retry 3 --retry-delay 2)
  [ "$DIRECT" = "1" ] && opts+=(--noproxy '*')

  old_ifs=$IFS
  IFS='|'
  # shellcheck disable=SC2086
  set -- $urls
  IFS=$old_ifs
  for url in "$@"; do
    if curl "${opts[@]}" -C - -o "$part" "$url" 2>"$TMP_ERR"; then
      if verifies "$part" "$size" "$sha"; then
        mv "$part" "$target" || { LAST_REASON="could not move the file into place"; return 1; }
        FETCH_URL="$url"
        return 0
      fi
      LAST_REASON="downloaded file did not match the pinned size or checksum"
      rm -f "$part"
    else
      LAST_REASON="$(scrub <"$TMP_ERR")"
      if [ -f "$part" ]; then
        # keep a shorter partial so the next run can resume it; drop one that can never resume
        have="$(file_size "$part")"
        if [ "$have" = "0" ] || [ "$have" -ge "$size" ]; then rm -f "$part"; fi
      fi
    fi
  done
  [ -n "$LAST_REASON" ] || LAST_REASON="no source available"
  return 1
}

# true when the install folder is complete and verified (adds a missing READY marker)
installed_ok() {
  local dest=$1 ready=$2 specs=$3 name size sha urls
  [ -d "$dest" ] && [ ! -L "$dest" ] || return 1
  while read -r name size sha urls; do
    [ -z "$name" ] && continue
    verifies "$dest/$name" "$size" "$sha" || return 1
  done <<EOF
$specs
EOF
  if [ "$ready" = "1" ] && [ ! -f "$dest/READY" ]; then
    : >"$dest/READY" || return 1
  fi
  return 0
}

# --------------------------------------------------------------- install one

install_model() {
  local key=$1 title dest ready specs parent stage prev rows ok name size sha urls state
  title="$(model_title "$key")"
  dest="$DATA_DIR/$(model_dest "$key")"
  ready="$(model_ready "$key")"
  specs="$(model_specs "$key")"
  parent="$(dirname "$dest")"
  stage="$parent/.staging-setup-$key"
  prev="$parent/.previous-setup-$key"

  echo
  echo "== $title"
  echo "   folder: $(tilde "$dest")"

  log ""
  log "### $title"
  log ""
  log "- Folder: \`$(tilde "$dest")\`"

  if installed_ok "$dest" "$ready" "$specs"; then
    echo "   ok: already installed and verified"
    log "- Result: already installed, every file verified (nothing downloaded)"
    return 0
  fi

  mkdir -p "$stage" || { echo "   FAILED: cannot create $stage" >&2; log "- Result: FAILED (cannot create the staging folder)"; return 1; }

  rows=""
  ok=1
  while read -r name size sha urls; do
    [ -z "$name" ] && continue
    echo "   download $name ($size bytes)"
    if fetch_file "$stage" "$name" "$size" "$sha" "$urls"; then
      echo "   verified $name"
      rows="${rows}| $name | $size | $sha | ${FETCH_URL} | ok |"$'\n'
    else
      echo "   FAILED $name: $LAST_REASON" >&2
      rows="${rows}| $name | $size | $sha | - | FAILED: $LAST_REASON |"$'\n'
      ok=0
    fi
  done <<EOF
$specs
EOF

  log ""
  log "| File | Size (bytes) | SHA-256 | Source | Status |"
  log "| --- | ---: | --- | --- | --- |"
  [ -n "$rows" ] && printf '%s' "$rows" >>"$RUN_LOG"
  log ""

  if [ "$ok" != "1" ]; then
    echo "   staging folder (kept for resume): $(tilde "$stage")" >&2
    log "- Staging folder (kept so the next run resumes): \`$(tilde "$stage")\`"
    log "- Result: FAILED. Verified files were kept in the staging folder, and the next run resumes from them. Any previous install is untouched."
    return 1
  fi

  # publish: the old install is set aside, not deleted, until the new one is in place
  rm -rf "$prev"
  if [ -e "$dest" ] || [ -L "$dest" ]; then
    mv "$dest" "$prev" || { log "- Result: FAILED (could not set the previous install aside)"; return 1; }
  fi
  if ! mv "$stage" "$dest"; then
    [ -e "$prev" ] && mv "$prev" "$dest"
    log "- Result: FAILED (could not move the verified files into place; previous install restored)"
    return 1
  fi
  if [ "$ready" = "1" ]; then
    : >"$dest/READY" || { log "- Result: FAILED (could not write the READY marker)"; return 1; }
  fi
  rm -rf "$prev"
  state="installed"
  [ "$ready" = "1" ] && state="installed, READY marker written last"
  echo "   done: $state"
  log "- Result: $state"
  return 0
}

# ------------------------------------------------------------------- the log

context_header() {
  cat <<'EOF'
# CONTEXT.md

Log of voice-model setup runs. `scripts/setup-voice-models.sh` appends one section
per run, newest at the bottom.

**What is logged:** the date and time (UTC), the repository commit, the Vox data folder
(home folder shown as `~`), the network mode, and for every model the destination
folder, each file's name, pinned size, SHA-256, source address and result.

**What is never logged:** proxy addresses, credentials, signed download links, or any
file contents.

**How to read it:** a model with `Result: installed` or `already installed` has every
file verified against its pinned size and checksum. After a run, quit Vox from the tray
and open it again so it rescans. Sources and licences: `docs/model-provenance.md`.
EOF
}

write_context() {
  local commit
  commit="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo n/a)"
  if [ ! -f "$CONTEXT_FILE" ]; then
    context_header >"$CONTEXT_FILE" || { echo "Cannot write $CONTEXT_FILE" >&2; return 1; }
  fi
  {
    echo
    echo "## Run $(date -u '+%Y-%m-%d %H:%M:%S') UTC"
    echo
    echo "- Script: \`scripts/setup-voice-models.sh\` at commit \`$commit\`"
    echo "- Vox data folder: \`$(tilde "$DATA_DIR")\`"
    echo "- Network mode: $(proxy_mode)"
    echo "- Models requested: $ONLY"
    echo "- Overall: $([ "$FAILED" = "0" ] && echo "all requested models ready" || echo "$FAILED model(s) failed, see below")"
    cat "$RUN_LOG"
  } >>"$CONTEXT_FILE" || { echo "Cannot write $CONTEXT_FILE" >&2; return 1; }
  echo
  echo "Log written to $(tilde "$CONTEXT_FILE")"
}

# ----------------------------------------------------------------------- main

usage() { sed -n '2,/^set -uo/p' "$SCRIPT_PATH" | sed '$d' | sed 's/^# \{0,1\}//'; }

main() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --direct) DIRECT=1 ;;
      --dry-run) DRY_RUN=1 ;;
      --only)
        [ $# -ge 2 ] || { echo "--only needs a value" >&2; exit 2; }
        ONLY="$(printf '%s' "$2" | tr ',' ' ')"
        shift
        ;;
      --data-dir)
        [ $# -ge 2 ] || { echo "--data-dir needs a value" >&2; exit 2; }
        DATA_DIR="$2"
        shift
        ;;
      --context)
        [ $# -ge 2 ] || { echo "--context needs a value" >&2; exit 2; }
        CONTEXT_FILE="$2"
        shift
        ;;
      -h | --help) usage; exit 0 ;;
      *) echo "Unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
  done

  ONLY="${ONLY:-$ALL_MODELS}"
  local key valid
  for key in $ONLY; do
    valid=0
    for m in $ALL_MODELS; do [ "$key" = "$m" ] && valid=1; done
    if [ "$valid" = "0" ]; then
      echo "Unknown model: $key (choose from: $ALL_MODELS)" >&2
      exit 2
    fi
  done
  command -v curl >/dev/null || { echo "curl is required" >&2; exit 2; }
  command -v shasum >/dev/null || { echo "shasum is required" >&2; exit 2; }

  echo "Vox data folder: $(tilde "$DATA_DIR")"
  echo "Network mode:    $(proxy_mode)"
  echo "Models:          $ONLY"
  [ -d "$DATA_DIR" ] || echo "Note: that folder does not exist yet; it will be created. Open Vox once if you are unsure of its location."

  if [ "$DRY_RUN" = "1" ]; then
    echo
    echo "Dry run, nothing will be changed:"
    for key in $ONLY; do
      local dest ready specs
      dest="$DATA_DIR/$(model_dest "$key")"
      ready="$(model_ready "$key")"
      specs="$(model_specs "$key")"
      if installed_ok_readonly "$dest" "$ready" "$specs"; then
        echo "  $key -> $(tilde "$dest")   already installed and verified"
      else
        echo "  $key -> $(tilde "$dest")   would download:"
        echo "$specs" | while read -r name size sha urls; do
          [ -n "$name" ] && echo "       $name ($size bytes)"
        done
      fi
    done
    exit 0
  fi

  RUN_LOG="$(mktemp -t vox-setup-log.XXXXXX)" || exit 2
  TMP_ERR="$(mktemp -t vox-setup-err.XXXXXX)" || exit 2
  trap 'rm -f "$RUN_LOG" "$TMP_ERR"' EXIT

  mkdir -p "$DATA_DIR" || { echo "Cannot create $DATA_DIR" >&2; exit 2; }

  for key in $ONLY; do
    install_model "$key" || FAILED=$((FAILED + 1))
  done

  write_context || FAILED=$((FAILED + 1))

  echo
  if [ "$FAILED" = "0" ]; then
    echo "Done. Quit Vox from the tray and open it again so it finds the models."
    echo "Then in Settings: pick the voice engine (Phonon or Natural), and turn on Smart turn."
    exit 0
  fi
  echo "$FAILED model(s) failed. Verified files are kept; re-run to resume." >&2
  echo "If the failure says 407 (proxy asked for authentication), try --direct if your" >&2
  echo "network policy allows it, or download the file in a browser under its exact" >&2
  echo "name, put it in the staging folder printed above, and re-run to verify it." >&2
  exit 1
}

# read-only twin of installed_ok for --dry-run (never writes a READY marker)
installed_ok_readonly() {
  local dest=$1 ready=$2 specs=$3 name size sha urls
  [ -d "$dest" ] && [ ! -L "$dest" ] || return 1
  while read -r name size sha urls; do
    [ -z "$name" ] && continue
    verifies "$dest/$name" "$size" "$sha" || return 1
  done <<EOF
$specs
EOF
  [ "$ready" = "0" ] || [ -f "$dest/READY" ]
}

# run main only when executed, not when sourced (tests source this file)
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
