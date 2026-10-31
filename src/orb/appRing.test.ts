// The app-mode ring (DESIGN Revision 5, A7). Proves, with a JS replica of color-mix and the repo's
// contrast helper, that the ring derived from --color-accent-a has >= 3:1 against every
// representative surface of its bucket, for the six presets and a spread of Custom accents, and
// that it keeps the accent's hue. Pure: no DOM.
import assert from "node:assert/strict";
import { appRingColour, APP_RING_MIX_PERCENT, mixHex } from "./globePalette";
import { ACCENTS, ACCENT_IDS, type AccentBucket } from "../styles/accents";
import { contrastRatio } from "../styles/wcag";

/** Representative surfaces of each bucket (the light and dark page and chat backgrounds). */
const SURFACES: Record<AccentBucket, readonly string[]> = {
  light: ["#FFFFFF", "#FBFBFB", "#F7F8FA"],
  dark: ["#0B0B0F", "#111111", "#1B1B22"],
};

/** Custom accents a user can pick: the spread the proof requires (A7). */
const CUSTOM_ACCENTS: readonly string[] = [
  "#FF0000",
  "#00FF00",
  "#0000FF",
  "#FFFF00",
  "#00FFFF",
  "#FF00FF",
  "#FFFFFF",
  "#FAFAFA",
  "#000000",
  "#080808",
  "#808080",
  "#FFD1DC",
  "#7C3AED",
];

/** Accents whose hue must survive the mix (saturated, unambiguous hues). */
const HUE_CHECKED: readonly string[] = ["#7C3AED", "#FF0000", "#0000FF"];

const MIN_RATIO = 3;
const HUE_TOLERANCE_DEG = 20;

/** Hue in degrees [0, 360) of a #RRGGBB colour. */
function hueOf(hex: string): number {
  const [r, g, b] = [1, 3, 5].map(
    (i) => parseInt(hex.slice(i, i + 2), 16) / 255,
  );
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  if (delta === 0) return 0;
  let h: number;
  if (max === r) h = ((g - b) / delta) % 6;
  else if (max === g) h = (b - r) / delta + 2;
  else h = (r - g) / delta + 4;
  return (h * 60 + 360) % 360;
}

/** The smallest circular distance between two hues, in degrees. */
const hueDistance = (a: number, b: number): number => {
  const d = Math.abs(a - b) % 360;
  return Math.min(d, 360 - d);
};

const scenarios: Record<string, () => void> = {
  "the CSS is a color-mix of the app accent into black (light) or white (dark), at the set percentage":
    () => {
      assert.equal(
        appRingColour("light"),
        `color-mix(in srgb, var(--color-accent-a) ${APP_RING_MIX_PERCENT}%, black)`,
      );
      assert.equal(
        appRingColour("dark"),
        `color-mix(in srgb, var(--color-accent-a) ${APP_RING_MIX_PERCENT}%, white)`,
      );
      assert.ok(!appRingColour("light").includes("color-accent-text"));
      assert.ok(!appRingColour("dark").includes("color-accent-text"));
    },
  "the replica mixes exactly as color-mix does (black and white ends)": () => {
    assert.equal(mixHex("#FFFFFF", "light", 55), "#8c8c8c");
    assert.equal(mixHex("#000000", "dark", 55), "#737373");
    assert.equal(mixHex("#FF0000", "light", 55), "#8c0000");
  },
  "six preset accents: the ring is >= 3:1 on every surface of its bucket":
    () => {
      for (const bucket of ["light", "dark"] as const) {
        for (const id of ACCENT_IDS) {
          const accent = ACCENTS[id][bucket].fillA;
          const ring = mixHex(accent, bucket);
          for (const surface of SURFACES[bucket]) {
            const ratio = contrastRatio(ring, surface);
            assert.ok(
              ratio >= MIN_RATIO,
              `${id} ${bucket} ring ${ring} on ${surface}: ${ratio.toFixed(2)}:1`,
            );
          }
        }
      }
    },
  "Custom accents (the proof spread): the ring is >= 3:1 on every surface of both buckets":
    () => {
      for (const bucket of ["light", "dark"] as const) {
        for (const accent of CUSTOM_ACCENTS) {
          const ring = mixHex(accent, bucket);
          for (const surface of SURFACES[bucket]) {
            const ratio = contrastRatio(ring, surface);
            assert.ok(
              ratio >= MIN_RATIO,
              `${accent} ${bucket} ring ${ring} on ${surface}: ${ratio.toFixed(2)}:1`,
            );
          }
        }
      }
    },
  "saturated accents keep their hue within 20 degrees on both buckets": () => {
    for (const bucket of ["light", "dark"] as const) {
      for (const accent of HUE_CHECKED) {
        const ring = mixHex(accent, bucket);
        const distance = hueDistance(hueOf(ring), hueOf(accent));
        assert.ok(
          distance <= HUE_TOLERANCE_DEG,
          `${accent} ${bucket}: ring ${ring} hue off by ${distance.toFixed(1)} deg`,
        );
      }
    }
  },
  "the Custom violet ring is not the placeholder teal (#5eead4)": () => {
    const ring = mixHex("#7C3AED", "light");
    assert.ok(hueDistance(hueOf(ring), hueOf("#7C3AED")) <= HUE_TOLERANCE_DEG);
    assert.ok(hueDistance(hueOf(ring), hueOf("#5eead4")) > 90);
  },
};

for (const [name, run] of Object.entries(scenarios)) {
  run();
  console.log(`ok - ${name}`);
}
console.log(
  `appRing.test.ts: ${Object.keys(scenarios).length} scenarios passed`,
);
