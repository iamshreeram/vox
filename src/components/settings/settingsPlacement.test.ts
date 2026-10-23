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

// The 21 controls that the Experimental page rendered: 18 from its layout list plus
// the three groups that were hard-coded in the page (conversationMemory, agentTools,
// codingJobs).
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
];

// Group ids that existed before the split. They are persisted verbatim in
// localStorage open-state keys, so every one of them must survive unchanged.
const LEGACY_GROUP_IDS = [
  "engine",
  "voiceCommands",
  "listening",
  "memory",
  "agentBridge",
  "acpAgent",
  "spokenReplies",
  "realtime",
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
    assert.equal(ALL_CONTROLS.length, 21);
  },
  "only the four destination sections are used, never experimental": () => {
    assert.deepEqual(
      [...new Set(PLACED_GROUPS.map((g) => g.section))].sort(),
      [...SECTIONS].sort(),
    );
  },
  "groups per section, in order": () => {
    const ids = (s: PlacedSection) => groupsForSection(s).map((g) => g.id);
    assert.deepEqual(ids("general"), ["voiceCommands", "listening"]);
    assert.deepEqual(ids("models"), ["vadEngine", "spokenReplies", "realtime"]);
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
    for (const id of ["vadBackend", "tts", "realtime"] as ControlId[]) {
      assert.equal(sec(id), "models", id);
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
    assert.equal(placementOf("realtime").groupId, "realtime");
  },
  "every legacy group id is retained, only vadEngine is new": () => {
    const ids = PLACED_GROUPS.map((g) => g.id);
    assert.equal(new Set(ids).size, ids.length, "group ids are unique");
    assert.deepEqual(
      ids.filter((id) => id !== "vadEngine").sort(),
      [...LEGACY_GROUP_IDS].sort(),
    );
    assert.ok(ids.includes("vadEngine"));
  },
  "only the engine group is open by default": () => {
    assert.deepEqual(
      PLACED_GROUPS.filter((g) => g.defaultOpen).map((g) => g.id),
      ["engine"],
    );
  },
  "every group has a non-empty translation title key": () => {
    for (const g of PLACED_GROUPS) {
      assert.match(g.titleKey, /^settings\.[A-Za-z.]+$/, g.id);
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
  },
  "gate off: no moved group renders in any destination section": () => {
    for (const s of SECTIONS) {
      assert.equal(markup(s, false), "", s);
    }
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
  "titles go through the injected translator": () => {
    const html = markup("models", true);
    assert.ok(html.includes("T:settings.advanced.tts.groupTitle"));
    assert.ok(html.includes("T:settings.experimental.realtime.title"));
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
