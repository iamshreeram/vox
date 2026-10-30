import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
