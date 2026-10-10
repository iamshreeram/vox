import assert from "node:assert/strict";
import { shouldFallBackToGeneral } from "./sectionGate";

const sections = {
  general: { enabled: () => true },
  debug: { enabled: (settings: any) => settings?.debug_mode ?? false },
};

assert.equal(
  shouldFallBackToGeneral("debug", { debug_mode: false }, sections),
  true,
);
assert.equal(
  shouldFallBackToGeneral("debug", { debug_mode: true }, sections),
  false,
);
assert.equal(shouldFallBackToGeneral("general", {}, sections), false);
assert.equal(shouldFallBackToGeneral("debug", null, sections), false);
assert.equal(shouldFallBackToGeneral("debug", undefined, sections), false);

console.log("section gate: all assertions passed");
