import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// UI enterprise R3-3, R3-16/17, R3-5/6: focus rings and overlay target sizes, pinned statically.

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string =>
  readFileSync(path.join(here, rel), "utf8");

const theme = read("../../styles/theme.css");
const slider = read("./Slider.tsx");
const select = read("./Select.tsx");
const overlayCss = read("../../overlay/RecordingOverlay.css");

const scenarios: Record<string, () => void> = {
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
