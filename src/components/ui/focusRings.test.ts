import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise R3-3, R3-16/17, R3-5/6: focus rings and overlay target sizes, pinned statically.

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string =>
  readFileSync(path.join(here, rel), "utf8");

const theme = read("../../styles/theme.css");
const slider = read("./Slider.tsx");
const select = read("./Select.tsx");
const toggle = read("./ToggleSwitch.tsx");
const filler = read("../settings/FillerWordRemoval.tsx");
const accent = read("../settings/appearance/AccentPicker.tsx");
const overlayCss = read("../../overlay/RecordingOverlay.css");

// R4-b: every component source file (tests excluded), walked recursively.
function componentSources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return componentSources(full);
    const isSource = /\.tsx?$/.test(entry.name);
    return isSource && !/\.test\.tsx?$/.test(entry.name) ? [full] : [];
  });
}

// A focus-state variant whose ring or outline uses an accent or logo colour. Empty allow-list:
// focus indicators use the global --focus-ring (or an on-accent inner ring), never a brand colour.
const ACCENT_FOCUS_ALLOW_LIST: readonly string[] = [];
const accentFocusClass =
  /(^|[\s"'`:])(peer-|group-)?focus(-visible|-within)?:[^\s"'`]*?(ring|outline|inset-ring)-(logo|accent)/;

const scenarios: Record<string, () => void> = {
  "no focus ring or outline in src/components uses an accent or logo colour (R4-b)":
    () => {
      const offenders: string[] = [];
      for (const file of componentSources(path.join(here, ".."))) {
        readFileSync(file, "utf8")
          .split("\n")
          .forEach((line, index) => {
            const match = accentFocusClass.exec(line);
            if (match && !ACCENT_FOCUS_ALLOW_LIST.includes(match[0].trim())) {
              offenders.push(`${file}:${index + 1}: ${line.trim()}`);
            }
          });
      }
      assert.deepEqual(offenders, []);
    },

  "range sliders draw one focus ring on their container, using the global --focus-ring (R3-3)":
    () => {
      assert.match(slider, /className="range-focus-ring flex items-center/);
      assert.match(
        theme,
        /\.range-focus-ring:has\(input\[type="range"\]:focus-visible\)\s*\{[^}]*outline:\s*2px solid var\(--focus-ring\)[^}]*outline-offset:\s*2px/,
      );
      // The input's own outline is removed only inside the container that draws the ring, so the
      // other range inputs (audio player, chat opacity, orb) keep the global focus ring.
      assert.match(
        theme,
        /\.range-focus-ring input\[type="range"\]:focus-visible\s*\{\s*outline:\s*none/,
      );
      assert.doesNotMatch(theme, /^input\[type="range"\]:focus-visible/m);
    },

  "ToggleSwitch: the visible track shows a 4px --focus-ring when its hidden checkbox has keyboard focus (R4-a)":
    () => {
      assert.match(toggle, /className="sr-only peer"/);
      assert.match(
        toggle,
        /peer-focus-visible:ring-4 peer-focus-visible:ring-\(--focus-ring\)/,
      );
      assert.doesNotMatch(toggle, /ring-logo-primary/);
    },

  // UI enterprise R5a: the switch track is 24px tall at the 15px root (h-6 would be 22.5px).
  "ToggleSwitch and the orb toggle track have a min-h-[24px] 24px track (R5a)":
    () => {
      const orb = read("../settings/orb/FloatingOrbSettings.tsx");
      assert.match(
        toggle,
        /className="relative w-\[44px\] h-\[24px\] min-h-\[24px\]/,
      );
      assert.match(orb, /h-\[24px\] min-h-\[24px\] w-\[44px\] rounded-full/);
    },

  "FillerWordRemoval: each native radio label draws the ring on the label when its radio has focus (R4-a)":
    () => {
      assert.match(filler, /className="radio-focus-label flex min-h-\[24px\]/);
      assert.match(
        theme,
        /\.radio-focus-label:has\(input\[type="radio"\]:focus-visible\)\s*\{[^}]*outline:\s*2px solid var\(--focus-ring\)[^}]*outline-offset:\s*2px/,
      );
      assert.match(
        theme,
        /\.radio-focus-label input\[type="radio"\]:focus-visible\s*\{\s*outline:\s*none/,
      );
    },

  "accent swatches: the focus ring is the global --focus-ring plus an on-accent inner ring, not the accent colour (R4-1)":
    () => {
      assert.doesNotMatch(accent, /outline-accent-a/);
      assert.match(accent, /focus-visible:outline-\(--focus-ring\)/);
      assert.match(accent, /focus-visible:inset-ring-\(--color-on-accent\)/);
    },

  "react-select control shows a 2px --focus-ring when focused or with the menu open (R3-16/17)":
    () => {
      assert.match(
        select,
        /boxShadow:\s*state\.isFocused\s*\?\s*"0 0 0 2px var\(--focus-ring\)"\s*:\s*"none"/,
      );
    },

  "overlay cancel buttons (.sx) are at least 24x24 px and their grid columns hold them (R3-5/6)":
    () => {
      assert.match(overlayCss, /\.sx \{\s*width: 24px;\s*height: 24px;/);
      assert.match(
        overlayCss,
        /grid-template-columns: minmax\(24px, 1fr\) auto minmax\(24px, 1fr\);/,
      );
    },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL - ${name}`);
    console.error(error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log("focusRings ok");
