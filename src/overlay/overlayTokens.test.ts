import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { blendOver, contrastRatio } from "../styles/wcag";

// UI enterprise S4a (item 2): the recording overlay's neutrals map to the shared palette tokens
// (theme.css) instead of their own hex literals. No behaviour or size change.

const here = path.dirname(fileURLToPath(import.meta.url));
const css = readFileSync(path.join(here, "RecordingOverlay.css"), "utf8");

const NEUTRALS = ["muted", "faint", "border", "hair"];
const HEX = /#[0-9a-fA-F]{3,8}\b/;

const declaration = (name: string): string => {
  const match = css.match(new RegExp(`${name}:\\s*([^;]+);`));
  assert.ok(match, `RecordingOverlay.css declares ${name}`);
  return match[1];
};

for (const bucket of ["light", "dark"]) {
  for (const neutral of NEUTRALS) {
    const name = `--${bucket}-s-${neutral}`;
    const value = declaration(name);
    assert.ok(!HEX.test(value), `${name} is still a hex literal: ${value}`);
    assert.match(
      value,
      /var\(--(light|dark)-color-|var\(--color-/,
      `${name} must reference a shared palette token, got ${value}`,
    );
  }
}

console.log("overlay tokens: 8 declarations mapped to shared tokens");

// UI enterprise R4-2: the working label ("Transcribing...") is >= 4.5:1 on its chip in the light,
// dark and aurora palettes. The chip is the page background (--s-surface is 98% of it).
const themeCss = readFileSync(path.join(here, "../styles/theme.css"), "utf8");
const paletteHex = (name: string): string => {
  const match = themeCss.match(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`));
  assert.ok(match, `theme.css defines --${name}`);
  return match[1].toLowerCase();
};
assert.match(
  css,
  /\.swork-label \{[^}]*color:\s*color-mix\(in srgb,\s*var\(--color-text\)\s*78%,\s*var\(--color-background\)\)/,
);
for (const name of ["light", "dark", "aurora"]) {
  const text = paletteHex(`${name}-color-text`);
  const background = paletteHex(`${name}-color-background`);
  const label = blendOver(text, 0.78, background);
  const ratio = contrastRatio(label, background);
  assert.ok(
    ratio >= 4.5,
    `${name} work label ${label} on chip ${background}: ${ratio.toFixed(2)}:1 is below 4.5:1`,
  );
}

console.log("overlay work label: 4.5:1 on the chip in light, dark and aurora");
