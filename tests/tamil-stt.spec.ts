import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  getMockCalls,
  installTauriMock,
  MOCK_ENGLISH_ONLY_MODEL,
  MOCK_MULTILINGUAL_MODEL,
  setMockCommand,
  emitMockEvent,
  type MockSettings,
} from "./fixtures/tauriMock";

/*
 * Tamil speech recognition in the speech-to-text language picker and its settings row
 * (LanguageSelector + TamilSttRow). Runs against the Vite dev server with the Tauri mock.
 *
 * The persisted selected_language is never rewritten by the UI when the model changes or the
 * Tamil pack becomes unavailable; the closed picker shows the fallback instead.
 */

const PACK_DIR = "/tmp/vox-test/aux_models/indic_conformer_int8";

const NOT_INSTALLED = {
  installed: false,
  ready: false,
  enabled: true,
  dir: PACK_DIR,
  missing: [
    "encoder.onnx",
    "ctc_decoder.onnx",
    "vocab.json",
    "language_masks.json",
  ],
  wrong_size: null,
  engine_error: null,
};

const READY = {
  installed: true,
  ready: true,
  enabled: true,
  dir: PACK_DIR,
  missing: [],
  wrong_size: null,
  engine_error: null,
};

const MODELS = [MOCK_ENGLISH_ONLY_MODEL, MOCK_MULTILINGUAL_MODEL];

const sttGroup = (page: Page): Locator =>
  page.locator('[data-group-id="sttModels"]');
const pickerButton = (page: Page, name: string): Locator =>
  sttGroup(page).getByRole("button", { name, exact: true });
const tamilOption = (page: Page): Locator =>
  page.getByRole("button", { name: "Tamil", exact: true });
const tamilRow = (page: Page): Locator =>
  page.getByRole("checkbox", { name: "Tamil speech recognition" });
const statusLine = (page: Page): Locator =>
  page.locator('[data-testid="tamil-stt-status"]');

async function openModels(page: Page): Promise<void> {
  await page.goto("/");
  await page.locator("p", { hasText: /^Models$/ }).click();
  await expect(page.locator('[data-settings-section="models"]')).toBeVisible();
}

async function openPicker(page: Page, closedLabel: string): Promise<void> {
  await pickerButton(page, closedLabel).click();
  await expect(page.getByPlaceholder("Search languages...")).toBeVisible();
}

async function closePicker(page: Page): Promise<void> {
  await page.keyboard.press("Escape");
}

async function lastLanguageCall(page: Page): Promise<unknown> {
  const calls = await getMockCalls(page, "change_selected_language_setting");
  return calls.at(-1)?.language;
}

async function focusWindow(page: Page): Promise<void> {
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
}

function baseOptions(
  status: Record<string, unknown>,
  settings: MockSettings = {},
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
      tamil_stt_status: status,
    },
  };
}

test.describe("Tamil speech recognition: language picker", () => {
  test("pack not installed: Tamil is not offered and the row shows the install command", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(NOT_INSTALLED, { selected_language: "en" }),
    );
    await openModels(page);

    await expect(pickerButton(page, "English")).toBeVisible();
    await openPicker(page, "English");
    await expect(tamilOption(page)).toHaveCount(0);
    await closePicker(page);

    await expect(statusLine(page)).toContainText(
      "scripts/download_indic_stt.sh",
    );
    await expect(statusLine(page)).toContainText(PACK_DIR);
  });

  test("pack ready: Tamil is offered, selecting it shows Tamil and saves 'ta'", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(READY, { selected_language: "en" }),
    );
    await openModels(page);

    await openPicker(page, "English");
    await expect(tamilOption(page)).toBeVisible();
    await tamilOption(page).click();

    await expect(pickerButton(page, "Tamil")).toBeVisible();
    await expect(
      sttGroup(page).getByText(/recognised by the Tamil speech pack/),
    ).toBeVisible();
    expect(await lastLanguageCall(page)).toBe("ta");
  });

  test("switching the model away and back keeps 'ta' and the Tamil display", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(READY, { selected_language: "en" }),
    );
    await openModels(page);
    await openPicker(page, "English");
    await tamilOption(page).click();
    await expect(pickerButton(page, "Tamil")).toBeVisible();
    const changesBefore = (
      await getMockCalls(page, "change_selected_language_setting")
    ).length;

    await setMockCommand(page, "get_current_model", MOCK_MULTILINGUAL_MODEL.id);
    await emitMockEvent(page, "model-state-changed", {});
    await expect(pickerButton(page, "Tamil")).toBeVisible();

    await setMockCommand(page, "get_current_model", MOCK_ENGLISH_ONLY_MODEL.id);
    await emitMockEvent(page, "model-state-changed", {});
    await expect(pickerButton(page, "Tamil")).toBeVisible();

    expect(await lastLanguageCall(page)).toBe("ta");
    expect(
      (await getMockCalls(page, "change_selected_language_setting")).length,
    ).toBe(changesBefore);
  });

  test("pack becomes unavailable: fallback is shown, the saved 'ta' is not rewritten", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(READY, { selected_language: "en" }),
    );
    await openModels(page);
    await openPicker(page, "English");
    await tamilOption(page).click();
    await expect(pickerButton(page, "Tamil")).toBeVisible();
    const changesBefore = (
      await getMockCalls(page, "change_selected_language_setting")
    ).length;

    await setMockCommand(page, "tamil_stt_status", NOT_INSTALLED);
    await focusWindow(page);

    await expect(pickerButton(page, "English")).toBeVisible();
    await expect(statusLine(page)).toContainText(
      "scripts/download_indic_stt.sh",
    );
    expect(
      (await getMockCalls(page, "change_selected_language_setting")).length,
    ).toBe(changesBefore);
    expect(await lastLanguageCall(page)).toBe("ta");
  });

  test("turning the Tamil switch off hides the option and saves enabled=false", async ({
    page,
  }) => {
    await installTauriMock(
      page,
      baseOptions(READY, { selected_language: "en" }),
    );
    await openModels(page);

    await expect(tamilRow(page)).toBeChecked();
    await page.locator("label", { has: tamilRow(page) }).click();
    await expect(tamilRow(page)).not.toBeChecked();

    const toggles = await getMockCalls(
      page,
      "change_stt_tamil_enabled_setting",
    );
    expect(toggles.at(-1)).toEqual({ enabled: false });

    await openPicker(page, "English");
    await expect(tamilOption(page)).toHaveCount(0);
  });

  test("an engine error is shown in the row and does not hide Tamil", async ({
    page,
  }) => {
    const message = "The Tamil speech model could not be loaded.";
    // Installed and enabled, but a transient engine error is set (ready is false).
    await installTauriMock(
      page,
      baseOptions(
        { ...READY, ready: false, engine_error: message },
        {
          selected_language: "ta",
        },
      ),
    );
    await openModels(page);

    await expect(statusLine(page)).toContainText(message);
    await expect(pickerButton(page, "Tamil")).toBeVisible();
    await openPicker(page, "Tamil");
    await expect(tamilOption(page).last()).toBeVisible();
    await closePicker(page);
  });
});
