import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";

// Source-level guards for the page wiring that the pure placement tests cannot see.
// (The mounted behaviour is covered by tests/settings-search.spec.ts.)
const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");
const exists = (relative: string): boolean =>
  existsSync(new URL(relative, import.meta.url));
const count = (haystack: string, needle: string): number =>
  haystack.split(needle).length - 1;

const models = read("./models/ModelsSettings.tsx");
// The page JSX starts at the first rendered card (the file also holds hooks and helpers).
const modelsJsx = models.slice(models.indexOf("<ModelSettingsCard"));
// A bare `loading` identifier (not a translation key like `groups.loading`, not `downloading`).
const LOADING_IDENT = /(?<![.\w"'])loading(?![\w"'])/;

const scenarios: Record<string, () => void> = {
  "language/translate card and unload timeout are on the Models page": () => {
    assert.ok(exists("./models/ModelSettingsCard.tsx"));
    assert.ok(!exists("./general/ModelSettingsCard.tsx"));
    assert.match(
      models,
      /import \{ ModelSettingsCard \} from "\.\/ModelSettingsCard"/,
    );
    assert.match(models, /ModelUnloadTimeoutSetting/);
    assert.ok(models.includes('"settings.models.groups.speechToText"'));
    assert.ok(modelsJsx.includes("<ModelSettingsCard"));
    assert.ok(modelsJsx.includes("<ModelUnloadTimeoutSetting"));
    assert.ok(modelsJsx.includes('<PlacedGroups section="models"'));
  },
  "General and Advanced no longer render the moved controls": () => {
    assert.ok(
      !read("./general/GeneralSettings.tsx").includes("ModelSettingsCard"),
    );
    assert.ok(
      !read("./advanced/AdvancedSettings.tsx").includes(
        "ModelUnloadTimeoutSetting",
      ),
    );
  },
  "the Models page never hides core controls behind the catalog loading state":
    () => {
      assert.ok(
        !/if \(loading\)\s*\{\s*return/.test(models),
        "no whole-page early return while the catalog loads",
      );
      const firstLoading = modelsJsx.search(LOADING_IDENT);
      assert.ok(firstLoading > 0, "the list area still shows a loading state");
      const heading = modelsJsx.indexOf("settings.models.transcriptionHeading");
      assert.ok(heading > 0);
      // Core cards and the gated groups are rendered outside the loading branch.
      assert.ok(modelsJsx.indexOf("<ModelSettingsCard") < firstLoading);
      assert.ok(modelsJsx.indexOf("<ModelUnloadTimeoutSetting") < firstLoading);
      assert.ok(heading < firstLoading, "spinner belongs to the list area");
      assert.ok(
        modelsJsx.indexOf('<PlacedGroups section="models"') > firstLoading,
      );
    },
  "TtsSettings keeps only the speech engine and voice choices": () => {
    const tts = read("./TtsSettings.tsx");
    for (const name of [
      "SpokenReplyStyle",
      "ConversationSection",
      "VoiceFeaturesSection",
    ]) {
      assert.ok(!tts.includes(name), `TtsSettings still references ${name}`);
    }
    assert.ok(!exists("./voice/VoiceFeaturesSection.tsx"));
  },
  "each split-out component is rendered exactly once, by placedRenderers":
    () => {
      const renderers = read("./placedRenderers.tsx");
      for (const name of [
        "SpokenReplyStyle",
        "ConversationSection",
        "PersonaSettings",
        "LatencyPanel",
        "SmartTurnSettings",
        "SpeakerFilterSettings",
      ]) {
        assert.equal(count(renderers, `<${name}`), 1, name);
      }
    },
  "no 'Experimental' heading is rendered inside the topic tabs": () => {
    const placed = read("./PlacedGroups.tsx");
    assert.ok(!placed.includes("groups.experimental"));
    assert.ok(!placed.includes("showBadge"));
    assert.ok(!read("./agents/AgentsSettings.tsx").includes("showBadge"));
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
  `models page placement: ${Object.keys(scenarios).length} scenarios passed`,
);
