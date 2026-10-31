import { test, expect, type Locator, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import {
  emitMockEvent,
  installTauriMock,
  setMockCommand,
} from "./fixtures/tauriMock";

/*
 * Settings empty states and load-failure Retry (UI enterprise T5, DESIGN U9). Runs against the
 * Vite dev server with the Tauri mock, in the style of settings-polish.spec.ts.
 *
 * - Models with nothing downloaded says "No models downloaded yet" with a CTA that focuses the
 *   downloadable list; a filter or search that matches nothing says "No models match this filter"
 *   with Clear filters. The two never show each other's text.
 * - A failed shortcut load shows a Retry button: fail -> Retry -> success, fail -> Retry -> fail,
 *   busy disables Retry, and only one row announces the failure (role=alert).
 * - Actionable controls are at least 36 px high, and nothing is clipped at 1100x760 or 480x720.
 */

const SHOTS = "/tmp/voxux/t5-shots";
const SIZES = {
  wide: { width: 1100, height: 760 },
  narrow: { width: 480, height: 720 },
};

const REMOTE_MODELS = [
  {
    id: "remote-alpha",
    name: "Remote Alpha",
    description: "Not downloaded yet. Recommended for most speakers.",
    filename: "remote-alpha.bin",
    source: "Local",
    size_mb: 10,
    is_downloaded: false,
    is_downloading: false,
    partial_size: 0,
    is_directory: false,
    engine_type: "TranscribeCpp",
    accuracy_score: 0.5,
    speed_score: 0.5,
    supports_translation: false,
    is_recommended: true,
    supported_languages: ["en"],
    supports_language_selection: false,
    is_custom: false,
    supports_streaming: false,
    supports_language_detection: false,
  },
  {
    id: "remote-beta",
    name: "Remote Beta",
    description: "Not downloaded yet. Streams live text.",
    filename: "remote-beta.bin",
    source: "Local",
    size_mb: 20,
    is_downloaded: false,
    is_downloading: false,
    partial_size: 0,
    is_directory: false,
    engine_type: "TranscribeCpp",
    accuracy_score: 0.6,
    speed_score: 0.6,
    supports_translation: false,
    is_recommended: false,
    supported_languages: ["en"],
    supports_language_selection: false,
    is_custom: false,
    supports_streaming: true,
    supports_language_detection: false,
  },
];

const LOCAL_MODEL = {
  ...REMOTE_MODELS[0],
  id: "local-alpha",
  name: "Local Alpha",
  description: "Already on this computer.",
  is_downloaded: true,
  is_recommended: false,
};

const NONE_DOWNLOADED = "No models downloaded yet";
const NO_MATCH = "No models match this filter.";
const SHORTCUT_ERROR = "Vox could not load your shortcut list.";

const tid = (page: Page, id: string): Locator =>
  page.locator(`[data-testid="${id}"]`);

async function heightOf(locator: Locator): Promise<number> {
  const box = await locator.boundingBox();
  return box?.height ?? 0;
}

async function openApp(
  page: Page,
  size: { width: number; height: number },
  commands: Record<string, unknown> = {},
  settingsOverride: Record<string, unknown> = {},
): Promise<void> {
  await page.setViewportSize(size);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      ...settingsOverride,
    },
    commands,
  });
  await page.goto("/");
  await expect(page.locator("p", { hasText: /^General$/ })).toBeVisible();
}

async function openModels(page: Page): Promise<void> {
  await page.locator("p", { hasText: /^Models$/ }).click();
  await expect(page.locator('[data-settings-section="models"]')).toBeVisible();
}

const search = (page: Page): Locator =>
  page.getByRole("textbox", { name: /search models/i });

async function focusedInsideAvailable(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const active = document.activeElement;
    const heading = Array.from(document.querySelectorAll("h2")).find(
      (h) => h.textContent?.trim() === "Available to Download",
    );
    return Boolean(
      active &&
        heading?.parentElement &&
        heading.parentElement.contains(active),
    );
  });
}

function shotDir(): void {
  mkdirSync(SHOTS, { recursive: true });
}

test.describe("settings empty states (T5)", () => {
  for (const [label, size] of Object.entries(SIZES)) {
    test(`no models downloaded shows the none-downloaded state at ${label} ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await openApp(page, size, { get_available_models: REMOTE_MODELS });
      await openModels(page);

      const block = tid(page, "models-empty-none");
      await expect(block).toBeVisible();
      await expect(block.getByText(NONE_DOWNLOADED)).toBeVisible();
      await expect(
        block.getByText(
          "Download a model from the list below to start transcribing.",
        ),
      ).toBeVisible();
      await expect(page.getByText(NO_MATCH)).toHaveCount(0);

      const cta = block.getByRole("button", {
        name: "Show models to download",
      });
      await expect(cta).toBeVisible();
      expect(await heightOf(cta)).toBeGreaterThanOrEqual(36);

      const pageOverflow = await page.evaluate(
        () => document.documentElement.scrollWidth - window.innerWidth,
      );
      expect(pageOverflow).toBeLessThanOrEqual(0);
      const blockClipped = await block.evaluate(
        (el: HTMLElement) => el.scrollWidth > el.clientWidth + 1,
      );
      expect(blockClipped).toBe(false);

      shotDir();
      await page.screenshot({
        path: `${SHOTS}/models-empty-none-${size.width}.png`,
        fullPage: false,
      });
    });

    test(`a search with no matches shows the no-match state at ${label} ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await openApp(page, size, { get_available_models: REMOTE_MODELS });
      await openModels(page);
      await search(page).fill("zzz-no-such-model");

      await expect(page.getByText(NO_MATCH)).toBeVisible();
      await expect(tid(page, "models-empty-none")).toHaveCount(0);
      expect(
        await page
          .getByRole("button", { name: "Clear filters" })
          .evaluate((el: HTMLElement) => el.scrollWidth <= el.clientWidth + 1),
      ).toBe(true);

      shotDir();
      await page.screenshot({
        path: `${SHOTS}/models-empty-nomatch-${size.width}.png`,
        fullPage: false,
      });
    });
  }

  test("the CTA scrolls to and focuses the downloadable list (mouse and keyboard)", async ({
    page,
  }) => {
    await openApp(page, SIZES.wide, { get_available_models: REMOTE_MODELS });
    await openModels(page);
    const cta = tid(page, "models-empty-none").getByRole("button", {
      name: "Show models to download",
    });
    await cta.click();
    await expect.poll(() => focusedInsideAvailable(page)).toBe(true);

    await cta.focus();
    await page.keyboard.press("Enter");
    await expect.poll(() => focusedInsideAvailable(page)).toBe(true);
  });

  test("Clear filters resets the search and the none-downloaded state returns", async ({
    page,
  }) => {
    await openApp(page, SIZES.wide, { get_available_models: REMOTE_MODELS });
    await openModels(page);
    await search(page).fill("zzz-no-such-model");
    await expect(page.getByText(NO_MATCH)).toBeVisible();

    const clear = page.getByRole("button", { name: "Clear filters" });
    expect(await heightOf(clear)).toBeGreaterThanOrEqual(36);
    await clear.focus();
    await page.keyboard.press("Enter");

    await expect(search(page)).toHaveValue("");
    await expect(page.getByText(NO_MATCH)).toHaveCount(0);
    await expect(tid(page, "models-empty-none")).toBeVisible();
  });

  test("a filter with models present never shows the none-downloaded text", async ({
    page,
  }) => {
    await openApp(page, SIZES.wide, {
      get_available_models: [LOCAL_MODEL, ...REMOTE_MODELS],
      get_current_model: "local-alpha",
    });
    await openModels(page);
    await expect(tid(page, "models-empty-none")).toHaveCount(0);

    await search(page).fill("zzz-no-such-model");
    await expect(page.getByText(NO_MATCH)).toBeVisible();
    await expect(page.getByText(NONE_DOWNLOADED)).toHaveCount(0);
  });
});

test.describe("shortcut load failure Retry (T5)", () => {
  // The settings read fails only after the app is up: the shortcut list is empty (bindings {}),
  // so the empty branch renders, and a later settings refresh fails. The failure is persistent
  // until the test flips the command to succeed (__count__: 0 falls through to the normal answer).
  const FAILING = { __error__: "settings unavailable" };
  const succeedNext = (page: Page) =>
    setMockCommand(page, "get_app_settings", { ...FAILING, __count__: 0 });

  const openWithFailedRefresh = async (page: Page) => {
    await openApp(page, SIZES.wide, {}, { bindings: {} });
    await setMockCommand(page, "get_app_settings", FAILING);
    await emitMockEvent(page, "settings-changed", { setting: "debug_mode" });
    await expect(page.getByText(SHORTCUT_ERROR).first()).toBeVisible();
  };

  test("fail -> Retry -> success clears the message; one row announces it", async ({
    page,
  }) => {
    await openWithFailedRefresh(page);
    await expect(
      page.getByRole("alert").filter({ hasText: SHORTCUT_ERROR }),
    ).toHaveCount(1);
    await expect(page.getByText("No shortcuts configured").first()).toHaveCount(
      0,
    );

    const retry = page.getByRole("button", { name: "Retry" }).first();
    expect(await heightOf(retry)).toBeGreaterThanOrEqual(36);

    shotDir();
    await page.screenshot({
      path: `${SHOTS}/shortcut-error-retry-${SIZES.wide.width}.png`,
    });

    await succeedNext(page);
    await retry.click();
    await expect(page.getByText(SHORTCUT_ERROR)).toHaveCount(0);
  });

  test("fail -> Retry -> fail keeps the same text and Retry stays enabled", async ({
    page,
  }) => {
    await openWithFailedRefresh(page);

    const retry = page.getByRole("button", { name: "Retry" }).first();
    await retry.click();
    await expect(page.getByText(SHORTCUT_ERROR).first()).toBeVisible();
    await expect(retry).toBeEnabled();
    await expect(
      page.getByRole("alert").filter({ hasText: SHORTCUT_ERROR }),
    ).toHaveCount(1);

    await succeedNext(page);
    await retry.click();
    await expect(page.getByText(SHORTCUT_ERROR)).toHaveCount(0);
  });

  test("Retry is busy and disabled while the load is in flight", async ({
    page,
  }) => {
    await openWithFailedRefresh(page);

    // The next settings load never answers, so the Retry stays busy.
    await setMockCommand(page, "get_app_settings", { __pending__: true });
    await page.getByRole("button", { name: "Retry" }).first().click();

    const busy = page.getByRole("button", { name: "Retrying\u2026" }).first();
    await expect(busy).toBeDisabled();
    await expect(busy).toHaveAttribute("aria-busy", "true");
  });
});

test("setup banner actions are at least 36 px high and nothing is clipped at 480 px", async ({
  page,
}) => {
  await openApp(page, SIZES.narrow, { get_available_models: REMOTE_MODELS });
  await openModels(page);

  const banner = tid(page, "setup-status-banner");
  await expect(banner).toBeVisible();
  const buttons = banner.getByRole("button");
  const count = await buttons.count();
  expect(count).toBeGreaterThan(0);
  for (let index = 0; index < count; index += 1) {
    expect(await heightOf(buttons.nth(index))).toBeGreaterThanOrEqual(36);
  }
  const clipped = await banner.evaluate(
    (el: HTMLElement) => el.scrollWidth > el.clientWidth + 1,
  );
  expect(clipped).toBe(false);
});
