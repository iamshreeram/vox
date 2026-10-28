import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Models page information architecture (DESIGN.md Rev 5, P1). Mounted tests against the
 * Vite dev server with the Tauri mock (tests/fixtures/tauriMock.ts). Each scenario runs
 * twice: with experimental_enabled false (the default for a new user) and true.
 *
 * Covers: the three collapsible groups on Models (Speech-to-text, Text-to-speech, Voice
 * detection) with their default open state and toggling; the STT list, language/translate
 * and unload-timeout controls; the TTS engine picker; the Voice detection controls (VAD
 * backend, Smart turn, Speaker filter) inside one group; Reply style and Realtime leaving
 * Models for General -> Conversation; collapse persistence across a reload; settings search
 * reaching Smart turn and Reply style without turning Experimental on; and Agents in the
 * sidebar when Experimental is on.
 *
 * Group ids come from the placement contract (settingsPlacement.ts) and the STT group id
 * "sttModels" from ModelsSettings.tsx. Titles and control labels are read from the en
 * translation file; the three group titles are the English strings from the P1 contract.
 */

const EXPERIMENTAL_FLAGS = [false, true] as const;

const STT_GROUP = "sttModels";
const TTS_GROUP = "spokenReplies";
const VAD_GROUP = "vadEngine";
const CONVERSATION_GROUP = "conversation";

const TITLES = {
  speechToText: "Speech-to-text",
  textToSpeech: "Text-to-speech",
  voiceDetection: "Voice detection",
} as const;

const NOT_INSTALLED = {
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
};

// A transcription model that supports language selection and translation, so the Model
// settings card renders (it renders nothing without a selected model).
const MOCK_MODEL = {
  id: "mock-model",
  name: "Mock Model",
  description: "A model used by the tests.",
  filename: "mock-model.bin",
  source: "Local",
  size_mb: 10,
  is_downloaded: true,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "TranscribeCpp",
  accuracy_score: 0.5,
  speed_score: 0.5,
  supports_translation: true,
  is_recommended: false,
  supported_languages: ["en", "de"],
  supports_language_selection: true,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: true,
};

const MOCK_COMMANDS = {
  get_available_models: [MOCK_MODEL],
  get_current_model: "mock-model",
  phonon_voice_status: NOT_INSTALLED,
  natural_voice_status: NOT_INSTALLED,
};

function translation(locale: string): Record<string, unknown> {
  const url = new URL(
    `../src/i18n/locales/${locale}/translation.json`,
    import.meta.url,
  );
  return JSON.parse(readFileSync(url, "utf8")) as Record<string, unknown>;
}

function textAt(root: Record<string, unknown>, dottedKey: string): string {
  let node: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (typeof node !== "object" || node === null) {
      throw new Error(`missing translation key ${dottedKey}`);
    }
    node = (node as Record<string, unknown>)[part];
  }
  if (typeof node !== "string") {
    throw new Error(`translation key ${dottedKey} is not a string`);
  }
  return node;
}

const en = translation("en");
const label = (key: string): string => textAt(en, key);

const searchBox = (page: Page): Locator =>
  page.getByRole("combobox", { name: /search settings/i });
const options = (page: Page): Locator => page.getByRole("option");
const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, text: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${text}$`) });
const group = (page: Page, id: string): Locator =>
  page.locator(`[data-group-id="${id}"]`);
// The collapsible header is the first button of the group card (CollapsibleSettingsGroup).
const header = (page: Page, id: string): Locator =>
  group(page, id).getByRole("button").first();

async function openModels(page: Page): Promise<void> {
  await navItem(page, "Models").click();
  await expect(section(page, "models")).toBeVisible();
}

async function readSetting(page: Page, key: string): Promise<unknown> {
  return page.evaluate(async (k: string) => {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (c: string) => Promise<Record<string, unknown>>;
        };
      }
    ).__TAURI_INTERNALS__;
    const settings = await internals.invoke("get_app_settings");
    return settings[k];
  }, key);
}

for (const experimental of EXPERIMENTAL_FLAGS) {
  const suffix = experimental ? "experimental on" : "experimental off";

  test.describe(`Models page IA (${suffix})`, () => {
    test.beforeEach(async ({ page }) => {
      await installTauriMock(page, {
        settings: { experimental_enabled: experimental },
        commands: MOCK_COMMANDS,
      });
      await page.goto("/");
      await expect(navItem(page, "General")).toBeVisible();
    });

    test("1. Models shows exactly three collapsible group headers in order", async ({
      page,
    }) => {
      await openModels(page);

      const groups = section(page, "models").locator("[data-group-id]");
      await expect(groups).toHaveCount(3);
      await expect(groups.nth(0)).toHaveAttribute("data-group-id", STT_GROUP);
      await expect(groups.nth(1)).toHaveAttribute("data-group-id", TTS_GROUP);
      await expect(groups.nth(2)).toHaveAttribute("data-group-id", VAD_GROUP);

      for (const id of [STT_GROUP, TTS_GROUP, VAD_GROUP]) {
        await expect(header(page, id)).toBeVisible();
      }
      await expect(
        header(page, STT_GROUP).locator(".settings-card-title"),
      ).toHaveText(TITLES.speechToText);
      await expect(
        header(page, TTS_GROUP).locator(".settings-card-title"),
      ).toHaveText(TITLES.textToSpeech);
      await expect(
        header(page, VAD_GROUP).locator(".settings-card-title"),
      ).toHaveText(TITLES.voiceDetection);
    });

    test("2. Speech-to-text is open, holds the model list and its controls, and toggles", async ({
      page,
    }) => {
      await openModels(page);

      const stt = group(page, STT_GROUP);
      await expect(header(page, STT_GROUP)).toHaveAttribute(
        "aria-expanded",
        "true",
      );
      await expect(
        stt.getByText(MOCK_MODEL.name, { exact: true }),
      ).toBeVisible();
      await expect(
        stt.getByText(label("settings.general.language.title"), {
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        stt.getByText(label("settings.advanced.translateToEnglish.label"), {
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        stt.getByText(label("settings.advanced.modelUnload.title"), {
          exact: true,
        }),
      ).toBeVisible();

      await header(page, STT_GROUP).click();
      await expect(header(page, STT_GROUP)).toHaveAttribute(
        "aria-expanded",
        "false",
      );
      await expect(
        stt.getByText(MOCK_MODEL.name, { exact: true }),
      ).toBeHidden();

      await header(page, STT_GROUP).click();
      await expect(header(page, STT_GROUP)).toHaveAttribute(
        "aria-expanded",
        "true",
      );
      await expect(
        stt.getByText(MOCK_MODEL.name, { exact: true }),
      ).toBeVisible();
    });

    test("3. Text-to-speech is open with the voice engine choice; Voice detection is collapsed and holds its controls", async ({
      page,
    }) => {
      await openModels(page);

      await expect(header(page, TTS_GROUP)).toHaveAttribute(
        "aria-expanded",
        "true",
      );
      const tts = group(page, TTS_GROUP);
      await expect(
        tts.locator('[data-testid="tts-engine-picker"]'),
      ).toBeVisible();
      await expect(tts.locator('[data-testid="tts-engine-card"]')).toHaveCount(
        3,
      );

      const vad = group(page, VAD_GROUP);
      await expect(header(page, VAD_GROUP)).toHaveAttribute(
        "aria-expanded",
        "false",
      );
      await expect(
        vad.getByText(label("settings.advanced.vadBackend.title"), {
          exact: true,
        }),
      ).toBeHidden();

      await header(page, VAD_GROUP).click();
      await expect(header(page, VAD_GROUP)).toHaveAttribute(
        "aria-expanded",
        "true",
      );
      await expect(
        vad.getByText(label("settings.advanced.vadBackend.title"), {
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        vad
          .getByText(label("converse.voice.smartTurn.label"), { exact: true })
          .first(),
      ).toBeVisible();
      await expect(
        vad
          .getByText(label("converse.voice.speaker.label"), { exact: true })
          .first(),
      ).toBeVisible();
    });

    test("4. Models does not contain Reply style or Realtime conversation", async ({
      page,
    }) => {
      await openModels(page);

      const models = section(page, "models");
      await expect(
        models.getByText(label("settings.models.groups.replyStyle"), {
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(
        models.getByText(label("settings.experimental.realtime.title"), {
          exact: true,
        }),
      ).toHaveCount(0);
    });

    test("5. General shows the Conversation group holding Reply style and Realtime conversation", async ({
      page,
    }) => {
      await navItem(page, "General").click();
      await expect(section(page, "general")).toBeVisible();

      const conversation = group(page, CONVERSATION_GROUP);
      await expect(conversation).toHaveCount(1);
      await expect(conversation.locator(".settings-card-title")).toHaveText(
        label("settings.general.groups.conversation"),
      );

      // Expand only when collapsed, so the test does not depend on the default state.
      if (
        (await header(page, CONVERSATION_GROUP).getAttribute(
          "aria-expanded",
        )) === "false"
      ) {
        await header(page, CONVERSATION_GROUP).click();
      }
      await expect(header(page, CONVERSATION_GROUP)).toHaveAttribute(
        "aria-expanded",
        "true",
      );
      await expect(
        conversation.getByText(label("settings.models.groups.replyStyle"), {
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        conversation.getByText(label("settings.experimental.realtime.title"), {
          exact: true,
        }),
      ).toBeVisible();
    });

    test("6. Speech-to-text collapse state persists across a reload", async ({
      page,
    }) => {
      await openModels(page);
      await header(page, STT_GROUP).click();
      await expect(header(page, STT_GROUP)).toHaveAttribute(
        "aria-expanded",
        "false",
      );

      await page.reload();
      await expect(navItem(page, "General")).toBeVisible();
      await openModels(page);

      await expect(header(page, STT_GROUP)).toHaveAttribute(
        "aria-expanded",
        "false",
      );
      await expect(
        group(page, STT_GROUP).getByText(MOCK_MODEL.name, { exact: true }),
      ).toBeHidden();
    });

    test("7. Settings search reaches Smart turn and Reply style", async ({
      page,
    }) => {
      // Smart turn: the result opens Models and shows the control inside Voice detection.
      await searchBox(page).fill("smart turn");
      await options(page)
        .filter({ hasText: label("converse.voice.smartTurn.label") })
        .first()
        .click();
      await expect(section(page, "models")).toBeVisible();
      await expect(
        group(page, VAD_GROUP)
          .getByText(label("converse.voice.smartTurn.label"), {
            exact: true,
          })
          .first(),
      ).toBeVisible();

      // Reply style: the result opens General and shows the row inside Conversation.
      await searchBox(page).fill("reply style");
      await options(page)
        .filter({ hasText: label("settings.models.groups.replyStyle") })
        .first()
        .click();
      await expect(section(page, "general")).toBeVisible();
      await expect(
        group(page, CONVERSATION_GROUP).getByText(
          label("settings.models.groups.replyStyle"),
          { exact: true },
        ),
      ).toBeVisible();

      // Searching did not turn Experimental on (the mocked settings are the source of truth).
      expect(await readSetting(page, "experimental_enabled")).toBe(
        experimental,
      );
    });

    test("8. Agents is reachable in the sidebar", async ({ page }) => {
      // Only the experimental-on run asserts Agents; the off run does not check it.
      test.skip(!experimental, "Agents is asserted with Experimental on only");
      await expect(navItem(page, "Agents")).toBeVisible();
    });
  });
}
