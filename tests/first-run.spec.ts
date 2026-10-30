import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";

/*
 * First run (DESIGN.md 'First run (C4)' Rev 2, 'First-run networking and consent' Rev 3).
 *
 * A fresh install goes: permissions -> transcription model -> Voice step -> Brain step -> main app.
 * Both extra steps are skippable. Returning users never see them, but get a dismissible
 * "Finish voice setup" callout on Models while spoken replies or the Brain are not set up.
 *
 * Mock contract (layered on tests/fixtures/tauriMock.ts without editing it):
 * - every invoke is recorded by name; read with getMockCalls(page, name).
 * - first_run_finish_step(step, outcome) is applied to the mocked settings the way the Rust
 *   spec does: voice+done sets first_run_tts "done" and tts_enabled true; voice+skipped and
 *   brain+skipped only change their own outcome; once neither step is "unset" anymore,
 *   first_run_setup_pending becomes false. Changes go through __voxMock.setSetting, so the
 *   settings store refreshes like it does for a real backend.
 *
 * Strings are read from the English translation file. Keys that do not exist yet fall back to
 * the key itself, so a missing string fails the matching assertion instead of crashing the module.
 * Brain controls reuse the data-testid contract of tests/brain-page.spec.ts.
 */

const en = JSON.parse(
  readFileSync(
    new URL("../src/i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

function textOr(root: Record<string, unknown>, dottedKey: string): string {
  let node: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (typeof node !== "object" || node === null) return dottedKey;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : dottedKey;
}
const label = (key: string): string => textOr(en, key);

const tid = (page: Page, id: string): Locator => page.getByTestId(id);
const brainField = (page: Page, id: string): Locator =>
  page.getByTestId(id).locator("input").first();
const navItem = (page: Page, text: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${text}$`) });
const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);

const BRAIN_CALLS = ["brain_test_connection", "brain_fetch_models"] as const;

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

const NOT_INSTALLED = {
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
};

// Fresh Brain settings as a new install stores them (see tests/brain-page.spec.ts).
const FRESH_BRAIN = {
  brain_schema_v1: true,
  brain_provider_id: "",
  brain_base_url: "",
  brain_model: "",
  brain_token_set: false,
  brain_source_composer: "inherit",
  brain_source_realtime: "inherit",
  brain_source_postprocess: "inherit",
  brain_test_ack_host: "",
  brain_transcript_consent_dest: "",
};

// A brand-new install right after the transcription model pick. Picking a model makes the
// backend set onboarding_completed (models.rs) and first_run_setup_pending starts true, so this
// is exactly the state the Voice step must start from (also what a restart resumes from).
const FRESH_FIRST_RUN = {
  ...FRESH_BRAIN,
  onboarding_completed: true,
  first_run_setup_pending: true,
  first_run_tts: "unset",
  first_run_brain: "unset",
  tts_enabled: false,
};

// Restart in the middle: the model was chosen (models.rs sets onboarding_completed on pick),
// the Voice step is done, the Brain step is still open.
const RESTARTED_AFTER_VOICE = {
  ...FRESH_BRAIN,
  onboarding_completed: true,
  first_run_setup_pending: true,
  first_run_tts: "done",
  first_run_brain: "unset",
  tts_enabled: true,
};

// Returning user: finished first run long ago, nothing set up for voice or the Brain.
const RETURNING = {
  onboarding_completed: true,
  first_run_setup_pending: false,
  first_run_tts: "unset",
  first_run_brain: "unset",
  tts_enabled: false,
};

const MODEL_COMMANDS = {
  get_available_models: [MOCK_MODEL],
  get_current_model: "mock-model",
  phonon_voice_status: NOT_INSTALLED,
  natural_voice_status: NOT_INSTALLED,
};

/**
 * Runs inside the page after the Tauri mock. Wraps invoke so that first_run_finish_step is
 * applied to the mocked settings. The base mock still records the call itself.
 */
function installFirstRunMockInPage(): void {
  interface FirstRunWindow {
    __TAURI_INTERNALS__: {
      invoke: (c: string, a?: Record<string, unknown>) => Promise<unknown>;
    };
    __voxMock: { setSetting: (k: string, v: unknown) => void };
  }
  const win = window as unknown as FirstRunWindow;
  const internals = win.__TAURI_INTERNALS__;
  const base = internals.invoke;
  internals.invoke = async (command, args = {}) => {
    const result = await base(command, args);
    // The Brain commands persist in Rust; mirror what they store so the page re-reads it.
    if (command === "change_brain_connection_setting") {
      win.__voxMock.setSetting("brain_provider_id", args.providerId);
      win.__voxMock.setSetting("brain_base_url", args.baseUrl);
      win.__voxMock.setSetting("brain_model", args.model);
      win.__voxMock.setSetting("brain_test_ack_host", "");
      win.__voxMock.setSetting("brain_transcript_consent_dest", "");
      return result;
    }
    if (command === "brain_acknowledge_test") {
      const current = (await base("get_app_settings", {})) as Record<
        string,
        unknown
      >;
      const url = new URL(String(current.brain_base_url));
      win.__voxMock.setSetting("brain_test_ack_host", url.host);
      return result;
    }
    if (command !== "first_run_finish_step") return result;

    const current = (await base("get_app_settings", {})) as Record<
      string,
      unknown
    >;
    const updates: Record<string, unknown> = {};
    if (args.step === "voice") {
      updates.first_run_tts = args.outcome;
      if (args.outcome === "done") updates.tts_enabled = true;
    } else {
      updates.first_run_brain = args.outcome;
    }
    const tts = updates.first_run_tts ?? current.first_run_tts;
    const brain = updates.first_run_brain ?? current.first_run_brain;
    if (tts !== "unset" && brain !== "unset") {
      updates.first_run_setup_pending = false;
    }
    for (const [key, value] of Object.entries(updates)) {
      win.__voxMock.setSetting(key, value);
    }
    return result;
  };
}

async function install(
  page: Page,
  settings: Record<string, unknown>,
  commands: Record<string, unknown> = {},
): Promise<void> {
  await installTauriMock(page, {
    settings,
    commands: { ...MODEL_COMMANDS, ...commands },
  });
  await page.addInitScript(installFirstRunMockInPage);
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

async function reachVoiceStep(page: Page): Promise<void> {
  await expect(
    page.getByRole("heading", {
      name: label("onboarding.firstRun.voice.title"),
    }),
  ).toBeVisible();
}

async function reachBrainStep(page: Page): Promise<void> {
  await expect(
    page.getByRole("heading", {
      name: label("onboarding.firstRun.brain.title"),
    }),
  ).toBeVisible();
}

async function expectMainApp(page: Page): Promise<void> {
  await expect(navItem(page, "General")).toBeVisible();
  await expect(
    page.getByRole("heading", {
      name: label("onboarding.firstRun.brain.title"),
    }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: label("onboarding.firstRun.voice.title"),
    }),
  ).toHaveCount(0);
}

function button(page: Page, key: string): Locator {
  return page.getByRole("button", { name: label(key), exact: true });
}

test.describe("First run", () => {
  test("1. after the model is chosen a Voice step offers engines, Continue and Skip; Continue finishes voice and opens Brain", async ({
    page,
  }) => {
    await install(page, FRESH_FIRST_RUN);
    await page.goto("/");

    await reachVoiceStep(page);
    await expect(
      page
        .locator('[data-testid="tts-engine-card"], [data-testid="model-row"]')
        .first(),
    ).toBeVisible();
    await expect(
      button(page, "onboarding.firstRun.voice.continue"),
    ).toBeVisible();
    await expect(button(page, "onboarding.firstRun.voice.skip")).toBeVisible();

    await button(page, "onboarding.firstRun.voice.continue").click();
    expect(await getMockCalls(page, "first_run_finish_step")).toEqual([
      { step: "voice", outcome: "done" },
    ]);
    await reachBrainStep(page);
  });

  test("2. Brain step: typing makes no Brain network call; Test needs the acknowledgement; Use this for spoken replies finishes", async ({
    page,
  }) => {
    await install(page, FRESH_FIRST_RUN);
    await page.goto("/");
    await reachVoiceStep(page);
    await button(page, "onboarding.firstRun.voice.continue").click();
    await reachBrainStep(page);

    // Choose a preset and type endpoint, model and token. Nothing may be sent yet.
    await tid(page, "brain-quickfill-ollama").click();
    await tid(page, "brain-base-url").fill("http://127.0.0.1:11434/v1");
    await brainField(page, "brain-model").fill("llama3");
    await brainField(page, "brain-model").press("Enter");
    await tid(page, "brain-token").fill("sk-FIRST-RUN-SENTINEL");
    await tid(page, "brain-token").blur();
    for (const command of BRAIN_CALLS) {
      expect(await getMockCalls(page, command)).toHaveLength(0);
    }

    // Test stays disabled until the acknowledgement is ticked.
    await expect(tid(page, "brain-test")).toBeDisabled();
    await tid(page, "brain-ack").getByRole("checkbox").check();
    expect(await getMockCalls(page, "brain_acknowledge_test")).toHaveLength(1);
    await expect(tid(page, "brain-test")).toBeEnabled();
    expect(await getMockCalls(page, "brain_test_connection")).toHaveLength(0);

    // One click sends exactly one connection test; the mock answers ok.
    await tid(page, "brain-test").click();
    await expect(tid(page, "brain-test-result")).toBeVisible();
    expect(await getMockCalls(page, "brain_test_connection")).toHaveLength(1);

    await button(page, "onboarding.firstRun.brain.useForReplies").click();
    expect(await getMockCalls(page, "brain_use_for_feature")).toEqual([
      { feature: "composer", useBrain: true },
    ]);
    expect(await getMockCalls(page, "first_run_finish_step")).toEqual([
      { step: "voice", outcome: "done" },
      { step: "brain", outcome: "done" },
    ]);
    await expectMainApp(page);
  });

  test("3. Skip on both steps: both outcomes are recorded, no Brain network call, spoken replies stay off", async ({
    page,
  }) => {
    await install(page, FRESH_FIRST_RUN);
    await page.goto("/");
    await reachVoiceStep(page);
    await button(page, "onboarding.firstRun.voice.skip").click();
    await reachBrainStep(page);
    await button(page, "onboarding.firstRun.brain.skip").click();

    expect(await getMockCalls(page, "first_run_finish_step")).toEqual([
      { step: "voice", outcome: "skipped" },
      { step: "brain", outcome: "skipped" },
    ]);
    for (const command of BRAIN_CALLS) {
      expect(await getMockCalls(page, command)).toHaveLength(0);
    }
    expect(await readSetting(page, "tts_enabled")).toBe(false);
    expect(await readSetting(page, "first_run_setup_pending")).toBe(false);
    await expectMainApp(page);
  });

  test("4. restart in the middle loads straight into the Brain step", async ({
    page,
  }) => {
    await install(page, RESTARTED_AFTER_VOICE);
    await page.goto("/");

    await reachBrainStep(page);
    await expect(
      page.getByRole("heading", {
        name: label("onboarding.firstRun.voice.title"),
      }),
    ).toHaveCount(0);
    expect(await getMockCalls(page, "first_run_finish_step")).toHaveLength(0);
    for (const command of BRAIN_CALLS) {
      expect(await getMockCalls(page, command)).toHaveLength(0);
    }
  });

  test("5. returning user goes straight to the main app; Models callout can be dismissed and stays dismissed after reload", async ({
    page,
  }) => {
    await install(page, RETURNING);
    await page.goto("/");
    await expectMainApp(page);
    expect(await getMockCalls(page, "first_run_finish_step")).toHaveLength(0);

    await navItem(page, "Models").click();
    await expect(section(page, "models")).toBeVisible();
    // S4a: the one setup-status banner replaces the Finish setup callout.
    const callout = tid(page, "setup-status-banner");
    await expect(callout).toBeVisible();
    await expect(callout).toContainText(
      label("settings.models.finishSetup.title"),
    );
    await callout
      .getByRole("button", {
        name: label("settings.models.finishSetup.dismiss"),
        exact: true,
      })
      .click();
    await expect(callout).toHaveCount(0);

    // A reload re-runs the init scripts (fresh mock settings) but keeps localStorage.
    await page.reload();
    await expectMainApp(page);
    await navItem(page, "Models").click();
    await expect(section(page, "models")).toBeVisible();
    await expect(tid(page, "setup-status-banner")).toHaveCount(0);
  });
});
