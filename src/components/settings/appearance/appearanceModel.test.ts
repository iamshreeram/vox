import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ACCENT_IDS } from "../../../styles/accents";
import {
  APPEARANCE_DEFAULTS,
  ACCENT_NAME_KEYS,
  ORB_CHOICES,
  ORB_FOLLOW_ID,
  accentEnabledFor,
  effectiveAppearance,
  effectiveOrbAccent,
  nextRadioIndex,
  orbChoiceId,
  orbChoiceNameKey,
  orbSelectedIndex,
  resetPlan,
  resolveSearchEntryId,
  rovingTabStops,
} from "./appearanceModel";

// Pure rules of the Appearance settings page (UI enterprise S3, DESIGN Revision 3 A3, A5, D3).
// The DOM and store wiring are covered by the store test and tests/appearance-settings.spec.ts.

const here = path.dirname(fileURLToPath(import.meta.url));
const en = JSON.parse(
  readFileSync(
    path.resolve(here, "../../../i18n/locales/en/translation.json"),
    "utf8",
  ),
) as Record<string, any>;

const scenarios: Record<string, () => void> = {
  "the accent applies only under system, light and dark (A3)": () => {
    assert.equal(accentEnabledFor("system"), true);
    assert.equal(accentEnabledFor("light"), true);
    assert.equal(accentEnabledFor("dark"), true);
    assert.equal(accentEnabledFor("aurora"), false);
    assert.equal(accentEnabledFor("custom"), false);
  },
  "defaults are the reset targets: system, amber, dark, vibrant, 70": () => {
    assert.deepEqual(APPEARANCE_DEFAULTS, {
      theme: "system",
      accent: "amber",
      chatTheme: "dark",
      visualStyle: "vibrant",
      chatOpacityPct: 70,
    });
  },
  "reset plan issues every default once, in a fixed order": () => {
    assert.deepEqual(resetPlan(), [
      { kind: "theme", value: "system" },
      { kind: "accent", value: "amber" },
      { kind: "chatTheme", value: "dark" },
      { kind: "visualStyle", value: "vibrant" },
      { kind: "chatOpacity", value: 70 },
    ]);
  },
  "arrow keys move with wrap-around, Home and End jump to the ends": () => {
    assert.equal(nextRadioIndex("ArrowRight", 0, 6), 1);
    assert.equal(nextRadioIndex("ArrowDown", 0, 6), 1);
    assert.equal(nextRadioIndex("ArrowRight", 5, 6), 0);
    assert.equal(nextRadioIndex("ArrowLeft", 0, 6), 5);
    assert.equal(nextRadioIndex("ArrowUp", 0, 6), 5);
    assert.equal(nextRadioIndex("ArrowLeft", 3, 6), 2);
    assert.equal(nextRadioIndex("Home", 4, 6), 0);
    assert.equal(nextRadioIndex("End", 1, 6), 5);
    assert.equal(nextRadioIndex("ArrowRight", 0, 2), 1);
    assert.equal(nextRadioIndex("ArrowRight", 1, 2), 0);
  },
  "other keys do not move the selection": () => {
    assert.equal(nextRadioIndex("Enter", 2, 6), null);
    assert.equal(nextRadioIndex("a", 2, 6), null);
    assert.equal(nextRadioIndex("Tab", 2, 6), null);
  },
  "roving tabindex: exactly one stop, the selected radio": () => {
    assert.deepEqual(rovingTabStops(2, 6), [-1, -1, 0, -1, -1, -1]);
    assert.deepEqual(rovingTabStops(0, 6), [0, -1, -1, -1, -1, -1]);
    // Nothing selected (e.g. an unknown stored value): the first radio is the stop.
    assert.deepEqual(rovingTabStops(-1, 3), [0, -1, -1]);
  },
  "swatch name keys cover every accent and read Amber, Blue, Violet, Emerald, Rose, Slate":
    () => {
      const names = ACCENT_IDS.map((id) => ACCENT_NAME_KEYS[id]);
      assert.deepEqual(
        names,
        ACCENT_IDS.map((id) => `settings.appearance.accent.swatch.${id}`),
      );
      const english = names.map((key) =>
        key.split(".").reduce((n, p) => n[p], en),
      );
      assert.deepEqual(english, [
        "Amber",
        "Blue",
        "Violet",
        "Emerald",
        "Rose",
        "Slate",
      ]);
    },
  // RE-POINTED (A5): the orb colour is 'auto' first, then 'app' (Follow app), then the six accents.
  "orb colour: auto first, follow app second, then the six accents, in accent order":
    () => {
      assert.deepEqual(ORB_CHOICES, ["auto", "app", ...ACCENT_IDS]);
      assert.equal(ORB_CHOICES.length, 8);
    },
  "orb colour: option ids are unique and follow has its own id": () => {
    const ids = ORB_CHOICES.map(orbChoiceId);
    assert.equal(new Set(ids).size, ids.length);
    assert.equal(orbChoiceId("app"), ORB_FOLLOW_ID);
    assert.equal(ids[0], "auto");
    assert.ok(!ACCENT_IDS.includes(ORB_FOLLOW_ID as never));
  },
  "orb colour: selection maps auto to the first option and each accent to its own":
    () => {
      assert.equal(orbSelectedIndex("auto"), 0);
      assert.equal(orbSelectedIndex("amber"), 2);
      assert.equal(orbSelectedIndex("slate"), 7);
    },
  "orb colour: unknown or absent stored values read as auto": () => {
    assert.equal(effectiveOrbAccent(null), "auto");
    assert.equal(effectiveOrbAccent(undefined), "auto");
    assert.equal(effectiveOrbAccent("magenta"), "auto");
    assert.equal(effectiveOrbAccent(42), "auto");
    assert.equal(effectiveOrbAccent("rose"), "rose");
    assert.equal(effectiveOrbAccent("app"), "app");
    assert.equal(orbSelectedIndex("magenta" as never), 0);
  },
  "orb colour: names are Auto, Follow app and the accent names": () => {
    // RE-POINTED (A5): null was Follow app; 'app' is Follow app now.
    assert.equal(orbChoiceNameKey("app"), "settings.appearance.orb.followApp");
    assert.equal(
      orbChoiceNameKey("blue"),
      "settings.appearance.accent.swatch.blue",
    );
    const names = ORB_CHOICES.map((choice) =>
      orbChoiceNameKey(choice)
        .split(".")
        .reduce((n, p) => n[p], en),
    );
    assert.deepEqual(names, [
      "Auto (changes with what Vox is doing)",
      "Follow app colour",
      "Amber",
      "Blue",
      "Violet",
      "Emerald",
      "Rose",
      "Slate",
    ]);
  },
  "orb colour: the copy says the orb starts a conversation and points to Appearance":
    () => {
      const floating = en.settings.floatingOrb;
      assert.match(floating.description, /start or stop a voice conversation/);
      assert.doesNotMatch(floating.description, /open the conversation/);
      assert.match(floating.colourHint, /Appearance/);
      assert.match(floating.colourHint, /Orb colour/);
      assert.equal(en.settings.appearance.orb.title, "Orb colour");
    },
  "effective values: absent or garbage settings read as the defaults": () => {
    assert.deepEqual(
      effectiveAppearance({
        theme: undefined,
        accent: undefined,
        chat_theme: undefined,
        visual_style: undefined,
      }),
      {
        theme: "system",
        accent: "amber",
        chatTheme: "dark",
        visualStyle: "vibrant",
      },
    );
    assert.deepEqual(
      effectiveAppearance({
        theme: "aurora",
        accent: "rose",
        chat_theme: "light",
        visual_style: "restrained",
      }),
      {
        theme: "aurora",
        accent: "rose",
        chatTheme: "light",
        visualStyle: "restrained",
      },
    );
    assert.deepEqual(
      effectiveAppearance({
        theme: "neon" as never,
        accent: "magenta" as never,
        chat_theme: "pink" as never,
        visual_style: "loud" as never,
      }),
      {
        theme: "system",
        accent: "amber",
        chatTheme: "dark",
        visualStyle: "vibrant",
      },
    );
  },
  "old search ids resolve to the Appearance page ids; others pass through":
    () => {
      assert.equal(resolveSearchEntryId("general.theme"), "appearance.theme");
      assert.equal(
        resolveSearchEntryId("general.customThemeText"),
        "appearance.customThemeText",
      );
      assert.equal(
        resolveSearchEntryId("general.chatOpacity"),
        "appearance.chatOpacity",
      );
      assert.equal(
        resolveSearchEntryId("general.cancelShortcut"),
        "general.cancelShortcut",
      );
      assert.equal(
        resolveSearchEntryId("appearance.accent"),
        "appearance.accent",
      );
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
if (failed > 0) throw new Error(`${failed} appearance model scenarios failed`);
console.log(
  `appearance model: ${Object.keys(scenarios).length} scenarios passed`,
);
