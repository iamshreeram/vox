import assert from "node:assert/strict";
import { EXPERIMENTAL_GROUPS, visibleControls } from "./experimentalLayout";

// The 17 controls the flat Experimental page rendered before the regrouping.
// `tts` and `realtime` are composite components kept whole.
const LEGACY_CONTROLS = [
  "postProcessing",
  "keyboardImplementation",
  "acceleration",
  "lazyStreamClose",
  "vadBackend",
  "memoryToggle",
  "memoryFacts",
  "voiceCommands",
  "voiceShortcuts",
  "voiceMediaControls",
  "agentBridge",
  "agentContext",
  "agentBridgeBinaryPath",
  "wakeWord",
  "ambientMode",
  "tts",
  "realtime",
];

const allFlags = { memoryEnabled: true, agentBridgeEnabled: true };
const noFlags = { memoryEnabled: false, agentBridgeEnabled: false };

const scenarios: Record<string, () => void> = {
  "seven groups with the expected ids in order": () => {
    assert.deepEqual(
      EXPERIMENTAL_GROUPS.map((g) => g.id),
      [
        "engine",
        "voiceCommands",
        "listening",
        "memory",
        "agentBridge",
        "spokenReplies",
        "realtime",
      ],
    );
  },
  "only the engine group is open by default": () => {
    assert.deepEqual(
      EXPERIMENTAL_GROUPS.filter((g) => g.defaultOpen).map((g) => g.id),
      ["engine"],
    );
  },
  "every legacy control appears exactly once and nothing else": () => {
    const ids = EXPERIMENTAL_GROUPS.flatMap((g) => g.controls.map((c) => c.id));
    assert.deepEqual([...ids].sort(), [...LEGACY_CONTROLS].sort());
    assert.equal(new Set(ids).size, ids.length);
  },
  "every group has a non-empty translation title key": () => {
    for (const g of EXPERIMENTAL_GROUPS) {
      assert.match(g.titleKey, /^settings\.[A-Za-z.]+$/, g.id);
      assert.ok(g.controls.length > 0, g.id);
    }
  },
  "group ids are unique": () => {
    const ids = EXPERIMENTAL_GROUPS.map((g) => g.id);
    assert.equal(new Set(ids).size, ids.length);
  },
  "conditional controls are gated by their own flag only": () => {
    const memory = EXPERIMENTAL_GROUPS.find((g) => g.id === "memory")!;
    const agent = EXPERIMENTAL_GROUPS.find((g) => g.id === "agentBridge")!;
    assert.deepEqual(visibleControls(memory, noFlags), ["memoryToggle"]);
    assert.deepEqual(visibleControls(memory, allFlags), [
      "memoryToggle",
      "memoryFacts",
    ]);
    assert.deepEqual(
      visibleControls(memory, {
        memoryEnabled: false,
        agentBridgeEnabled: true,
      }),
      ["memoryToggle"],
    );
    assert.deepEqual(visibleControls(agent, noFlags), [
      "agentBridge",
      "agentContext",
    ]);
    assert.deepEqual(
      visibleControls(agent, {
        memoryEnabled: true,
        agentBridgeEnabled: false,
      }),
      ["agentBridge", "agentContext"],
    );
    assert.deepEqual(visibleControls(agent, allFlags), [
      "agentBridge",
      "agentContext",
      "agentBridgeBinaryPath",
    ]);
  },
  "with every flag on all 17 controls are visible": () => {
    const all = EXPERIMENTAL_GROUPS.flatMap((g) =>
      visibleControls(g, allFlags),
    );
    assert.equal(all.length, LEGACY_CONTROLS.length);
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
if (failed > 0)
  throw new Error(`${failed} experimental layout scenarios failed`);
console.log(
  `experimental layout: ${Object.keys(scenarios).length} scenarios passed`,
);
