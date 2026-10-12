import assert from "node:assert/strict";
import {
  SECTION_IDS,
  SECTION_ICON_CLASS,
  sidebarItemClass,
} from "./sidebarTheme";

const scenarios: Record<string, () => void> = {
  "section ids match the sidebar sections": () => {
    assert.deepEqual(
      [...SECTION_IDS].sort(),
      [
        "about",
        "advanced",
        "debug",
        "experimental",
        "general",
        "history",
        "models",
        "postprocessing",
      ].sort(),
    );
  },
  "every section has a distinct static colored icon class": () => {
    const seen = new Set<string>();
    for (const id of SECTION_IDS) {
      const cls = SECTION_ICON_CLASS[id];
      assert.ok(cls, `no icon class for ${id}`);
      // A literal text-<hue>-500 so Tailwind can see it at build time.
      assert.match(cls, /^text-[a-z]+-500$/, `${id}: ${cls}`);
      assert.ok(!seen.has(cls), `duplicate icon class ${cls}`);
      seen.add(cls);
    }
  },
  "active item uses the accent gradient with readable on-accent text": () => {
    const active = sidebarItemClass(true);
    for (const token of [
      "bg-linear-to-r",
      "from-accent-a",
      "to-accent-b",
      "text-on-accent",
      "rounded-lg",
    ]) {
      assert.ok(active.includes(token), `active missing ${token}`);
    }
  },
  "inactive item has no gradient but keeps a hover state": () => {
    const inactive = sidebarItemClass(false);
    assert.ok(!inactive.includes("from-accent-a"));
    assert.ok(!inactive.includes("text-on-accent"));
    assert.ok(inactive.includes("hover:"));
    assert.ok(inactive.includes("rounded-lg"));
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
if (failed > 0) throw new Error(`${failed} sidebar theme scenarios failed`);
console.log(`sidebar theme: ${Object.keys(scenarios).length} scenarios passed`);
