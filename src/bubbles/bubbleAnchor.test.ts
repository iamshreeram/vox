// Issue #113 spec: the bubble stack anchor (top when the stack sits below the orb, bottom
// otherwise). Authoritative: do not weaken an assertion; report a wrong one. Run with
// `bun run test:bubbles` (chained after bubbleModel.test.ts).

import assert from "node:assert/strict";
import {
  anchorOf,
  applySnapshot,
  createState,
  visibleBubbles,
  type BubbleItemWire,
  type Snapshot,
} from "./bubbleModel";

const wire = (id: number, text: string): BubbleItemWire => ({
  id,
  role: "user",
  text,
  age_ms: 0,
  ttl_ms: 4000,
});

const snap = (
  rev: number,
  items: BubbleItemWire[],
  anchor?: string,
): Snapshot =>
  ({
    instance: 7,
    rev,
    items,
    ...(anchor === undefined ? {} : { anchor }),
  }) as Snapshot;

const scenarios: Record<string, () => void> = {
  "anchorOf: top is top, everything else is bottom": () => {
    assert.equal(anchorOf("top"), "top");
    assert.equal(anchorOf("bottom"), "bottom");
    assert.equal(anchorOf(undefined), "bottom", "missing means bottom");
    assert.equal(anchorOf(null), "bottom");
    assert.equal(anchorOf("sideways"), "bottom", "unknown means bottom");
    assert.equal(anchorOf(3), "bottom");
  },

  "a fresh state is anchored at the bottom": () => {
    assert.equal(createState(7).anchor, "bottom");
  },

  "a snapshot without anchor keeps old payloads at the bottom": () => {
    const s = applySnapshot(createState(7), snap(1, [wire(1, "a")]), 0);
    assert.equal(s.anchor, "bottom");
  },

  "a newer revision carries its anchor": () => {
    const s = applySnapshot(createState(7), snap(1, [wire(1, "a")], "top"), 0);
    assert.equal(s.anchor, "top");
    assert.equal(s.rev, 1);
  },

  "the same revision with a new anchor keeps the bubbles and their lifetimes":
    () => {
      const first = applySnapshot(
        createState(7),
        snap(1, [wire(1, "a")], "bottom"),
        1000,
      );
      const moved = applySnapshot(
        first,
        snap(1, [wire(9, "other")], "top"),
        2000,
      );
      assert.equal(moved.anchor, "top");
      assert.equal(
        moved.items,
        first.items,
        "the item list is the same object",
      );
      assert.deepEqual(
        visibleBubbles(moved, 2000).map((b) => b.text),
        ["a"],
        "the same revision does not replace the bubbles",
      );
      assert.equal(
        visibleBubbles(moved, 4999).length,
        1,
        "alive until its ttl",
      );
      assert.equal(visibleBubbles(moved, 5000).length, 0, "not restarted");
    },

  "the same revision with the same anchor returns the same state": () => {
    const first = applySnapshot(createState(7), snap(1, [wire(1, "a")]), 0);
    assert.equal(applySnapshot(first, snap(1, [], "bottom"), 0), first);
  },

  "an older revision cannot change the anchor": () => {
    const first = applySnapshot(createState(7), snap(5, [], "top"), 0);
    assert.equal(applySnapshot(first, snap(4, [], "bottom"), 0), first);
  },

  "a stale instance cannot change the anchor": () => {
    const first = applySnapshot(createState(7), snap(1, [], "top"), 0);
    const other = { ...snap(1, [], "bottom"), instance: 8 };
    assert.equal(applySnapshot(first, other, 0), first);
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
console.log("bubbleAnchor ok");
