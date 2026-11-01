import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  getMockCalls,
  installTauriMock,
  MOCK_ENGLISH_ONLY_MODEL,
  MOCK_MULTILINGUAL_MODEL,
  type MockSettings,
} from "./fixtures/tauriMock";

/*
 * Automatic language detection (opt-in, experimental) row in the Models settings, beside the
 * Tamil speech recognition row. Status comes from the `auto_language_status` command.
 */

const MODELS = [MOCK_ENGLISH_ONLY_MODEL, MOCK_MULTILINGUAL_MODEL];
const TITLE = "Automatic language detection";
const DESCRIPTION_START =
  "When the speech model is English-only, Vox can run a second multilingual Whisper model";

const autoRow = (page: Page): Locator =>
  page.getByRole("checkbox", { name: TITLE });
const tamilRow = (page: Page): Locator =>
  page.getByRole("checkbox", { name: "Tamil speech recognition" });
const autoTitle = (page: Page): Locator =>
  page.getByRole("heading", { name: TITLE, exact: true });
const tamilTitle = (page: Page): Locator =>
  page.getByRole("heading", { name: "Tamil speech recognition", exact: true });
const statusLine = (page: Page): Locator =>
  page.locator('[data-testid="auto-language-status"]');

async function openModels(page: Page): Promise<void> {
  await page.goto("/");
  await page.locator("p", { hasText: /^Models$/ }).click();
  await expect(page.locator('[data-settings-section="models"]')).toBeVisible();
}

function baseOptions(
  settings: MockSettings = {},
  autoStatus: unknown = {
    enabled: false,
    lid_model_id: null,
    lid_model_name: null,
    lid_loaded: false,
    tamil_pack_ready: false,
  },
): Parameters<typeof installTauriMock>[1] {
  return {
    settings: {
      selected_language: "auto",
      stt_tamil_enabled: true,
      ...settings,
    },
    commands: {
      get_available_models: MODELS,
      get_current_model: MOCK_ENGLISH_ONLY_MODEL.id,
      auto_language_status: autoStatus,
    },
  };
}

test.describe("Automatic language detection row", () => {
  test("row is visible with title, Experimental badge and description, and OFF by default", async ({
    page,
  }) => {
    await installTauriMock(page, baseOptions());
    await openModels(page);

    await expect(autoTitle(page)).toBeVisible();
    const row = page.locator(`[data-setting-title="${TITLE}"]`);
    await expect(row.getByText("Experimental", { exact: true })).toBeVisible();
    // Description is shown in the tooltip of the info button, like the Tamil row.
    await row.getByRole("button", { name: "More information" }).hover();
    await expect(page.getByText(DESCRIPTION_START)).toBeVisible();
    // The disclosure must state the cost and the limits, not only the benefit.
    const tip = page.getByText(DESCRIPTION_START);
    await expect(tip).toContainText("about 2 seconds");
    await expect(tip).toContainText("about 0.8 seconds");
    await expect(tip).toContainText(
      "ordinary English dictations are not slowed down",
    );
    await expect(tip).toContainText(
      "One- or two-word replies are not detected",
    );
    await expect(tip).toContainText("Everything stays on this device");
    await expect(tip).toContainText("Live streaming dictation is unchanged");
    await expect(tip).toContainText(
      "returns nothing for audio that clearly contains speech",
    );
    await expect(tip).toContainText(
      "background noise can occasionally be taken",
    );
    await expect(autoRow(page)).not.toBeChecked();
    await expect(statusLine(page)).toHaveCount(0);
  });

  test("toggling calls change_stt_auto_language_enabled_setting with enabled=true and the switch stays on", async ({
    page,
  }) => {
    await installTauriMock(page, baseOptions());
    await openModels(page);

    await page.locator("label", { has: autoRow(page) }).click();
    await expect(autoRow(page)).toBeChecked();

    const calls = await getMockCalls(
      page,
      "change_stt_auto_language_enabled_setting",
    );
    expect(calls.at(-1)).toEqual({ enabled: true });
  });

  test("enabled with no eligible model shows the install-a-multilingual-model line", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions({ stt_auto_language_enabled: true }),
    );
    await openModels(page);

    await expect(statusLine(page)).toHaveText(
      "Needs a multilingual Whisper model (for example Turbo). Download one under Models.",
    );
  });

  test("enabled with a loaded model shows its name and (loaded)", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(
        { stt_auto_language_enabled: true },
        {
          enabled: true,
          lid_model_id: "whisper-turbo",
          lid_model_name: "Whisper Turbo",
          lid_loaded: true,
          tamil_pack_ready: false,
        },
      ),
    );
    await openModels(page);

    await expect(statusLine(page)).toHaveText(
      "Language check model: Whisper Turbo (loaded)",
    );
  });

  test("a model that is available but not loaded shows its name without (loaded)", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(
        { stt_auto_language_enabled: true },
        {
          enabled: true,
          lid_model_id: "whisper-turbo",
          lid_model_name: "Whisper Turbo",
          lid_loaded: false,
          tamil_pack_ready: false,
        },
      ),
    );
    await openModels(page);

    await expect(statusLine(page)).toHaveText(
      "Language check model: Whisper Turbo",
    );
  });

  test("the status line is absent while the switch is OFF", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(
        { stt_auto_language_enabled: false },
        {
          enabled: false,
          lid_model_id: "whisper-turbo",
          lid_model_name: "Whisper Turbo",
          lid_loaded: true,
          tamil_pack_ready: true,
        },
      ),
    );
    await openModels(page);

    await expect(autoTitle(page)).toBeVisible();
    await expect(autoRow(page)).not.toBeChecked();
    await expect(statusLine(page)).toHaveCount(0);
  });

  test("a rejected status read shows no status line and the row still works", async ({
    page,
  }) => {
    await installTauriMock(page, {
      ...baseOptions({ stt_auto_language_enabled: true }),
      commands: {
        get_available_models: MODELS,
        get_current_model: MOCK_ENGLISH_ONLY_MODEL.id,
        auto_language_status: { __error__: "status unavailable" },
      },
    });
    await openModels(page);

    await expect(autoTitle(page)).toBeVisible();
    await expect(autoRow(page)).toBeChecked();
    await expect(statusLine(page)).toHaveCount(0);
  });

  test("the title left edge matches the Tamil speech row within 1 px", async ({
    page,
  }) => {
    await installTauriMock(page, baseOptions());
    await openModels(page);

    await expect(autoTitle(page)).toBeVisible();
    await expect(tamilTitle(page)).toBeVisible();
    const autoBox = await autoTitle(page).boundingBox();
    const tamilBox = await tamilTitle(page).boundingBox();
    expect(autoBox).not.toBeNull();
    expect(tamilBox).not.toBeNull();
    expect(
      Math.abs((autoBox?.x ?? 0) - (tamilBox?.x ?? 0)),
    ).toBeLessThanOrEqual(1);
  });

  test("the status line left edge matches the Tamil status line within 1 px", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(
        { stt_auto_language_enabled: true },
        {
          enabled: true,
          lid_model_id: "whisper-turbo",
          lid_model_name: "Whisper Turbo",
          lid_loaded: true,
          tamil_pack_ready: false,
        },
      ),
    );
    await openModels(page);

    const autoLine = await statusLine(page).boundingBox();
    const tamilLine = await page
      .locator('[data-testid="tamil-stt-status-block"]')
      .locator("p")
      .first()
      .boundingBox();
    expect(autoLine).not.toBeNull();
    expect(tamilLine).not.toBeNull();
    expect(
      Math.abs((autoLine?.x ?? 0) - (tamilLine?.x ?? 0)),
    ).toBeLessThanOrEqual(1);
  });
});
