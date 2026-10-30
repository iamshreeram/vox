// N1e spec: notice bubbles on the bubble page (DESIGN.md A-C3). A notice carries only a code; the
// page renders the localised text for that code, or the generic unknown text. No raw code and no
// free text is ever rendered. Authoritative: do not weaken an assertion; report a wrong one.
// Run with `bun run test:bubbles`.
//
// Contract (src/bubbles/bubbleModel.ts):
//   NOTICE_CODES: the six known codes plus "unknown".
//   noticeMessageKey(code: unknown) -> "bubbles.notice.<code>" for a known code, else
//     "bubbles.notice.unknown" (missing, empty, non-string or unrecognised input).
//   Wire items may carry kind ("transcript" | "notice", missing = transcript) and notice_code.
//   visibleBubbles returns kind and noticeCode ("notice" items keep their code, text is "").

import assert from "node:assert/strict";
import {
  NOTICE_CODES,
  applySnapshot,
  createState,
  noticeMessageKey,
  visibleBubbles,
  type Snapshot,
} from "./bubbleModel";

const INSTANCE = 3;

const noticeSnapshot = (rev: number): Snapshot => ({
  instance: INSTANCE,
  rev,
  items: [
    {
      id: 0,
      role: "assistant",
      text: "",
      age_ms: 0,
      ttl_ms: 4000,
      kind: "notice",
      notice_code: "tts_off",
    },
  ],
});

const scenarios: Record<string, () => void> = {
  "known codes map to their own key": () => {
    assert.deepEqual([...NOTICE_CODES].sort(), [
      "ambient",
      "composer_off",
      "dictation_busy",
      "experimental_off",
      "text_ending",
      "tts_off",
      "unknown",
    ]);
    for (const code of NOTICE_CODES) {
      assert.equal(noticeMessageKey(code), `bubbles.notice.${code}`);
    }
  },

  "unknown, missing or malformed codes use the generic key": () => {
    for (const raw of [undefined, null, "", 42, "something_new", "<b>x</b>"]) {
      assert.equal(
        noticeMessageKey(raw),
        "bubbles.notice.unknown",
        String(raw),
      );
    }
  },

  "a notice is visible with its code and no text": () => {
    const state = applySnapshot(
      createState(INSTANCE),
      noticeSnapshot(1),
      1_000,
    );
    const shown = visibleBubbles(state, 1_000);
    assert.equal(shown.length, 1);
    assert.equal(shown[0].kind, "notice");
    assert.equal(shown[0].noticeCode, "tts_off");
    assert.equal(shown[0].text, "");
  },

  "a notice dismisses at its ttl (4 s)": () => {
    const state = applySnapshot(
      createState(INSTANCE),
      noticeSnapshot(1),
      1_000,
    );
    assert.equal(visibleBubbles(state, 4_999).length, 1);
    assert.equal(visibleBubbles(state, 5_000).length, 0);
  },

  "an item without kind is a transcript line": () => {
    const snapshot: Snapshot = {
      instance: INSTANCE,
      rev: 1,
      items: [{ id: 5, role: "user", text: "hi", age_ms: 0, ttl_ms: 4000 }],
    };
    const shown = visibleBubbles(
      applySnapshot(createState(INSTANCE), snapshot, 0),
      0,
    );
    assert.equal(shown[0].kind, "transcript");
    assert.equal(shown[0].noticeCode, null);
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
console.log("bubbleNotice ok");
