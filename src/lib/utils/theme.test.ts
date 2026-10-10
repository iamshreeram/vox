import assert from "node:assert/strict";
import { relativeLuminance, neutralBucketFor } from "./theme";

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

console.log("theme luminance: all assertions passed");
