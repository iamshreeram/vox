import assert from "node:assert/strict";
import type { TtsEngine, TtsEngineInfo, TtsRuntime } from "../../bindings";
import {
  buildEngineCards,
  canSelect,
  nextSelectionOnLoad,
} from "./ttsEngineState";
import { formatMegabytes } from "./naturalVoiceState";

// Contract under test (DESIGN.md Rev 4, section 3 P6). The worker implements
// ttsEngineState.ts to satisfy this file; the shapes below are what this test assumes.

type EngineInstall =
  | { phase: "not_installed" }
  | { phase: "downloading"; done: number; total: number }
  | { phase: "verifying" }
  | { phase: "ready" }
  | {
      phase: "error";
      kind: "corrupt" | "proxy" | "disk" | "blocked" | "generic";
    };

type EngineBadge =
  | { kind: "recommended" }
  | { kind: "runtime"; runtime: TtsRuntime }
  | { kind: "english_only" }
  | { kind: "size"; sizeBytes: number; label: string }
  | { kind: "license"; license: string };

type EngineCard = {
  id: TtsEngine;
  selected: boolean;
  selectable: boolean;
  disabledReason: "unsupported_platform" | null;
  badges: EngineBadge[];
  primaryAction: "install" | "cancel" | "delete" | null;
  showImportFromFolder: boolean;
  usingSystemVoiceNote: boolean;
  showVoiceDropdown: boolean;
  showRateSlider: boolean;
  showRateNote: boolean;
  previewEnabled: boolean;
  error: { kind: string; showImportHint: boolean } | null;
  progress: number | null;
};

const PHONON_SIZE_BYTES = 64_500_000;

const engine = (
  over: Partial<TtsEngineInfo> & { id: TtsEngine },
): TtsEngineInfo => ({
  recommended: false,
  runtime: "operating_system",
  requires_download: false,
  size_bytes: 0,
  english_only: false,
  supports_voice_choice: false,
  supports_rate: false,
  supported_on_platform: true,
  license: "",
  ...over,
});

const SYSTEM = engine({
  id: "system",
  runtime: "operating_system",
  supports_voice_choice: true,
  supports_rate: true,
  license: "OS-provided",
});

const PHONON = engine({
  id: "phonon",
  recommended: true,
  runtime: "pure_rust",
  requires_download: true,
  size_bytes: PHONON_SIZE_BYTES,
  english_only: true,
  supports_voice_choice: false,
  supports_rate: false,
  supported_on_platform: true,
  license: "Apache-2.0",
});

const NATURAL = engine({
  id: "natural",
  runtime: "onnx_runtime",
  requires_download: true,
  size_bytes: 120_673_052,
  supports_voice_choice: true,
  supports_rate: true,
  supported_on_platform: true,
  license: "Apache-2.0",
});

// Catalog order deliberately differs from the id order to prove nothing is sorted.
const MAC_CATALOG: TtsEngineInfo[] = [PHONON, SYSTEM, NATURAL];

// Mocked non-macOS catalog: the local engines are unsupported here.
const OTHER_PLATFORM_CATALOG: TtsEngineInfo[] = [
  SYSTEM,
  { ...PHONON, supported_on_platform: false },
  { ...NATURAL, supported_on_platform: false },
];

const card = (cards: EngineCard[], id: TtsEngine): EngineCard => {
  const found = cards.find((c) => c.id === id);
  assert.ok(found, `no card for ${id}`);
  return found;
};

const cardsFor = (
  catalog: TtsEngineInfo[],
  selected: TtsEngine,
  installs: Partial<Record<TtsEngine, EngineInstall>> = {},
): EngineCard[] =>
  buildEngineCards(catalog, selected, installs) as EngineCard[];

const assertProgress = (value: number | null) => {
  assert.ok(
    value === null || (Number.isFinite(value) && value >= 0 && value <= 1),
    `progress must be null or within [0,1], got ${String(value)}`,
  );
};

const scenarios: Record<string, () => void> = {
  "catalog order is preserved, one card per entry": () => {
    const cards = cardsFor(MAC_CATALOG, "system");
    assert.deepEqual(
      cards.map((c) => c.id),
      ["phonon", "system", "natural"],
    );
  },
  "an empty catalog renders no cards": () => {
    assert.deepEqual(cardsFor([], "system"), []);
  },
  "selected is true only for the persisted engine": () => {
    const cards = cardsFor(MAC_CATALOG, "natural");
    assert.equal(cards.filter((c) => c.selected).length, 1);
    assert.equal(card(cards, "natural").selected, true);
    assert.equal(card(cards, "phonon").selected, false);
    assert.equal(card(cards, "system").selected, false);
  },
  "exactly one Recommended badge, on a supported engine": () => {
    const cards = cardsFor(MAC_CATALOG, "system");
    const recommended = cards.filter((c) =>
      c.badges.some((b) => b.kind === "recommended"),
    );
    assert.equal(recommended.length, 1);
    assert.equal(recommended[0].id, "phonon");
  },
  "no Recommended badge anywhere on a non-macOS catalog": () => {
    const cards = cardsFor(OTHER_PLATFORM_CATALOG, "system");
    for (const c of cards) {
      assert.equal(
        c.badges.some((b) => b.kind === "recommended"),
        false,
        `${c.id} must not be recommended`,
      );
    }
  },
  "unsupported Phonon is disabled with a reason and no actions": () => {
    const cards = cardsFor(OTHER_PLATFORM_CATALOG, "system");
    const phonon = card(cards, "phonon");
    assert.equal(phonon.selectable, false);
    assert.equal(phonon.disabledReason, "unsupported_platform");
    assert.equal(phonon.primaryAction, null);
    assert.equal(phonon.showImportFromFolder, false);
    assert.equal(phonon.previewEnabled, false);
    assert.equal(phonon.error, null);
    assert.equal(phonon.progress, null);
    assert.equal(card(cards, "system").disabledReason, null);
    assert.equal(card(cards, "system").selectable, true);
  },
  "unsupported Phonon gets no actions even with an install state": () => {
    const cards = cardsFor(OTHER_PLATFORM_CATALOG, "system", {
      phonon: { phase: "not_installed" },
    });
    const phonon = card(cards, "phonon");
    assert.equal(phonon.primaryAction, null);
    assert.equal(phonon.showImportFromFolder, false);
    assert.equal(phonon.previewEnabled, false);
  },
  "a persisted unsupported selection stays selected and is never rewritten":
    () => {
      const cards = cardsFor(OTHER_PLATFORM_CATALOG, "phonon");
      assert.equal(card(cards, "phonon").selected, true);
      assert.equal(
        nextSelectionOnLoad(OTHER_PLATFORM_CATALOG, "phonon"),
        "phonon",
      );
      assert.equal(
        nextSelectionOnLoad(OTHER_PLATFORM_CATALOG, "natural"),
        "natural",
      );
      assert.equal(
        nextSelectionOnLoad(OTHER_PLATFORM_CATALOG, "system"),
        "system",
      );
    },
  "nextSelectionOnLoad always returns the persisted engine": () => {
    for (const id of ["phonon", "natural", "system"] as TtsEngine[]) {
      assert.equal(nextSelectionOnLoad(MAC_CATALOG, id), id);
      assert.equal(nextSelectionOnLoad(OTHER_PLATFORM_CATALOG, id), id);
    }
  },
  "canSelect follows supported_on_platform, not the model state": () => {
    assert.equal(canSelect(MAC_CATALOG, "phonon"), true);
    assert.equal(canSelect(MAC_CATALOG, "natural"), true);
    assert.equal(canSelect(MAC_CATALOG, "system"), true);
    assert.equal(canSelect(OTHER_PLATFORM_CATALOG, "phonon"), false);
    assert.equal(canSelect(OTHER_PLATFORM_CATALOG, "natural"), false);
    assert.equal(canSelect(OTHER_PLATFORM_CATALOG, "system"), true);
  },
  "a missing model does not block selecting a supported engine": () => {
    const cards = cardsFor(MAC_CATALOG, "system", {
      phonon: { phase: "not_installed" },
    });
    assert.equal(card(cards, "phonon").selectable, true);
    assert.equal(card(cards, "phonon").disabledReason, null);
  },
  "the system engine never has a model, install or import": () => {
    const cards = cardsFor(MAC_CATALOG, "system");
    const system = card(cards, "system");
    assert.equal(system.primaryAction, null);
    assert.equal(system.showImportFromFolder, false);
    assert.equal(system.usingSystemVoiceNote, false);
    assert.equal(system.progress, null);
    assert.equal(system.error, null);
    assert.equal(system.previewEnabled, true);
  },
  "system voice dropdown and rate slider follow the catalog flags": () => {
    const on = card(cardsFor(MAC_CATALOG, "system"), "system");
    assert.equal(on.showVoiceDropdown, true);
    assert.equal(on.showRateSlider, true);
    assert.equal(on.showRateNote, false);

    const off = card(
      cardsFor(
        [
          engine({
            id: "system",
            supports_voice_choice: false,
            supports_rate: false,
          }),
        ],
        "system",
      ),
      "system",
    );
    assert.equal(off.showVoiceDropdown, false);
    assert.equal(off.showRateSlider, false);
    assert.equal(off.showRateNote, true);
  },
  "Phonon has no voice dropdown, hides the rate slider and shows the rate note":
    () => {
      const phonon = card(cardsFor(MAC_CATALOG, "phonon"), "phonon");
      assert.equal(phonon.showVoiceDropdown, false);
      assert.equal(phonon.showRateSlider, false);
      assert.equal(phonon.showRateNote, true);
    },
  "Kokoro (natural) shows the voice dropdown and the rate slider": () => {
    const natural = card(cardsFor(MAC_CATALOG, "natural"), "natural");
    assert.equal(natural.showVoiceDropdown, true);
    assert.equal(natural.showRateSlider, true);
    assert.equal(natural.showRateNote, false);
  },
  "badges come from the catalog in the documented order": () => {
    const phonon = card(cardsFor(MAC_CATALOG, "system"), "phonon");
    assert.deepEqual(
      phonon.badges.map((b) => b.kind),
      ["recommended", "runtime", "english_only", "size", "license"],
    );
    assert.deepEqual(phonon.badges[1], {
      kind: "runtime",
      runtime: "pure_rust",
    });
    assert.deepEqual(phonon.badges[4], {
      kind: "license",
      license: "Apache-2.0",
    });

    const size = phonon.badges[3];
    assert.equal(size.kind, "size");
    if (size.kind === "size") {
      assert.equal(size.sizeBytes, PHONON_SIZE_BYTES);
      assert.equal(size.label, formatMegabytes(PHONON_SIZE_BYTES));
      assert.equal(size.label, "62 MB");
    }
  },
  "runtime badge carries the raw catalog runtime for each engine": () => {
    const cards = cardsFor(MAC_CATALOG, "system");
    assert.deepEqual(
      card(cards, "system").badges.find((b) => b.kind === "runtime"),
      { kind: "runtime", runtime: "operating_system" },
    );
    assert.deepEqual(
      card(cards, "natural").badges.find((b) => b.kind === "runtime"),
      { kind: "runtime", runtime: "onnx_runtime" },
    );
  },
  "english-only badge only when the catalog says so": () => {
    const cards = cardsFor(MAC_CATALOG, "system");
    assert.equal(
      card(cards, "phonon").badges.some((b) => b.kind === "english_only"),
      true,
    );
    assert.equal(
      card(cards, "natural").badges.some((b) => b.kind === "english_only"),
      false,
    );
  },
  "a not-installed Phonon card offers install, import and preview": () => {
    const phonon = card(
      cardsFor(MAC_CATALOG, "phonon", { phonon: { phase: "not_installed" } }),
      "phonon",
    );
    assert.equal(phonon.primaryAction, "install");
    assert.equal(phonon.showImportFromFolder, true);
    assert.equal(phonon.previewEnabled, true);
    assert.equal(phonon.progress, null);
    assert.equal(phonon.error, null);
    assert.equal(phonon.usingSystemVoiceNote, true);
  },
  "the using-system-voice note only while selected and not ready": () => {
    const notSelected = card(
      cardsFor(MAC_CATALOG, "system", { phonon: { phase: "not_installed" } }),
      "phonon",
    );
    assert.equal(notSelected.usingSystemVoiceNote, false);

    const selectedReady = card(
      cardsFor(MAC_CATALOG, "phonon", { phonon: { phase: "ready" } }),
      "phonon",
    );
    assert.equal(selectedReady.usingSystemVoiceNote, false);

    for (const install of [
      { phase: "not_installed" },
      { phase: "downloading", done: 1, total: 2 },
      { phase: "verifying" },
      { phase: "error", kind: "corrupt" },
    ] as EngineInstall[]) {
      const c = card(
        cardsFor(MAC_CATALOG, "natural", { natural: install }),
        "natural",
      );
      assert.equal(c.usingSystemVoiceNote, true, install.phase);
    }
  },
  "install states drive the primary action": () => {
    const action = (install: EngineInstall) =>
      card(cardsFor(MAC_CATALOG, "system", { natural: install }), "natural")
        .primaryAction;
    assert.equal(action({ phase: "not_installed" }), "install");
    assert.equal(
      action({ phase: "downloading", done: 5, total: 10 }),
      "cancel",
    );
    assert.equal(action({ phase: "verifying" }), null);
    assert.equal(action({ phase: "ready" }), "delete");
    assert.equal(action({ phase: "error", kind: "corrupt" }), "install");
  },
  "downloading progress is done/total": () => {
    const progress = (done: number, total: number) =>
      card(
        cardsFor(MAC_CATALOG, "system", {
          natural: { phase: "downloading", done, total },
        }),
        "natural",
      ).progress;
    assert.equal(progress(5, 10), 0.5);
    assert.equal(progress(10, 10), 1);
    assert.equal(progress(0, 10), 0);
  },
  "progress never exceeds 1 and never becomes NaN or Infinity": () => {
    const progress = (done: number, total: number) =>
      card(
        cardsFor(MAC_CATALOG, "system", {
          natural: { phase: "downloading", done, total },
        }),
        "natural",
      ).progress;
    assert.equal(progress(15, 10), 1);
    assert.equal(progress(-5, 10), 0);
    assert.equal(progress(5, 0), null);
    assert.equal(progress(0, 0), null);
    assert.equal(progress(5, -1), null);
    assertProgress(progress(Number.NaN, 10));
    assertProgress(progress(5, Number.NaN));
    assertProgress(progress(5, Number.POSITIVE_INFINITY));
    assertProgress(progress(Number.POSITIVE_INFINITY, 10));
  },
  "progress is null outside the downloading phase": () => {
    for (const install of [
      { phase: "not_installed" },
      { phase: "verifying" },
      { phase: "ready" },
      { phase: "error", kind: "generic" },
    ] as EngineInstall[]) {
      const c = card(
        cardsFor(MAC_CATALOG, "system", { natural: install }),
        "natural",
      );
      assert.equal(c.progress, null, install.phase);
    }
  },
  "error kinds: import hint for corrupt, proxy, blocked and disk only": () => {
    for (const kind of ["corrupt", "proxy", "blocked", "disk"] as const) {
      const c = card(
        cardsFor(MAC_CATALOG, "phonon", {
          phonon: { phase: "error", kind },
        }),
        "phonon",
      );
      assert.deepEqual(c.error, { kind, showImportHint: true }, kind);
      assert.equal(c.primaryAction, "install", kind);
      assert.equal(c.showImportFromFolder, true, kind);
    }
    const generic = card(
      cardsFor(MAC_CATALOG, "phonon", {
        phonon: { phase: "error", kind: "generic" },
      }),
      "phonon",
    );
    assert.deepEqual(generic.error, { kind: "generic", showImportHint: false });
    assert.equal(generic.primaryAction, "install");
  },
  "an unknown install state behaves as not installed": () => {
    const missing = cardsFor(MAC_CATALOG, "phonon", {});
    const unknown = cardsFor(MAC_CATALOG, "phonon", {
      phonon: undefined,
    });
    assert.deepEqual(unknown, missing);
    assert.equal(card(unknown, "phonon").primaryAction, "install");
    assert.equal(card(unknown, "phonon").progress, null);
  },
  "an unknown engine id in the catalog renders a card without throwing": () => {
    const catalog: TtsEngineInfo[] = [
      SYSTEM,
      engine({ id: "future_engine" as TtsEngine, requires_download: true }),
    ];
    const cards = cardsFor(catalog, "system");
    assert.equal(cards.length, 2);
    assert.equal(card(cards, "future_engine" as TtsEngine).id, "future_engine");
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
