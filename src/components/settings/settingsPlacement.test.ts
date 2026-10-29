import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import {
  PLACED_GROUPS,
  groupsForSection,
  placementOf,
  visibleControls,
  type ControlId,
  type PlacedSection,
} from "./settingsPlacement";
import { PlacedGroupsView } from "./PlacedGroups";
import { SETTINGS_SECTION_IDS, resolveSectionAlias } from "./agentToolsForm";
import { shouldFallBackToGeneral } from "../sectionGate";

// The 27 placed controls: the 21 that the old Experimental page rendered (18 from its layout
// list plus conversationMemory, agentTools, codingJobs) and six that used to be buried
// inside the one big "Spoken replies" control (reply style, conversation, persona, latency
// panel, smart turn, speaker filter) and are now placed on their own. Nine of the 27 sit in
// always-visible groups (P1); the other 18 stay behind the Experimental switch.
const ALL_CONTROLS: ControlId[] = [
  "postProcessing",
  "keyboardImplementation",
  "acceleration",
  "lazyStreamClose",
  "vadBackend",
  "voiceCommands",
  "voiceShortcuts",
  "voiceMediaControls",
  "wakeWord",
  "ambientMode",
  "memoryToggle",
  "memoryFacts",
  "conversationMemory",
  "agentBridge",
  "agentContext",
  "agentBridgeBinaryPath",
  "acpAgent",
  "agentTools",
  "codingJobs",
  "tts",
  "realtime",
  "replyStyle",
  "conversation",
  "floatingOrb",
  "persona",
  "latencyPanel",
  "smartTurn",
  "speakerFilter",
];

// Group ids that existed before the split. They are persisted verbatim in
// localStorage open-state keys, so every one of them must survive unchanged.
const NEW_GROUP_IDS = ["vadEngine", "conversation"];
const LEGACY_GROUP_IDS = [
  "engine",
  "voiceCommands",
  "listening",
  "memory",
  "agentBridge",
  "acpAgent",
  "spokenReplies",
  "conversationMemory",
  "agentTools",
  "codingJobs",
];

const SECTIONS: PlacedSection[] = ["general", "models", "advanced", "agents"];
const allFlags = { memoryEnabled: true, agentBridgeEnabled: true };
const noFlags = { memoryEnabled: false, agentBridgeEnabled: false };

const fakeRender = Object.fromEntries(
  ALL_CONTROLS.map((id) => [
    id,
    () => React.createElement("i", { "data-ctl": id }),
  ]),
) as Record<ControlId, () => React.ReactNode>;

const markup = (
  section: PlacedSection,
  experimentalEnabled: boolean,
  flags = allFlags,
): string =>
  renderToStaticMarkup(
    React.createElement(PlacedGroupsView, {
      section,
      experimentalEnabled,
      flags,
      render: fakeRender,
      t: (key: string) => `T:${key}`,
    }),
  );

const count = (haystack: string, needle: string): number =>
  haystack.split(needle).length - 1;

const scenarios: Record<string, () => void> = {
  "every control is placed exactly once and nothing else": () => {
    const ids = PLACED_GROUPS.flatMap((g) => g.controls.map((c) => c.id));
    assert.deepEqual([...ids].sort(), [...ALL_CONTROLS].sort());
    assert.equal(new Set(ids).size, ids.length);
    assert.equal(ALL_CONTROLS.length, 28);
  },
  "only the four destination sections are used, never experimental": () => {
    assert.deepEqual(
      [...new Set(PLACED_GROUPS.map((g) => g.section))].sort(),
      [...SECTIONS].sort(),
    );
  },
  "groups per section, in order": () => {
    const ids = (s: PlacedSection) => groupsForSection(s).map((g) => g.id);
    assert.deepEqual(ids("general"), [
      "voiceCommands",
      "listening",
      "conversation",
    ]);
    assert.deepEqual(ids("models"), ["spokenReplies", "vadEngine"]);
    assert.deepEqual(ids("advanced"), ["engine"]);
    assert.deepEqual(ids("agents"), [
      "acpAgent",
      "agentBridge",
      "agentTools",
      "codingJobs",
      "memory",
      "conversationMemory",
    ]);
  },
  "model and engine choices live in Models, everything else by topic": () => {
    const sec = (id: ControlId) => placementOf(id).section;
    for (const id of [
      "vadBackend",
      "tts",
      "smartTurn",
      "speakerFilter",
    ] as ControlId[]) {
      assert.equal(sec(id), "models", id);
    }
    for (const id of [
      "conversation",
      "persona",
      "replyStyle",
      "realtime",
      "latencyPanel",
    ] as ControlId[]) {
      assert.equal(placementOf(id).section, "general", id);
      assert.equal(placementOf(id).groupId, "conversation", id);
    }
    for (const id of [
      "voiceCommands",
      "voiceShortcuts",
      "voiceMediaControls",
      "wakeWord",
      "ambientMode",
    ] as ControlId[]) {
      assert.equal(sec(id), "general", id);
    }
    for (const id of [
      "postProcessing",
      "keyboardImplementation",
      "acceleration",
      "lazyStreamClose",
    ] as ControlId[]) {
      assert.equal(sec(id), "advanced", id);
    }
    for (const id of [
      "memoryToggle",
      "memoryFacts",
      "conversationMemory",
      "agentBridge",
      "agentContext",
      "agentBridgeBinaryPath",
      "acpAgent",
      "agentTools",
      "codingJobs",
    ] as ControlId[]) {
      assert.equal(sec(id), "agents", id);
    }
    assert.equal(placementOf("vadBackend").groupId, "vadEngine");
    assert.equal(placementOf("tts").groupId, "spokenReplies");
    assert.equal(placementOf("realtime").groupId, "conversation");
    assert.equal(placementOf("smartTurn").groupId, "vadEngine");
    assert.equal(placementOf("speakerFilter").groupId, "vadEngine");
    assert.equal(placementOf("replyStyle").groupId, "conversation");
  },
  "the wake word is ungated in Conversation; ambient listening stays experimental":
    () => {
      // A-W5: one trigger (Hey Jarvis) lives with the conversation controls and is always visible.
      assert.equal(placementOf("wakeWord").groupId, "conversation");
      assert.equal(placementOf("wakeWord").section, "general");
      assert.equal(
        PLACED_GROUPS.find((g) => g.id === "conversation")?.experimental,
        false,
      );
      assert.equal(placementOf("ambientMode").groupId, "listening");
      assert.equal(
        PLACED_GROUPS.find((g) => g.id === "listening")?.experimental,
        true,
      );
      const general = markup("general", false);
      assert.equal(count(general, 'data-ctl="wakeWord"'), 1);
      assert.equal(count(general, 'data-ctl="ambientMode"'), 0);
    },
  "every legacy group id is retained, only the listed ids are new": () => {
    const ids = PLACED_GROUPS.map((g) => g.id);
    assert.equal(new Set(ids).size, ids.length, "group ids are unique");
    assert.deepEqual(
      ids.filter((id) => !NEW_GROUP_IDS.includes(id)).sort(),
      [...LEGACY_GROUP_IDS].sort(),
    );
    for (const id of NEW_GROUP_IDS) assert.ok(ids.includes(id), id);
  },
  "only the Text-to-speech and engine groups are open by default": () => {
    assert.deepEqual(
      PLACED_GROUPS.filter((g) => g.defaultOpen).map((g) => g.id),
      ["spokenReplies", "engine"],
    );
  },
  "every group has a non-empty translation title key": () => {
    for (const g of PLACED_GROUPS) {
      assert.match(g.titleKey, /^settings\.[A-Za-z.]+$/, g.id);
      assert.ok(g.controls.length > 0, `${g.id} has controls`);
    }
  },
  "placementOf agrees with the group that holds the control": () => {
    for (const g of PLACED_GROUPS) {
      for (const c of g.controls) {
        assert.deepEqual(placementOf(c.id), {
          section: g.section,
          groupId: g.id,
        });
      }
    }
  },
  "flag-conditional controls follow their flag": () => {
    const g = (id: string) => PLACED_GROUPS.find((x) => x.id === id);
    const bridge = g("agentBridge");
    const memory = g("memory");
    assert.ok(bridge && memory);
    assert.deepEqual(visibleControls(bridge, allFlags), [
      "agentBridge",
      "agentContext",
      "agentBridgeBinaryPath",
    ]);
    assert.deepEqual(visibleControls(bridge, noFlags), [
      "agentBridge",
      "agentContext",
    ]);
    assert.deepEqual(visibleControls(memory, allFlags), [
      "memoryToggle",
      "memoryFacts",
    ]);
    assert.deepEqual(visibleControls(memory, noFlags), ["memoryToggle"]);
    // Each optional control follows ITS OWN flag only (mixed flags).
    const onlyBridge = { memoryEnabled: false, agentBridgeEnabled: true };
    const onlyMemory = { memoryEnabled: true, agentBridgeEnabled: false };
    assert.deepEqual(visibleControls(memory, onlyBridge), ["memoryToggle"]);
    assert.deepEqual(visibleControls(bridge, onlyBridge), [
      "agentBridge",
      "agentContext",
      "agentBridgeBinaryPath",
    ]);
    assert.deepEqual(visibleControls(memory, onlyMemory), [
      "memoryToggle",
      "memoryFacts",
    ]);
    assert.deepEqual(visibleControls(bridge, onlyMemory), [
      "agentBridge",
      "agentContext",
    ]);
  },
  "gate off: only the always-visible groups render, in their own section":
    () => {
      for (const s of SECTIONS) {
        const html = markup(s, false);
        for (const g of PLACED_GROUPS) {
          const expected = g.section === s && !g.experimental ? 1 : 0;
          assert.equal(
            count(html, `data-group-id="${g.id}"`),
            expected,
            `${s}/${g.id}`,
          );
        }
      }
      assert.equal(markup("advanced", false), "");
      assert.equal(markup("agents", false), "");
    },
  "gate on: each expected group renders exactly once, and no other section's":
    () => {
      for (const s of SECTIONS) {
        const html = markup(s, true);
        for (const g of PLACED_GROUPS) {
          const expected = g.section === s ? 1 : 0;
          assert.equal(
            count(html, `data-group-id="${g.id}"`),
            expected,
            `${s}/${g.id}`,
          );
        }
      }
    },
  "gate on: each control marker appears exactly once across all sections":
    () => {
      const html = SECTIONS.map((s) => markup(s, true)).join("");
      for (const id of ALL_CONTROLS) {
        assert.equal(count(html, `data-ctl="${id}"`), 1, id);
      }
    },
  "gate on with flags off hides the conditional controls only": () => {
    const html = SECTIONS.map((s) => markup(s, true, noFlags)).join("");
    assert.equal(count(html, 'data-ctl="memoryFacts"'), 0);
    assert.equal(count(html, 'data-ctl="agentBridgeBinaryPath"'), 0);
    for (const id of ALL_CONTROLS.filter(
      (c) => c !== "memoryFacts" && c !== "agentBridgeBinaryPath",
    )) {
      assert.equal(count(html, `data-ctl="${id}"`), 1, id);
    }
  },
  "experimental controls inside an ungated group follow the switch": () => {
    const off = markup("general", false);
    const on = markup("general", true);
    for (const id of ["realtime", "latencyPanel"]) {
      assert.equal(count(off, `data-ctl="${id}"`), 0, `${id} off`);
      assert.equal(count(on, `data-ctl="${id}"`), 1, `${id} on`);
    }
    for (const id of ["replyStyle", "conversation", "persona"]) {
      assert.equal(count(off, `data-ctl="${id}"`), 1, `${id} off`);
    }
    assert.equal(
      count(off, 'data-group-id="conversation"'),
      1,
      "the Conversation group itself stays ungated",
    );
  },
  "titles go through the injected translator": () => {
    const html = markup("models", true);
    assert.ok(html.includes("T:settings.models.groups.textToSpeech"));
    assert.ok(html.includes("T:settings.models.groups.voiceDetection"));
    assert.ok(
      markup("general", false).includes(
        "T:settings.general.groups.conversation",
      ),
    );
    assert.ok(
      markup("general", true).includes(
        "T:settings.general.groups.conversation",
      ),
    );
  },
  "legacy experimental section id resolves to agents": () => {
    assert.equal(resolveSectionAlias("experimental"), "agents");
    assert.equal(resolveSectionAlias("models"), "models");
    assert.ok(SETTINGS_SECTION_IDS.includes("agents"));
    assert.ok(SETTINGS_SECTION_IDS.includes("experimental"));
    assert.ok(!(SETTINGS_SECTION_IDS as readonly string[]).includes("debug"));
  },
  "agents falls back to general when the gate is off, including via the alias":
    () => {
      const sections = {
        agents: {
          enabled: (s: { experimental_enabled?: boolean }) =>
            s?.experimental_enabled === true,
        },
        general: { enabled: () => true },
      };
      assert.equal(
        shouldFallBackToGeneral(
          "agents",
          { experimental_enabled: false },
          sections,
        ),
        true,
      );
      assert.equal(
        shouldFallBackToGeneral(
          "agents",
          { experimental_enabled: true },
          sections,
        ),
        false,
      );
      // An old tool call for "experimental" lands on agents first, then the gate decides.
      const landed = resolveSectionAlias("experimental");
      assert.equal(
        shouldFallBackToGeneral(
          landed,
          { experimental_enabled: false },
          sections,
        ),
        true,
      );
      // Settings not loaded yet: never fall back.
      assert.equal(shouldFallBackToGeneral("agents", null, sections), false);
    },
};

let failed = 0;
for (const [name, fn] of Object.entries(scenarios)) {
  try {
    fn();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n`, error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log(
  `settings placement: ${Object.keys(scenarios).length} scenarios passed`,
);
