// G3b-A orb behaviour scenarios (review round 1). Pure checks for the model and for the gaze
// budget in orb.css. Run with `bun run test:orb:behavior`. Authoritative: do not weaken an
// assertion; if a test is wrong, report it.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { GAZE_MAX_UNITS, isPrimaryPress, keyCommand } from "./orbModel";

const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");

/** The px multiplier of `calc(var(--gaze-x, 0) * Npx)` inside the rule for `selector`. */
function gazeMultiplier(css: string, selector: string): number {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const rule = new RegExp(
    `${escaped}\\s*\\{[^}]*translate:\\s*calc\\(var\\(--gaze-x,\\s*0\\)\\s*\\*\\s*([\\d.]+)px\\)`,
  ).exec(css);
  assert.ok(rule, `no gaze translate rule for ${selector}`);
  return Number(rule[1]);
}

const scenarios: Record<string, () => void> = {
  "held keys do not repeat a command": () => {
    for (const key of ["Enter", " ", "ContextMenu"]) {
      assert.equal(
        keyCommand({ key, shiftKey: false, repeat: true }),
        null,
        `auto-repeat of ${key} must be ignored`,
      );
    }
    assert.equal(
      keyCommand({ key: "Enter", shiftKey: false, repeat: false }),
      "activate",
    );
    assert.equal(
      keyCommand({ key: "F10", shiftKey: true, repeat: true }),
      null,
    );
  },

  "only a primary press without Ctrl starts a gesture": () => {
    assert.equal(isPrimaryPress({ button: 0, ctrlKey: false }), true);
    assert.equal(isPrimaryPress({ button: 0, ctrlKey: true }), false);
    assert.equal(isPrimaryPress({ button: 2, ctrlKey: false }), false);
    assert.equal(isPrimaryPress({ button: 1, ctrlKey: false }), false);
  },

  "body and highlight gaze shifts add up to the cap, the highlight leads":
    () => {
      const css = read("./orb.css");
      // The shine is a child of the body group: its own rule adds to the body shift.
      const body = gazeMultiplier(css, ".orb-root .vox-body-group");
      const shineExtra = gazeMultiplier(css, ".orb-root .vox-shine");
      const shineTotal = body + shineExtra;
      assert.ok(body > 0, "body must move");
      assert.ok(shineExtra > 0, "highlight must move");
      assert.ok(
        shineTotal <= GAZE_MAX_UNITS + 1e-9,
        `shine total ${shineTotal} exceeds ${GAZE_MAX_UNITS} units`,
      );
      assert.ok(
        shineTotal >= GAZE_MAX_UNITS - 1e-9,
        `shine total ${shineTotal} leaves the cap unused`,
      );
      assert.ok(shineTotal >= body, "the highlight must lead the body");
    },
};

let failed = 0;
for (const [name, fn] of Object.entries(scenarios)) {
  try {
    fn();
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
console.log("orbBehavior ok");
