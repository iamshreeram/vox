import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ACCENT_IDS } from "../../../styles/accents";
import {
  APPEARANCE_DEFAULTS,
  ACCENT_NAME_KEYS,
  accentEnabledFor,
  effectiveAppearance,
  nextRadioIndex,
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
