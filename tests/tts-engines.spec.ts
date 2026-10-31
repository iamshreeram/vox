import { mkdirSync } from "node:fs";
import path from "node:path";
import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
  setMockCommand,
  setMockSetting,
} from "./fixtures/tauriMock";

/*
 * Engine picker (Spoken replies, Experimental page). Mounted tests against the Vite dev
 * server with the Tauri mock. Covers: card order and the Recommended badge; selecting Phonon
 * with no model; Preview only on the selected card; install progress then ready (mocked
 * events); the import hint on an error; translated proxy text (no backend text); and a
 * non-macOS catalog that disables Phonon while keeping a persisted selection visible.
 * Screenshots of four states are written only when SHOTS_DIR is set.
 */

const NOT_INSTALLED = {
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
};

const NATURAL_NOT_INSTALLED = { ...NOT_INSTALLED };

const card = (page: Page, engine: string): Locator =>
  page.locator(`[data-testid="tts-engine-card"][data-engine="${engine}"]`);

async function openExperimental(page: Page): Promise<void> {
  await page.goto("/");
  // Reach the Spoken replies card through settings search, as the search spec does: the
  // sidebar click alone does not switch pages in this harness.
  await page
    .getByRole("combobox", { name: /search settings/i })
    .fill("voice engine");
  await page
    .getByRole("option")
    .filter({ hasText: "Voice engine" })
    .first()
    .click();
  await expect(page.locator('[data-testid="tts-engine-picker"]')).toBeVisible();
}

test.describe("Engine picker: cards", () => {
  test("renders three cards in order with Recommended on Phonon", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    const cards = page.locator('[data-testid="tts-engine-card"]');
    await expect(cards).toHaveCount(3);
    await expect(cards.nth(0)).toHaveAttribute("data-engine", "phonon");
    await expect(cards.nth(1)).toHaveAttribute("data-engine", "natural");
    await expect(cards.nth(2)).toHaveAttribute("data-engine", "system");

    await expect(card(page, "phonon").getByText("Recommended")).toBeVisible();
    await expect(card(page, "natural").getByText("Recommended")).toHaveCount(0);
    await expect(card(page, "system").getByText("Recommended")).toHaveCount(0);
  });

  test("selecting Phonon with no model shows the system-voice line", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    // Before selection: no Preview on the unselected Phonon card.
    await expect(
      card(page, "phonon").getByRole("button", { name: "Preview" }),
    ).toHaveCount(0);

    await card(page, "phonon").getByRole("button", { name: "Select" }).click();
    // The backend persists the choice; the mock stands in for that write.
    await setMockSetting(page, "tts_engine", "phonon");

    await expect(card(page, "phonon")).toHaveAttribute("data-selected", "true");
    await expect(
      card(page, "phonon").getByText("Using the system voice until installed"),
    ).toBeVisible();
    // Preview needs an installed model: the selected card without a model has none.
    await expect(
      card(page, "phonon").getByRole("button", { name: "Preview" }),
    ).toHaveCount(0);
    await expect(
      card(page, "natural").getByRole("button", { name: "Preview" }),
    ).toHaveCount(0);
    await expect(
      card(page, "system").getByRole("button", { name: "Preview" }),
    ).toHaveCount(0);
  });
});

test.describe("Engine picker: Phonon model", () => {
  test("install progress, then ready after the mocked events", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        phonon_voice_status: {
          ...NOT_INSTALLED,
          model: "downloading",
          downloaded: 50,
          total: 100,
        },
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    const phonon = card(page, "phonon");
    await expect(phonon.getByText("Downloading… 50%")).toBeVisible();
    await expect(phonon.getByRole("button", { name: "Cancel" })).toBeVisible();
    await expect(phonon.getByRole("button", { name: "Preview" })).toHaveCount(
      0,
    );

    await setMockCommand(page, "phonon_voice_status", {
      ...NOT_INSTALLED,
      model: "ready",
      downloaded: 100,
      total: 100,
    });
    await emitMockEvent(page, "phonon-voice-state", {
      ok: true,
      message: null,
    });

    await expect(phonon.getByText("Installed")).toBeVisible();
    await expect(phonon.getByRole("button", { name: "Delete" })).toBeVisible();
    // Ready and selected: Preview is meaningful now.
    await expect(phonon.getByRole("button", { name: "Preview" })).toBeVisible();
    await expect(
      phonon.getByText("Using the system voice until installed"),
    ).toHaveCount(0);
  });

  test("an error row shows the import hint", async ({ page }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    await emitMockEvent(page, "phonon-voice-error", {
      ok: false,
      message:
        "The model files failed verification. Please download the model again.",
    });

    const phonon = card(page, "phonon");
    await expect(
      phonon.getByText(
        "The voice files did not pass verification. Install the voice again.",
      ),
    ).toBeVisible();
    await expect(
      phonon.getByText(
        "You can also install from a folder that holds the four model files.",
      ),
    ).toBeVisible();
    await expect(
      phonon.getByRole("button", { name: "Install from folder" }),
    ).toBeVisible();
  });

  test("a proxy error shows translated text, never the backend sentence", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    const backendText =
      "The download was blocked by a proxy that needs sign-in. Install the model from a folder instead.";
    await emitMockEvent(page, "phonon-voice-error", {
      ok: false,
      message: backendText,
    });

    const phonon = card(page, "phonon");
    await expect(
      phonon.getByText(
        "The download was blocked by a network proxy that needs sign-in.",
      ),
    ).toBeVisible();
    await expect(
      phonon.getByText(
        "Blocked by a proxy? Download the four files elsewhere and install from a folder.",
      ),
    ).toBeVisible();
    await expect(phonon.getByText(backendText)).toHaveCount(0);
  });
});

test.describe("Engine picker: non-macOS catalog", () => {
  test("disables Phonon and keeps a persisted Phonon selection visible", async ({
    page,
  }) => {
    await installTauriMock(page, {
      platform: "linux",
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        list_tts_engines: OFF_PLATFORM_CATALOG,
        phonon_voice_status: { ...NOT_INSTALLED, supported: false },
        natural_voice_status: { ...NATURAL_NOT_INSTALLED, supported: false },
      },
    });
    await openExperimental(page);

    const phonon = card(page, "phonon");
    await expect(phonon).toHaveAttribute("data-selected", "true");
    await expect(phonon.getByText("Available on macOS only.")).toBeVisible();
    await expect(
      phonon.getByText(
        "Not available on this platform. The system voice is used instead.",
      ),
    ).toBeVisible();
    await expect(
      phonon.getByRole("button", { name: "Selected" }),
    ).toBeDisabled();
    await expect(phonon.getByText("Recommended")).toHaveCount(0);
    await expect(phonon.getByRole("button", { name: "Install" })).toHaveCount(
      0,
    );

    await expect(card(page, "system")).toHaveAttribute(
      "data-selected",
      "false",
    );
    await expect(
      card(page, "system").getByRole("button", { name: "Select" }),
    ).toBeEnabled();
  });
});

const OFF_PLATFORM_CATALOG = [
  {
    id: "phonon",
    recommended: true,
    runtime: "pure_rust",
    requires_download: true,
    size_bytes: 65051089,
    english_only: true,
    supports_voice_choice: false,
    supports_rate: false,
    supported_on_platform: false,
    license: "Apache-2.0",
  },
  {
    id: "natural",
    recommended: false,
    runtime: "onnx_runtime",
    requires_download: true,
    size_bytes: 120673052,
    english_only: false,
    supports_voice_choice: true,
    supports_rate: true,
    supported_on_platform: false,
    license: "Apache-2.0",
  },
  {
    id: "system",
    recommended: false,
    runtime: "operating_system",
    requires_download: false,
    size_bytes: 0,
    english_only: false,
    supports_voice_choice: true,
    supports_rate: true,
    supported_on_platform: true,
    license: "OS-provided",
  },
];

// Screenshots for review (not committed). Runs only when SHOTS_DIR is set.
const SHOTS_DIR = process.env.SHOTS_DIR;

async function shoot(page: Page, name: string): Promise<void> {
  if (!SHOTS_DIR) return;
  mkdirSync(SHOTS_DIR, { recursive: true });
  // The section that holds the title, the description and every engine card.
  const section = page
    .locator('[data-testid="tts-engine-picker"]')
    .locator("xpath=ancestor::*[@data-setting-title][1]");
  await section.scrollIntoViewIfNeeded();
  await section.screenshot({ path: path.join(SHOTS_DIR, name) });
}

test.describe("Engine picker: screenshots", () => {
  test.skip(!SHOTS_DIR, "set SHOTS_DIR to write screenshots");

  test("01-default: catalog order, Phonon not installed, System selected", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);
    await shoot(page, "01-default.png");
  });

  test("02-downloading: Phonon selected, progress about 40%", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        phonon_voice_status: {
          ...NOT_INSTALLED,
          model: "downloading",
          downloaded: 40,
          total: 100,
        },
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);
    await expect(
      card(page, "phonon").getByText("Downloading… 40%"),
    ).toBeVisible();
    await shoot(page, "02-downloading.png");
  });

  test("03-error-proxy: proxy error row with the import hint", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);
    await emitMockEvent(page, "phonon-voice-error", {
      ok: false,
      message:
        "The download was blocked by a proxy that needs sign-in. Install the model from a folder instead.",
    });
    await expect(
      card(page, "phonon").getByRole("button", { name: "Install from folder" }),
    ).toBeVisible();
    await shoot(page, "03-error-proxy.png");
  });

  test("04-unsupported: non-macOS, persisted Phonon selection disabled", async ({
    page,
  }) => {
    await installTauriMock(page, {
      platform: "linux",
      settings: { experimental_enabled: true, tts_engine: "phonon" },
      commands: {
        list_tts_engines: OFF_PLATFORM_CATALOG,
        phonon_voice_status: { ...NOT_INSTALLED, supported: false },
        natural_voice_status: { ...NATURAL_NOT_INSTALLED, supported: false },
      },
    });
    await openExperimental(page);
    await expect(
      card(page, "phonon").getByText("Available on macOS only."),
    ).toBeVisible();
    await shoot(page, "04-unsupported.png");
  });
});

test.describe("Tamil voice row", () => {
  const tamilRow = (page: Page): Locator =>
    page.getByRole("checkbox", { name: "Tamil voice" });

  test("shows the toggle, the not-installed status and the credit", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    await expect(tamilRow(page)).toBeChecked();
    await expect(
      page.locator('[data-testid="tamil-voice-status"]'),
    ).toContainText("Not installed");
    await expect(
      page.getByText(
        "Voice: Pocket TTS Tamil by Prasad Vittaldev, based on Kyutai (CC BY 4.0)",
      ),
    ).toBeVisible();
  });

  test("toggling calls the Tamil setting command", async ({ page }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
      },
    });
    await openExperimental(page);

    // The checkbox is visually hidden; click its visible label (the switch track).
    await page.locator("label", { has: tamilRow(page) }).click();
    await expect
      .poll(async () =>
        (await getMockCalls(page, "change_tts_tamil_enabled_setting")).map(
          (call) => call.enabled,
        ),
      )
      .toEqual([false]);
  });

  test("an engine failure shows the error message", async ({ page }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NATURAL_NOT_INSTALLED,
        tamil_voice_status: {
          installed: true,
          ready: false,
          enabled: true,
          dir: "/tmp/vox-test/tts_models/pocket-tts-tamil-int4",
          missing: [],
          wrong_size: [],
          engine_error: "model file could not be read",
        },
      },
    });
    await openExperimental(page);

    await expect(page.locator('[data-testid="tamil-voice-status"]')).toHaveText(
      "Tamil voice failed to load: model file could not be read",
    );
  });
});
