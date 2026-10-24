#!/usr/bin/env bash
# Functional tests for scripts/setup-voice-models.sh using local file:// fixtures
# (the real downloads are blocked by the proxy on this machine).
set -u
SCRIPT="$(cd "$(dirname "$0")" && pwd)/setup-voice-models.sh"
W="$(mktemp -d -t vox-setup-test.XXXXXX)"; trap 'rm -rf "$W"' EXIT
mkdir -p "$W/fx"
# shellcheck disable=SC1090
source "$SCRIPT"
PROTO='=file'

# fixtures: four small fake "phonon" files with real checksums
for n in model.q8.gguf default-voice.safetensors tokenizer.json config.json; do
  head -c 3000 /dev/urandom > "$W/fx/$n"
done
spec_good() {
  for n in model.q8.gguf default-voice.safetensors tokenizer.json config.json; do
    echo "$n $(file_size "$W/fx/$n") $(file_sha "$W/fx/$n") file://$W/fx/$n"
  done
}
model_specs() { [ "$1" = phonon ] && spec_good; }

pass=0; fail=0
check() { if eval "$2"; then echo "PASS  $1"; pass=$((pass+1)); else echo "FAIL  $1"; fail=$((fail+1)); fi; }
run() { ( main --data-dir "$W/data" --context "$W/ctx.md" --only phonon "$@" ) > "$W/out.txt" 2>&1; echo $?; }
D="$W/data/phonon_models/phonon-31m-en"

rc=$(run)
check "T1 fresh install exits 0" "[ $rc = 0 ]"
check "T1 four files + READY in the right folder" "[ -f $D/READY ] && [ \$(ls $D | wc -l | tr -d ' ') = 5 ]"
check "T1 READY is an empty regular file" "[ -f $D/READY ] && [ ! -s $D/READY ]"
check "T1 staging folder gone" "[ ! -e $W/data/phonon_models/.staging-setup-phonon ]"
check "T1 log says READY written last" "grep -q 'READY marker written last' $W/ctx.md"

rc=$(run)
check "T2 re-run exits 0 and downloads nothing" "[ $rc = 0 ] && grep -q 'already installed' $W/out.txt && ! grep -q 'download ' $W/out.txt"
check "T2 log has two run sections" "[ \$(grep -c '^## Run' $W/ctx.md) = 2 ]"

echo tamper >> "$D/config.json"
rc=$(run)
check "T3 tampered file is replaced" "[ $rc = 0 ] && verifies $D/config.json \$(file_size $W/fx/config.json) \$(file_sha $W/fx/config.json)"
check "T3 no leftover previous/staging" "[ -z \"\$(ls -a $W/data/phonon_models | grep -E 'previous|staging')\" ]"

rm -f "$D/READY"
rc=$(run)
check "T4 missing READY is added without downloading" "[ $rc = 0 ] && [ -f $D/READY ] && ! grep -q 'download ' $W/out.txt"

# T5: a source that disappears must leave the existing install untouched
echo tamper >> "$D/tokenizer.json"          # forces a re-install attempt
mv "$W/fx/model.q8.gguf" "$W/fx/model.q8.gguf.gone"
rc=$(run)
check "T5 failed download exits 1" "[ $rc = 1 ]"
check "T5 existing (tampered) install not deleted by a failed attempt" "[ -f $D/model.q8.gguf ] && [ -f $D/READY ]"
check "T5 failure reported with staging path" "grep -q 'staging folder' $W/out.txt"
check "T5 log records FAILED" "grep -q 'Result: FAILED' $W/ctx.md"
mv "$W/fx/model.q8.gguf.gone" "$W/fx/model.q8.gguf"
rc=$(run)
check "T5b resumes from staging and then succeeds" "[ $rc = 0 ] && verifies $D/tokenizer.json \$(file_size $W/fx/tokenizer.json) \$(file_sha $W/fx/tokenizer.json)"

# T6: right size, wrong checksum must never be installed
model_specs() { [ "$1" = phonon ] && spec_good | sed "1s/ [0-9a-f]\{64\} / $(printf '0%.0s' $(seq 64)) /"; }
rm -rf "$W/data2"
( main --data-dir "$W/data2" --context "$W/ctx2.md" --only phonon ) > "$W/out2.txt" 2>&1; rc=$?
check "T6 checksum mismatch exits 1" "[ $rc = 1 ]"
check "T6 nothing installed, no .part kept" "[ ! -e $W/data2/phonon_models/phonon-31m-en ] && [ -z \"\$(ls $W/data2/phonon_models/.staging-setup-phonon | grep part)\" ]"
check "T6 reason mentions checksum" "grep -q 'did not match' $W/ctx2.md"

# T7: log hygiene. Run with a fake proxy address and credentials in the environment
# and make sure none of it, nor any signed-link marker, reaches the log.
rm -rf "$W/data4"
( https_proxy='http://fakeuser:fakesecret@proxy.example.invalid:3128' \
  main --data-dir "$W/data4" --context "$W/ctx4.md" --only phonon ) > "$W/out4.txt" 2>&1
check "T7 proxy address and credentials never reach the log" "! grep -i -E 'fakesecret|fakeuser|proxy.example.invalid' $W/ctx4.md $W/ctx.md $W/ctx2.md"
check "T7 log says a proxy was in use without naming it" "grep -q 'shell proxy settings in use (address not logged)' $W/ctx4.md"
check "T7 no signed-link markers in any log" "! grep -E 'X-Amz|Signature=|token=' $W/ctx.md $W/ctx2.md $W/ctx4.md"

# T8: dry run changes nothing and writes no log
rm -rf "$W/data3"
( main --data-dir "$W/data3" --context "$W/ctx3.md" --only phonon --dry-run ) > "$W/out3.txt" 2>&1
check "T8 dry run creates nothing" "[ ! -e $W/data3 ] && [ ! -e $W/ctx3.md ]"
echo; echo "passed=$pass failed=$fail"
[ $fail = 0 ]
