// UI enterprise R2a (R2-12): the orb is a role=button and its keyboard focus ring must be visible on
// any backdrop. The ring is two-tone: a light 2px outline and a dark 2px band (box-shadow), each at
// >= 3:1 against the other tone. Node fs only.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const css = readFileSync(
  fileURLToPath(new URL("./orb.css", import.meta.url)),
  "utf8",
);
const tsx = readFileSync(
  fileURLToPath(new URL("./OrbApp.tsx", import.meta.url)),
  "utf8",
);

const focusRule = css.match(/\.orb-root:focus-visible\s*\{([^}]*)\}/)?.[1];
assert.ok(focusRule, "orb.css declares a .orb-root:focus-visible rule");
assert.match(focusRule, /outline:\s*2px solid #ffffff/, "light outline");
assert.match(
  focusRule,
  /outline-offset:\s*2px/,
  "outline sits outside the dark band",
);
assert.match(focusRule, /box-shadow:\s*0 0 0 4px #000000/, "dark band");
assert.doesNotMatch(
  focusRule,
  /outline-offset:\s*-/,
  "the ring must not be drawn inside the orb",
);

// Contrast of each tone against the other (WCAG relative luminance).
const lum = (hex: string): number => {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};
const ratio = (a: string, b: string): number => {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};
assert.ok(
  ratio("#ffffff", "#000000") >= 3,
  "light and dark tones differ by 3:1",
);

// The orb is still the focusable role=button (the ring applies to it).
assert.match(
  tsx,
  /role="button"[\s\S]*tabIndex=\{0\}/,
  "orb stays a focusable button",
);
console.log("orbFocus ok");
