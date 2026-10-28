// G4a spec: pure page model for the bubble window (DESIGN.md A7-2, A8-2, R6-4 page model).
// Authoritative: do not weaken an assertion; if a test is wrong, report it. Run with
// `bun run test:bubbles`.
//
// Contract (src/bubbles/bubbleModel.ts):
//   type BubbleItemWire = { id: number; role: "user" | "assistant"; text: string; age_ms: number; ttl_ms: number }
//   type Snapshot = { instance: number; rev: number; items: BubbleItemWire[] }   (as sent by Rust)
//   FADE_MS = 600
//   createState(instance) -> state (rev starts at 0; Rust revs start at 1)
//   applySnapshot(state, snapshot, nowMs) -> new state. Ignored (state returned unchanged) when
//     snapshot.instance !== state.instance or snapshot.rev <= state.rev. Otherwise replaces the whole
//     list; bornAt = nowMs - age_ms, so a replay after reload keeps the original lifetime.
//   visibleBubbles(state, nowMs) -> [{ id, role, text, opacity }] oldest first.
//     opacity 1 until ttl - FADE_MS, linear to 0 at ttl; removed once age >= ttl.
//   alignFor(role) -> "right" for user, "left" for assistant
//   fitBubbles(heights, container, gap) -> how many NEWEST bubbles fit (drop oldest first, the
//     newest is always kept, so >= 1 when heights is non-empty; 0 for empty).
//   enterMotion(reducedMotion) -> { durationMs: 180, translatePx: 8 } or { durationMs: 0, translatePx: 0 }
//   capText(text) -> same rule as the Rust cap_display_text (<= 240 code points, word-boundary cut
//     with an ellipsis, whitespace collapsed and trimmed).

import assert from "node:assert/strict";
import {
  FADE_MS,
  alignFor,
  applySnapshot,
  capText,
  createState,
  enterMotion,
  fitBubbles,
  visibleBubbles,
  type BubbleItemWire,
  type Snapshot,
} from "./bubbleModel";

const wire = (
  id: number,
  role: "user" | "assistant",
  text: string,
  ageMs: number,
  ttlMs: number,
): BubbleItemWire => ({ id, role, text, age_ms: ageMs, ttl_ms: ttlMs });

const snap = (
  instance: number,
  rev: number,
  items: BubbleItemWire[],
): Snapshot => ({
  instance,
  rev,
  items,
});

const texts = (
  state: ReturnType<typeof createState>,
  nowMs: number,
): string[] => visibleBubbles(state, nowMs).map((b) => b.text);

const near = (actual: number, expected: number): void => {
  assert.ok(
    Math.abs(actual - expected) < 1e-9,
    `expected ${expected}, got ${actual}`,
  );
};

const scenarios: Record<string, () => void> = {
  "FADE_MS is 600 ms (A7-2 lifetime fade)": () => {
    assert.equal(FADE_MS, 600);
  },

  "a snapshot for another instance is ignored (A7-2)": () => {
    const s0 = createState(7);
    const s1 = applySnapshot(
      s0,
      snap(8, 1, [wire(1, "user", "hi", 0, 4000)]),
      1000,
    );
    assert.deepEqual(texts(s1, 1000), []);
  },

  "an older or equal rev is ignored (A7-2 idempotent replay)": () => {
    const s1 = applySnapshot(
      createState(7),
      snap(7, 5, [wire(1, "user", "A", 0, 4000)]),
      0,
    );
    const equal = applySnapshot(
      s1,
      snap(7, 5, [wire(2, "user", "B", 0, 4000)]),
      0,
    );
    assert.deepEqual(texts(equal, 0), ["A"]);
    const older = applySnapshot(
      s1,
      snap(7, 4, [wire(3, "user", "C", 0, 4000)]),
      0,
    );
    assert.deepEqual(texts(older, 0), ["A"]);
  },

  "a newer rev replaces the whole list": () => {
    const s1 = applySnapshot(
      createState(7),
      snap(7, 1, [
        wire(1, "user", "A", 0, 4000),
        wire(2, "assistant", "B", 0, 4000),
      ]),
      0,
    );
    const s2 = applySnapshot(
      s1,
      snap(7, 2, [wire(2, "assistant", "B", 0, 4000)]),
      0,
    );
    assert.deepEqual(texts(s2, 0), ["B"]);
  },

  "replacing with the same snapshot is idempotent": () => {
    const payload = snap(7, 2, [
      wire(1, "user", "A", 0, 4000),
      wire(2, "assistant", "B", 0, 4000),
    ]);
    const once = applySnapshot(createState(7), payload, 0);
    const twice = applySnapshot(once, payload, 0);
    assert.deepEqual(texts(twice, 0), ["A", "B"]);
    assert.deepEqual(texts(applySnapshot(twice, payload, 0), 0), ["A", "B"]);
  },

  "an empty snapshot clears the list": () => {
    const s1 = applySnapshot(
      createState(7),
      snap(7, 1, [wire(1, "user", "A", 0, 4000)]),
      0,
    );
    const s2 = applySnapshot(s1, snap(7, 2, []), 0);
    assert.deepEqual(visibleBubbles(s2, 0), []);
  },

  "items keep the order Rust sent (oldest first)": () => {
    const s = applySnapshot(
      createState(7),
      snap(7, 1, [
        wire(1, "user", "first", 0, 8000),
        wire(2, "assistant", "second", 0, 8000),
      ]),
      0,
    );
    assert.deepEqual(
      visibleBubbles(s, 0).map((b) => b.id),
      [1, 2],
    );
  },

  "opacity follows the fade curve (A7-2 lifetime rule)": () => {
    const s = applySnapshot(
      createState(7),
      snap(7, 1, [wire(1, "user", "hi", 0, 4000)]),
      0,
    );
    near(visibleBubbles(s, 0)[0].opacity, 1);
    near(visibleBubbles(s, 3400)[0].opacity, 1); // ttl - FADE_MS: fade starts here
    near(visibleBubbles(s, 3700)[0].opacity, 0.5); // 300 / 600
    near(visibleBubbles(s, 3999)[0].opacity, 1 / 600);
  },

  "a bubble is removed once its ttl is reached": () => {
    const s = applySnapshot(
      createState(7),
      snap(7, 1, [wire(1, "user", "hi", 0, 4000)]),
      0,
    );
    assert.equal(
      visibleBubbles(s, 3999).length,
      1,
      "alive one ms before the ttl",
    );
    assert.deepEqual(visibleBubbles(s, 4000), []);
    assert.deepEqual(visibleBubbles(s, 5000), []);
  },

  "a replay after reload keeps the original lifetime (age_ms honoured)": () => {
    // Rust says the line is already 1000 ms old at now=5000, so it was born at 4000 and dies at 8000.
    const s = applySnapshot(
      createState(7),
      snap(7, 1, [wire(1, "assistant", "hello", 1000, 4000)]),
      5000,
    );
    near(visibleBubbles(s, 5000)[0].opacity, 1);
    assert.equal(visibleBubbles(s, 7999).length, 1, "still alive at 7999");
    assert.deepEqual(
      visibleBubbles(s, 8000),
      [],
      "gone at 8000, not 5000 + ttl",
    );
  },

  "alignFor: user right, assistant left": () => {
    assert.equal(alignFor("user"), "right");
    assert.equal(alignFor("assistant"), "left");
  },

  "fitBubbles: empty input fits nothing": () => {
    assert.equal(fitBubbles([], 240, 8), 0);
  },

  "fitBubbles: a single too-tall bubble is still kept (newest always kept)":
    () => {
      assert.equal(fitBubbles([500], 240, 8), 1);
    },

  "fitBubbles: exact fit counts the gaps": () => {
    assert.equal(fitBubbles([100, 100], 208, 8), 2, "100 + 8 + 100 = 208");
    assert.equal(fitBubbles([100, 100], 207, 8), 1);
    assert.equal(fitBubbles([240], 240, 8), 1);
  },

  "fitBubbles: drops the oldest first and stops at the first misfit from the newest":
    () => {
      assert.equal(fitBubbles([50, 60, 70], 1000, 8), 3);
      assert.equal(
        fitBubbles([10, 500, 20], 100, 8),
        1,
        "the 500 blocks older ones",
      );
      assert.equal(
        fitBubbles([10, 10, 10], 45, 8),
        2,
        "10 + 8 + 10 + 8 + 10 = 46 > 45",
      );
      assert.equal(fitBubbles([10, 10, 10], 56, 8), 3);
    },

  "enterMotion: 180 ms and 8 px, or nothing under reduced motion": () => {
    assert.deepEqual(enterMotion(false), { durationMs: 180, translatePx: 8 });
    assert.deepEqual(enterMotion(true), { durationMs: 0, translatePx: 0 });
  },

  "capText: empty and whitespace-only": () => {
    assert.equal(capText(""), "");
    assert.equal(capText("   \n\t "), "");
  },

  "capText: collapses and trims whitespace": () => {
    assert.equal(capText("  a \n  b\t c  "), "a b c");
  },

  "capText: text of exactly 240 code points is unchanged": () => {
    const exact = "a".repeat(240);
    assert.equal(capText(exact), exact);
    assert.equal(capText(`  ${"b".repeat(240)}  `), "b".repeat(240));
  },

  "capText: long text is cut at a word boundary with an ellipsis": () => {
    const out = capText("word ".repeat(100));
    assert.ok(Array.from(out).length <= 240, "at most 240 code points");
    assert.ok(out.endsWith("…"), `ellipsis: ${out}`);
    const body = out.slice(0, -1).trimEnd();
    assert.ok(/^(word )*word$/.test(body), `whole words only: ${body}`);
    assert.equal(capText(out), out, "idempotent");
  },

  "capText: multibyte accents stay valid": () => {
    const out = capText("é ".repeat(200));
    assert.ok(Array.from(out).length <= 240);
    assert.ok(out.endsWith("…"));
    assert.ok(/^(é )*é$/.test(out.slice(0, -1).trimEnd()));
  },

  "capText: emoji are whole code points, never broken surrogates": () => {
    const emoji = "\u{1F600}";
    const out = capText(`${emoji} `.repeat(200));
    assert.ok(Array.from(out).length <= 240);
    assert.ok(out.endsWith("…"));
    assert.ok(/^(\u{1F600} )*\u{1F600}$/u.test(out.slice(0, -1).trimEnd()));
    assert.ok(
      !/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(
        out,
      ),
      "no lone surrogate",
    );
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
console.log("bubbleModel ok");
