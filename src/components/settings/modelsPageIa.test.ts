import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  PLACED_GROUPS,
  groupsForSection,
  placementOf,
  visibleGroups,
  type ControlId,
} from "./settingsPlacement";

// P1 spec (DESIGN.md Rev 5): the Models page is three collapsible groups built from one
// shared container; Reply style and Real-time leave it; Smart turn and Speaker filter join
// Voice detection; the groups needed for a working install no longer need Experimental.
const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");
const en = JSON.parse(
  readFileSync(
    new URL("../../i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;
const lookup = (key: string): unknown =>
  key
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node !== null && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      en,
    );

const ids = (section: Parameters<typeof groupsForSection>[0]): string[] =>
  groupsForSection(section).map((group) => group.id);
const controls = (groupId: string): ControlId[] =>
  (PLACED_GROUPS.find((group) => group.id === groupId)?.controls ?? []).map(
    (entry) => entry.id,
  );

const scenarios: Record<string, () => void> = {
  "Models page placed groups are Text-to-speech then Voice detection": () => {
    assert.deepEqual(ids("models"), ["spokenReplies", "vadEngine"]);
    assert.deepEqual(controls("spokenReplies"), ["tts"]);
    assert.deepEqual(controls("vadEngine"), [
      "vadBackend",
      "smartTurn",
      "speakerFilter",
    ]);
  },

  "Reply style and Real-time are no longer on the Models page": () => {
    for (const id of ["replyStyle", "realtime"] as const) {
      assert.equal(placementOf(id).section, "general", `${id} section`);
      assert.equal(placementOf(id).groupId, "conversation", `${id} group`);
    }
    assert.deepEqual(controls("conversation"), [
      "conversation",
      "persona",
      "replyStyle",
      "realtime",
      "latencyPanel",
    ]);
    for (const group of groupsForSection("models")) {
      for (const entry of group.controls) {
        assert.ok(
          entry.id !== "replyStyle" && entry.id !== "realtime",
          `${group.id} must not hold ${entry.id}`,
        );
      }
    }
  },

  "Smart turn lives with VAD, not in its own group": () => {
    assert.equal(placementOf("smartTurn").groupId, "vadEngine");
    assert.equal(placementOf("speakerFilter").groupId, "vadEngine");
    assert.ok(!PLACED_GROUPS.some((group) => group.id === "smartTurn"));
    assert.ok(!PLACED_GROUPS.some((group) => group.id === "speakerFilter"));
  },

  "the groups needed for a working install are not experimental": () => {
    const open = new Set(["spokenReplies", "vadEngine", "conversation"]);
    for (const group of PLACED_GROUPS) {
      assert.equal(
        group.experimental,
        !open.has(group.id),
        `${group.id}.experimental`,
      );
    }
  },

  "visibleGroups honours the per-group experimental flag": () => {
    const names = (section: Parameters<typeof visibleGroups>[0], on: boolean) =>
      visibleGroups(section, on).map((group) => group.id);
    assert.deepEqual(names("models", false), ["spokenReplies", "vadEngine"]);
    assert.deepEqual(names("general", false), ["conversation"]);
    assert.deepEqual(names("advanced", false), []);
    assert.deepEqual(names("agents", false), []);
    for (const section of [
      "general",
      "models",
      "advanced",
      "agents",
    ] as const) {
      assert.deepEqual(names(section, true), ids(section), `${section} on`);
    }
  },

  "group titles are Text-to-speech and Voice detection": () => {
    const tts = PLACED_GROUPS.find((group) => group.id === "spokenReplies");
    const vad = PLACED_GROUPS.find((group) => group.id === "vadEngine");
    assert.equal(tts?.titleKey, "settings.models.groups.textToSpeech");
    assert.equal(vad?.titleKey, "settings.models.groups.voiceDetection");
    assert.equal(
      lookup("settings.models.groups.textToSpeech"),
      "Text-to-speech",
    );
    assert.equal(
      lookup("settings.models.groups.voiceDetection"),
      "Voice detection",
    );
    assert.equal(
      lookup("settings.models.groups.speechToText"),
      "Speech-to-text",
    );
  },

  "Speech-to-text is a collapsible group with the same container, open by default":
    () => {
      const models = read("./models/ModelsSettings.tsx");
      const jsx = models.slice(models.indexOf("<ModelSettingsCard"));
      // Group opens before the language/translate card and the unload timeout.
      const group = models.indexOf('id="sttModels"');
      assert.ok(group > 0, 'ModelsSettings must render id="sttModels"');
      assert.ok(
        models.lastIndexOf("<CollapsibleSettingsGroup", group) >
          models.lastIndexOf("</CollapsibleSettingsGroup>", group),
        "sttModels is rendered by the shared CollapsibleSettingsGroup",
      );
      assert.match(
        models.slice(group, group + 300),
        /defaultOpen(=\{true\}|\s|\/|>)/,
        "sttModels opens by default",
      );
      const closeGroup = models.indexOf("</CollapsibleSettingsGroup>", group);
      for (const inside of [
        "<ModelSettingsCard",
        "<ModelUnloadTimeoutSetting",
        "settings.models.yourModels",
        "settings.models.availableModels",
      ]) {
        const at = models.indexOf(inside);
        assert.ok(
          at > group && at < closeGroup,
          `${inside} is inside sttModels`,
        );
      }
      assert.ok(
        jsx.indexOf('<PlacedGroups section="models"') >
          jsx.indexOf("sttModels"),
        "placed groups follow the Speech-to-text group",
      );
      // No bare top-level "Model loading" plain group any more.
      assert.ok(
        !/<SettingsGroup[^>]*settings\.models\.groups\.loading/.test(models),
        "model loading timeout is inside the Speech-to-text group, not a plain group",
      );
    },

  "the Speech-to-text group title comes from i18n": () => {
    const models = read("./models/ModelsSettings.tsx");
    assert.ok(models.includes("settings.models.groups.speechToText"));
  },

  "search metadata follows the group flag, not a blanket true": () => {
    const index = read("./search/index/experimental.ts");
    assert.ok(
      !/requiresExperimental:\s*true,/.test(index),
      "requiresExperimental must be derived from group.experimental",
    );
    assert.match(index, /group\.experimental/);
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
console.log("modelsPageIa ok");
