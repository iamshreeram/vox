import assert from "node:assert/strict";
import { versionLabel } from "./footerLabels";

// UI enterprise S4a (U8): the footer must never show a bare "v" while the version is unknown.

const scenarios: Record<string, () => void> = {
  "an unknown version renders no label at all (no bare v)": () => {
    assert.equal(versionLabel(""), null);
    assert.equal(versionLabel("   "), null);
    assert.equal(versionLabel(null), null);
    assert.equal(versionLabel(undefined), null);
  },
  "a known version renders v plus the version": () => {
    assert.equal(versionLabel("0.1.2"), "v0.1.2");
    assert.equal(versionLabel(" 1.4.0 "), "v1.4.0");
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) throw new Error(`${failed} footer label scenarios failed`);
console.log(`footer labels: ${Object.keys(scenarios).length} scenarios passed`);
