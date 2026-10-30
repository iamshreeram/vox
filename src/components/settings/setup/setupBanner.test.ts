import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { setupBannerModel, type BannerInput } from "./setupBanner";

// UI enterprise S4a (U6): ONE setup-status banner. General and Models show the same component;
// it lists every missing prerequisite once, with one primary action and one secondary action.

const here = path.dirname(fileURLToPath(import.meta.url));
const settingsDir = path.join(here, "..");
const read = (file: string): string =>
  readFileSync(path.join(settingsDir, file), "utf8");

const inherit = (
  feature: BannerInput["inherit"][number]["feature"],
  state: BannerInput["inherit"][number]["state"] = "ready",
): BannerInput["inherit"][number] => ({ feature, state });

const scenarios: Record<string, () => void> = {
  "General lists each inheriting feature once, with Open Brain and Use own setup":
    () => {
      const model = setupBannerModel({
        page: "general",
        inherit: [
          inherit("composer", "needs_setup"),
          inherit("composer", "needs_setup"),
          inherit("realtime_stream"),
        ],
        brainUnset: false,
      });
      assert.equal(model.visible, true);
      assert.deepEqual(
        model.items.map((item) => item.kind === "feature" && item.feature),
        ["composer", "realtime_stream"],
      );
      assert.equal(model.primary, "open_brain");
      assert.equal(model.secondary, "use_own");
    },

  "General with nothing inheriting shows no banner": () => {
    const model = setupBannerModel({
      page: "general",
      inherit: [],
      brainUnset: false,
    });
    assert.equal(model.visible, false);
    assert.deepEqual(model.items, []);
  },

  "Models shows the Brain-unset prerequisite once with Open Brain and Dismiss":
    () => {
      const model = setupBannerModel({
        page: "models",
        inherit: [],
        brainUnset: true,
      });
      assert.equal(model.visible, true);
      assert.deepEqual(model.items, [{ kind: "brain_unset" }]);
      assert.equal(model.primary, "open_brain");
      assert.equal(model.secondary, "dismiss");
    },

  "Models with the Brain connected shows no banner": () => {
    const model = setupBannerModel({
      page: "models",
      inherit: [],
      brainUnset: false,
    });
    assert.equal(model.visible, false);
  },

  "the same primary and secondary action shape on General and Models": () => {
    const general = setupBannerModel({
      page: "general",
      inherit: [inherit("composer", "needs_consent")],
      brainUnset: true,
    });
    const models = setupBannerModel({
      page: "models",
      inherit: [],
      brainUnset: true,
    });
    for (const model of [general, models]) {
      assert.equal(model.primary, "open_brain");
      assert.ok(model.secondary === "use_own" || model.secondary === "dismiss");
    }
  },

  "General and Models render the one shared banner and no second banner":
    () => {
      const general = read("general/GeneralSettings.tsx");
      const models = read("models/ModelsSettings.tsx");
      assert.match(general, /SetupStatusBanner/);
      assert.match(models, /SetupStatusBanner/);
      assert.doesNotMatch(
        general,
        /<BrainNotice/,
        "General has no second banner",
      );
      assert.doesNotMatch(
        models,
        /FinishSetupCallout/,
        "Models has no second banner",
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
if (failed > 0) throw new Error(`${failed} setup banner scenarios failed`);
console.log(`setup banner: ${Object.keys(scenarios).length} scenarios passed`);
