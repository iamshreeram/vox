import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

// UI enterprise S4b (U13): the shared Dialog traps focus, closes on Escape and restores focus to the
// element that opened it. Pinned on the source because the Dialog is a portal with no DOM test rig.
const source = readFileSync(new URL("./Dialog.tsx", import.meta.url), "utf8");

const scenarios: Record<string, () => void> = {
  "Escape closes a dismissible dialog": () => {
    assert.match(source, /event\.key === "Escape" && dismissible/);
    assert.match(source, /onOpenChange\(false\)/);
  },
  "Tab is trapped inside the dialog (wraps from last to first and back)":
    () => {
      assert.match(source, /getFocusableElements\(/);
      assert.match(source, /event\.key !== "Tab"|event\.key === "Tab"/);
      assert.match(source, /firstElement\.focus\(\)/);
      assert.match(source, /lastElement\.focus\(\)/);
    },
  "focus is restored to the opener when the dialog closes": () => {
    assert.match(source, /previousFocusRef\.current =/);
    assert.match(source, /previousFocusRef\.current\?\.focus\(\)/);
  },
  "the close button has an accessible name": () => {
    assert.match(source, /aria-label=\{closeLabel\}/);
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
console.log("dialogA11y ok");
