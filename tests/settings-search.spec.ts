import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock, setMockSetting } from "./fixtures/tauriMock";

/*
 * Mounted acceptance tests for Settings search (DESIGN.md rev 5).
 * Phase 1 (smoke) proves the Vite app boots with the Tauri mock.
 * The remaining scenarios target UI that is NOT implemented yet
 * (SettingsSearch combobox, data-setting-title/data-group-id attributes,
 * settings-search-highlight class). They are expected to fail until it exists.
 *
 * Selectors that depend on translation strings use the en/de translation
 * files as the source of truth where possible.
 */

const RESULT_ID_PREFIX = "settings-search-option-";
const HIGHLIGHT_CLASS = "settings-search-highlight";
const HIGHLIGHT_MS = 1800;

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

const searchBox = (page: Page): Locator =>
  page.getByRole("combobox", { name: /search settings/i });
// Locale-neutral locator for the combobox (accessible name is translated).
const localeSearchBox = (page: Page): Locator =>
  page.locator(
    'input[role="combobox"][aria-controls="settings-search-listbox"]',
  );
const listbox = (page: Page): Locator => page.getByRole("listbox");
const options = (page: Page): Locator => page.getByRole("option");
const highlighted = (page: Page): Locator =>
  page.locator(`.${HIGHLIGHT_CLASS}`);
const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });

async function activeOptionId(box: Locator): Promise<string | null> {
  return box.getAttribute("aria-activedescendant");
}

test.describe("Settings search: boot smoke", () => {
  test("app boots to the settings UI with the Tauri mock", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");

    // Settings UI, not onboarding: the sidebar General entry is visible.
    // Sidebar entries are clickable <div>s with a <p> label (no button role).
    await expect(navItem(page, "General")).toBeVisible();

    await navItem(page, "Advanced").click();
    await expect(page.getByText("App", { exact: true }).first()).toBeVisible();
  });
});

test.describe("Settings search: navigation and reveal", () => {
  test("(1) only the destination same-titled row is highlighted after navigating", async ({
    page,
  }) => {
    // Outgoing page (Post Process) has a row titled "Model"; the destination
    // (Experimental > Spoken replies) has another row titled "Model".
    await installTauriMock(page, {
      settings: {
        post_process_enabled: true,
        experimental_enabled: true,
        // The destination "Model" row renders only in model mode.
        tts_composer_mode: "model",
      },
    });
    await page.goto("/");
    await navItem(page, "Post Process").click();
    await expect(section(page, "postprocessing")).toBeVisible();

    await searchBox(page).fill("model");
    await options(page)
      .filter({ hasText: "Model" })
      .filter({ hasText: "Experimental" })
      .first()
      .click();

    await expect(section(page, "experimental")).toBeVisible();
    await expect(highlighted(page)).toHaveCount(1);
    await expect(
      section(page, "experimental").locator(`.${HIGHLIGHT_CLASS}`),
    ).toHaveCount(1);
    await expect(highlighted(page)).toBeInViewport();
  });

  test("(2) closed Experimental card opens when localStorage throws and the row is revealed", async ({
    page,
  }) => {
    await installTauriMock(page, {
      storageThrows: true,
      settings: { experimental_enabled: true },
    });
    await page.goto("/");

    await searchBox(page).fill("keep mic");
    await options(page).filter({ hasText: "Keep Mic Open" }).first().click();

    const title = "Keep Mic Open Between Transcriptions";
    const row = page.locator(`[data-setting-title="${title}"]`);
    await expect(row).toBeVisible();
    await expect(row).toHaveClass(new RegExp(HIGHLIGHT_CLASS));
  });

  test("(3) card opens after late mount when the page is entered from search", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    // The Experimental page is not mounted until the search result navigates there.
    await expect(section(page, "experimental")).toHaveCount(0);
    await searchBox(page).fill("keep mic");
    await options(page).filter({ hasText: "Keep Mic Open" }).first().click();

    await expect(section(page, "experimental")).toBeVisible();
    const row = page.locator(
      '[data-group-id] [data-setting-title="Keep Mic Open Between Transcriptions"]',
    );
    await expect(row).toBeVisible();
  });

  test("(10) leaving mid-reveal cancels later highlight and scroll", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    await searchBox(page).fill("keep mic");
    await options(page).filter({ hasText: "Keep Mic Open" }).first().click();
    await navItem(page, "General").click();

    // Timing window under test: the reveal would have run within this period.
    await page.waitForTimeout(HIGHLIGHT_MS + 500);
    await expect(section(page, "general")).toBeVisible();
    await expect(highlighted(page)).toHaveCount(0);
  });

  test("(12) pageOnly results navigate without a reveal", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");

    await navItem(page, "General").click();
    await searchBox(page).fill("history");
    await options(page).filter({ hasText: "History" }).first().click();

    await expect(section(page, "history")).toBeVisible();
    await expect(highlighted(page)).toHaveCount(0);
  });
});

test.describe("Settings search: keyboard and composition", () => {
  test("(4a) ArrowDown and ArrowUp wrap through the results", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("mic");
    await expect(listbox(page)).toBeVisible();
    const count = await options(page).count();
    expect(count).toBeGreaterThan(1);

    const first = await activeOptionId(box);
    for (let i = 0; i < count; i += 1) {
      await page.keyboard.press("ArrowDown");
    }
    await expect(box).toHaveAttribute("aria-activedescendant", first ?? "");

    await page.keyboard.press("ArrowUp");
    await expect(box).not.toHaveAttribute("aria-activedescendant", first ?? "");
  });

  test("(4b) Enter selects the active result and clears the query", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("keep mic");
    await expect(listbox(page)).toBeVisible();
    await page.keyboard.press("Enter");

    await expect(box).toHaveValue("");
    await expect(listbox(page)).toHaveCount(0);
    await expect(section(page, "experimental")).toBeVisible();
  });

  test("(4c) Escape clears the query, a second Escape blurs", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("mic");
    await box.press("Escape");
    await expect(box).toHaveValue("");
    await box.press("Escape");
    await expect(box).not.toBeFocused();
  });

  test("(4d) Tab closes the popover and removes aria-activedescendant", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("mic");
    await expect(box).toHaveAttribute("aria-activedescendant", /.+/);
    await box.press("Tab");
    await expect(listbox(page)).toHaveCount(0);
    await expect(box).not.toHaveAttribute("aria-activedescendant");
  });

  test("(4e) outside click closes the popover and removes aria-activedescendant", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("mic");
    await expect(listbox(page)).toBeVisible();
    await page.locator("h2").first().click();
    await expect(listbox(page)).toHaveCount(0);
    await expect(box).not.toHaveAttribute("aria-activedescendant");
  });

  test("(4f) Cmd+F focuses the search box", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");

    await page.locator("h2").first().click();
    await page.keyboard.press("ControlOrMeta+f");
    await expect(searchBox(page)).toBeFocused();
  });

  test("(5a) while composing, arrows, Enter and Escape do nothing", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("mic");
    await expect(listbox(page)).toBeVisible();
    const before = await activeOptionId(box);

    await box.evaluate((el) =>
      el.dispatchEvent(
        new CompositionEvent("compositionstart", { bubbles: true }),
      ),
    );
    await box.press("ArrowDown");
    await box.press("Escape");
    await box.press("Enter");

    await expect(box).toHaveValue("mic");
    await expect(box).toHaveAttribute("aria-activedescendant", before ?? "");
    await expect(section(page, "general")).toBeVisible();
  });

  test("(5b) an Enter within 50 ms after compositionend is ignored", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("keep mic");
    await expect(listbox(page)).toBeVisible();

    // Same-task dispatch keeps the window deterministic (WebKit order guard).
    await box.evaluate((el) => {
      el.dispatchEvent(
        new CompositionEvent("compositionstart", { bubbles: true }),
      );
      el.dispatchEvent(
        new CompositionEvent("compositionend", { bubbles: true }),
      );
      el.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
      );
    });

    await expect(box).toHaveValue("keep mic");
    await expect(section(page, "general")).toBeVisible();
  });
});

test.describe("Settings search: active option and results", () => {
  test("(6a) the 30th result scrolls into view", async ({ page }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("e");
    await expect(options(page)).toHaveCount(30);
    for (let i = 0; i < 29; i += 1) {
      await page.keyboard.press("ArrowDown");
    }
    await expect(options(page).nth(29)).toBeInViewport();
  });

  test("(6b) shrinking results leaves a valid active option", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("e");
    await expect(options(page)).toHaveCount(30);
    for (let i = 0; i < 25; i += 1) {
      await page.keyboard.press("ArrowDown");
    }
    await box.fill("mic");
    await expect(listbox(page)).toBeVisible();

    const active = await activeOptionId(box);
    expect(active).toMatch(new RegExp(`^${RESULT_ID_PREFIX}`));
    const ids = await options(page).evaluateAll((nodes) =>
      nodes.map((n) => n.id),
    );
    expect(ids).toContain(active);
  });

  test("(11) no-result state shows the query; clearing leaves the page unchanged", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    await navItem(page, "Advanced").click();
    await expect(section(page, "advanced")).toBeVisible();

    const box = searchBox(page);
    await box.fill("zzqqxx");
    await expect(page.getByText(/No settings found for/)).toContainText(
      "zzqqxx",
    );

    await box.fill("");
    await expect(listbox(page)).toHaveCount(0);
    await expect(section(page, "advanced")).toBeVisible();
  });
});

test.describe("Settings search: locked and gated results", () => {
  test("(8a) with Experimental off, a locked result shows the hint and routes to Advanced", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    const box = searchBox(page);
    await box.fill("lazy");
    const hint = options(page).filter({ hasText: /Turn on .*first/ });
    await expect(hint.first()).toBeVisible();
    await hint.first().click();

    await expect(section(page, "advanced")).toBeVisible();
    await expect(highlighted(page)).toHaveAttribute(
      "data-setting-title",
      "Experimental Features",
    );
  });

  test("(8b) with Experimental on and post-processing off, a provider result routes to the Post Processing toggle", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    await searchBox(page).fill("provider");
    const locked = options(page).filter({ hasText: /Turn on .*first/ });
    await expect(locked.first()).toBeVisible();
    await locked.first().click();

    await expect(section(page, "experimental")).toBeVisible();
    await expect(highlighted(page)).toHaveAttribute(
      "data-setting-title",
      "Post Processing",
    );
  });
});

test.describe("Settings search: debug and language", () => {
  test("(9) debug entries are absent when debug mode is off and present when on", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");

    await searchBox(page).fill("log level");
    await expect(options(page).filter({ hasText: "Log Level" })).toHaveCount(0);

    await setMockSetting(page, "debug_mode", true);
    await searchBox(page).fill("");
    await searchBox(page).fill("log level");
    await expect(
      options(page).filter({ hasText: "Log Level" }).first(),
    ).toBeVisible();
  });

  test("(7) results follow the app language (German boot, German label found)", async ({
    page,
  }) => {
    // NOTE: the mock cannot switch i18n at runtime (the app syncs language only
    // at boot), so this covers the boot-language path instead.
    const de = translation("de");
    const germanLabel = textAt(de, "settings.advanced.lazyStreamClose.label");
    await installTauriMock(page, {
      settings: { app_language: "de", experimental_enabled: true },
    });
    await page.goto("/");

    // Locale-neutral: the German accessible name differs from the English one.
    await localeSearchBox(page).fill(germanLabel);
    await expect(
      options(page).filter({ hasText: germanLabel }).first(),
    ).toBeVisible();
  });
});
