import assert from "node:assert/strict";
import {
  downloadPercent,
  failureI18nKey,
  formatMegabytes,
  naturalRowState,
  selectedEngineNeedsModel,
  type NaturalStatusLike,
} from "./naturalVoiceState";

const status = (over: Partial<NaturalStatusLike> = {}): NaturalStatusLike => ({
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
  ...over,
});

const scenarios: Record<string, () => void> = {
  "percent is floored, clamped and safe for bad input": () => {
    assert.equal(downloadPercent(0, 0), 0);
    assert.equal(downloadPercent(50, 100), 50);
    assert.equal(downloadPercent(1, 3), 33);
    assert.equal(downloadPercent(100, 100), 100);
    assert.equal(downloadPercent(5, 3), 100);
    assert.equal(downloadPercent(-5, 10), 0);
    assert.equal(downloadPercent(10, -10), 0);
    assert.equal(downloadPercent(Number.NaN, 10), 0);
    assert.equal(downloadPercent(10, Number.NaN), 0);
  },
  "no status or an unsupported platform hides the section": () => {
    assert.equal(naturalRowState(null), "hidden");
    assert.equal(naturalRowState(status({ supported: false })), "hidden");
    assert.equal(
      naturalRowState(status({ supported: false, model: "ready" })),
      "hidden",
    );
  },
  "the row follows the model state": () => {
    assert.equal(naturalRowState(status()), "download");
    assert.equal(
      naturalRowState(
        status({ model: "downloading", downloaded: 5, total: 10 }),
      ),
      "downloading",
    );
    for (const model of ["verifying", "installing", "deleting", "loading"]) {
      assert.equal(naturalRowState(status({ model })), "working", model);
    }
    assert.equal(naturalRowState(status({ model: "ready" })), "ready");
  },
  "a failure only matters once the model is installed": () => {
    assert.equal(
      naturalRowState(
        status({ model: "ready", health: "failed", failure: "load" }),
      ),
      "failed",
    );
    assert.equal(
      naturalRowState(
        status({ model: "not_installed", health: "failed", failure: "load" }),
      ),
      "download",
    );
    assert.equal(
      naturalRowState(
        status({ model: "downloading", health: "failed", failure: "sink" }),
      ),
      "downloading",
    );
  },
  "failure codes map to stable i18n keys": () => {
    for (const code of ["load", "synth", "sink", "worker_exit"]) {
      assert.equal(
        failureI18nKey(code),
        `settings.advanced.tts.natural.failure.${code}`,
      );
    }
    assert.equal(
      failureI18nKey(null),
      "settings.advanced.tts.natural.failure.unknown",
    );
    assert.equal(
      failureI18nKey("???"),
      "settings.advanced.tts.natural.failure.unknown",
    );
    assert.equal(
      failureI18nKey(""),
      "settings.advanced.tts.natural.failure.unknown",
    );
  },
  "sizes are shown in whole megabytes": () => {
    assert.equal(formatMegabytes(0), "0 MB");
    assert.equal(formatMegabytes(92_361_271), "88 MB");
    assert.equal(formatMegabytes(28_311_781), "27 MB");
    assert.equal(formatMegabytes(-1), "0 MB");
    assert.equal(formatMegabytes(Number.NaN), "0 MB");
  },
  "the missing-model hint only shows when natural is selected, supported and absent":
    () => {
      assert.equal(selectedEngineNeedsModel("natural", status()), true);
      assert.equal(
        selectedEngineNeedsModel("natural", status({ model: "downloading" })),
        true,
      );
      assert.equal(
        selectedEngineNeedsModel("natural", status({ model: "ready" })),
        false,
      );
      assert.equal(selectedEngineNeedsModel("system", status()), false);
      assert.equal(
        selectedEngineNeedsModel("natural", status({ supported: false })),
        false,
      );
      assert.equal(selectedEngineNeedsModel("natural", null), false);
    },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`not ok - ${name}`);
    console.error(error);
  }
}
if (failed > 0) {
  process.exit(1);
}
