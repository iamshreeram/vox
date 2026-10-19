#!/bin/bash
# Generates the 20 extra speaker clips in /tmp/vox-voice-assets/audio2/ with macOS `say`
# (fully local, nothing uploaded). Output: 16 kHz mono LEI16 WAV via afconvert.
# NOTE: `say` output is not guaranteed bit-reproducible across macOS builds; the
# sha256 of every produced WAV is recorded in fixtures/manifest.json (external_inputs).
set -euo pipefail
OUT=/tmp/vox-voice-assets/audio2
TMP=$(mktemp -d)
mkdir -p "$OUT"

make_clip() {
  local voice="$1" name="$2" text="$3"
  say -v "$voice" -o "$TMP/$name.aiff" "$text"
  afconvert -f WAVE -d LEI16@16000 -c 1 "$TMP/$name.aiff" "$OUT/$name.wav"
}

# Samantha (en_US) -> s1a..s1e
make_clip Samantha s1a "The quick brown fox jumps over the lazy dog near the river bank."
make_clip Samantha s1b "Please remember to bring an umbrella because the forecast calls for rain later."
make_clip Samantha s1c "The museum opens at nine in the morning and closes just before the evening."
make_clip Samantha s1d "Several engineers gathered in the conference room to review the quarterly schedule."
make_clip Samantha s1e "Fresh vegetables are sold at the market every Saturday from early in the morning."

# Daniel (en_GB) -> s2a..s2e
make_clip Daniel s2a "A steady wind moved across the empty field while the clouds slowly drifted east."
make_clip Daniel s2b "The train to the northern station leaves every twenty minutes during weekdays."
make_clip Daniel s2c "Water boils at one hundred degrees Celsius when the pressure is at sea level."
make_clip Daniel s2d "Our team finished the report early and sent it to the client before lunch."
make_clip Daniel s2e "The library keeps a large collection of maps and old travel guides."

# Karen (en_AU) -> s3a..s3e
make_clip Karen s3a "Turn left at the second traffic light and continue past the large green park."
make_clip Karen s3b "The recipe calls for two cups of flour, one egg, and a little warm milk."
make_clip Karen s3c "Birds gather near the lake each morning to feed on seeds scattered along the shore."
make_clip Karen s3d "The committee will announce the results of the survey at the end of next week."
make_clip Karen s3e "Please place the boxes on the wooden shelf beside the window in the hallway."

# Fred (en_US) -> s4a..s4e
make_clip Fred s4a "Mountain paths can be slippery after rain, so wear shoes with good grip."
make_clip Fred s4b "The bakery on the corner sells fresh bread, muffins, and cinnamon rolls."
make_clip Fred s4c "A small boat drifted slowly across the harbor as the sun rose over the hills."
make_clip Fred s4d "Students should review chapter four before the exam on Thursday afternoon."
make_clip Fred s4e "The garden needs about an inch of water each week during the dry season."

rm -rf "$TMP"
echo "wrote $(ls "$OUT"/*.wav | wc -l) clips to $OUT"
