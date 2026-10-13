import assert from "node:assert/strict";
import {
  relativeLuminance,
  neutralBucketFor,
  onAccentFor,
  derivedAccentProperties,
  DEFAULT_CUSTOM_THEME_COLORS,
} from "./theme";

// Same hand-computed test matrix as settings.rs's relative_luminance tests --
// this MUST stay in sync with that Rust implementation (see theme.ts's doc
// comment on relativeLuminance for why it's duplicated here at all).

const closeTo = (actual: number | null, expected: number, epsilon: number) => {
  assert.notEqual(actual, null, `expected a number, got null`);
  assert.ok(
    Math.abs((actual as number) - expected) < epsilon,
    `expected ${actual} to be within ${epsilon} of ${expected}`,
  );
};

closeTo(relativeLuminance("#000000"), 0.0, 1e-9);
closeTo(relativeLuminance("#ffffff"), 1.0, 1e-6);
closeTo(relativeLuminance("#808080"), 0.216, 1e-3);
closeTo(relativeLuminance("#ff0000"), 0.2126, 1e-3);
closeTo(relativeLuminance("#00ff00"), 0.7152, 1e-3);
closeTo(relativeLuminance("#bbbbbb"), 0.4966, 1e-3);
closeTo(relativeLuminance("#bcbcbc"), 0.5028, 1e-3);
assert.equal(relativeLuminance("not-hex"), null);
assert.equal(relativeLuminance("#fff"), null, "3-digit shorthand rejected");
assert.equal(relativeLuminance(""), null, "empty string rejected");

assert.equal(neutralBucketFor("#000000"), "dark");
assert.equal(neutralBucketFor("#ffffff"), "light");
assert.equal(neutralBucketFor("#808080"), "dark");
// The exact pair that straddles the 0.5 threshold either side.
assert.equal(neutralBucketFor("#bbbbbb"), "dark");
assert.equal(neutralBucketFor("#bcbcbc"), "light");
// Invalid hex defensively defaults to the dark bucket.
assert.equal(neutralBucketFor("not-hex"), "dark");

// ---- accent gradient tokens for the custom theme ----
const contrast = (a: string, b: string): number => {
  const [hi, lo] = [relativeLuminance(a)!, relativeLuminance(b)!].sort(
    (x, y) => y - x,
  );
  return (hi + 0.05) / (lo + 0.05);
};

// Dark accent ends need light text, light ends need dark text.
assert.equal(onAccentFor("#0f766e", "#7c3aed"), "#ffffff");
assert.equal(onAccentFor("#fbbf24", "#fb923c"), "#111111");
// The default (Aurora-like) custom palette: bright teal + lavender -> dark text.
assert.equal(
  onAccentFor(
    DEFAULT_CUSTOM_THEME_COLORS.accent,
    DEFAULT_CUSTOM_THEME_COLORS.accent_secondary,
  ),
  "#111111",
);
// Mixed ends: the worst-case end decides, so the text stays readable on both.
for (const [a, b] of [
  ["#ffffff", "#000000"],
  ["#2dd4bf", "#1e1b4b"],
  ["#facc15", "#7c3aed"],
]) {
  const on = onAccentFor(a, b);
  const worst = Math.min(contrast(on, a), contrast(on, b));
  const other = on === "#ffffff" ? "#111111" : "#ffffff";
  assert.ok(
    worst >= Math.min(contrast(other, a), contrast(other, b)),
    `${a}/${b}: ${on} is not the better worst-case choice`,
  );
}
// Unparseable input never throws and falls back to dark text.
assert.equal(onAccentFor("nope", "#fff"), "#111111");

const props = derivedAccentProperties({
  ...DEFAULT_CUSTOM_THEME_COLORS,
  accent: "#0f766e",
  accent_secondary: "#7c3aed",
});
assert.deepEqual(props, {
  "--color-accent-a": "#0f766e",
  "--color-accent-b": "#7c3aed",
  "--color-on-accent": "#ffffff",
  "--color-background-ui": "#0f766e",
});

console.log("theme luminance and accent tokens: all assertions passed");
