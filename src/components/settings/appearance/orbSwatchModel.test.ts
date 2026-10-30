import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ACCENT_IDS } from "@/styles/accents";
import { contrastRatio } from "@/styles/wcag";
import { GLOBE_STOPS } from "@/orb/globePalette";
import { ORB_CHOICES, orbChoiceNameKey } from "./appearanceModel";
import {
  AUTO_QUARTERS,
  CHECK_DARK,
  CHECK_LIGHT,
  checkColourFor,
  orbSwatchSpec,
  sphereGradientId,
} from "./orbSwatchModel";

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (relative: string): string =>
  readFileSync(path.resolve(here, relative), "utf8");

const scenarios: Record<string, () => void> = {
  "the swatch table covers all eight orb options, in order": () => {
    assert.deepEqual(
      [...ORB_CHOICES],
      ["auto", "app", "amber", "blue", "violet", "emerald", "rose", "slate"],
    );
    for (const choice of ORB_CHOICES) {
      const spec = orbSwatchSpec(choice);
      if (choice === "auto") assert.equal(spec.kind, "auto");
      else if (choice === "app") assert.equal(spec.kind, "app");
      else assert.equal(spec.kind, "sphere");
    }
  },
  "each option has a unique name key; Auto and Follow app keys exist": () => {
    const keys = ORB_CHOICES.map(orbChoiceNameKey);
    assert.equal(new Set(keys).size, keys.length);
    assert.equal(orbChoiceNameKey("auto"), "settings.appearance.orb.auto");
    assert.equal(orbChoiceNameKey("app"), "settings.appearance.orb.followApp");
  },
  "auto shows the four state colours as quarters": () => {
    assert.equal(AUTO_QUARTERS.length, 4);
    assert.equal(new Set(AUTO_QUARTERS).size, 4);
    assert.deepEqual(orbSwatchSpec("auto"), {
      kind: "auto",
      quarters: AUTO_QUARTERS,
      check: CHECK_DARK,
    });
  },
  "every sphere swatch uses its accent's globe stops": () => {
    for (const accent of ACCENT_IDS) {
      const spec = orbSwatchSpec(accent);
      assert.equal(spec.kind, "sphere");
      if (spec.kind === "sphere") {
        assert.deepEqual(spec.stops, GLOBE_STOPS[accent]);
      }
    }
  },
  "the selected check has >= 3:1 against the mid and light stop of every sphere":
    () => {
      for (const accent of ACCENT_IDS) {
        const stops = GLOBE_STOPS[accent];
        const check = checkColourFor(stops);
        assert.ok([CHECK_DARK, CHECK_LIGHT].includes(check));
        assert.ok(
          contrastRatio(check, stops.mid) >= 3,
          `${accent} check vs mid ${contrastRatio(check, stops.mid).toFixed(2)}`,
        );
        assert.ok(
          contrastRatio(check, stops.hi) >= 3,
          `${accent} check vs hi ${contrastRatio(check, stops.hi).toFixed(2)}`,
        );
      }
    },
  "the Auto check has >= 3:1 against every quarter": () => {
    for (const quarter of AUTO_QUARTERS) {
      assert.ok(contrastRatio(CHECK_DARK, quarter) >= 3);
    }
  },
  "gradient ids are unique per instance and safe in url(#...)": () => {
    const ids = [":r0:", ":r1:", ":r2:", ":r10:"].map(sphereGradientId);
    assert.equal(new Set(ids).size, ids.length);
    for (const id of ids) assert.match(id, /^[A-Za-z0-9_-]+$/);
  },
  "the component never uses a fixed gradient id": () => {
    const source = read("OrbColorSetting.tsx");
    assert.match(source, /useId\(\)/);
    assert.doesNotMatch(source, /id="orb-sphere/);
    assert.doesNotMatch(source, /id=\{?"?orb-sphere-?"?\}?/);
  },
};

for (const [name, run] of Object.entries(scenarios)) {
  run();
  console.log(`ok - ${name}`);
}
console.log(
  `orbSwatchModel.test.ts: ${Object.keys(scenarios).length} scenarios passed`,
);
