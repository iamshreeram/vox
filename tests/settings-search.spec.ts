import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock, setMockSetting } from "./fixtures/tauriMock";

/*
 * Mounted acceptance tests for Settings search (DESIGN.md rev 5), run against the
 * Vite dev server with the Tauri mock (tests/fixtures/tauriMock.ts). The suite
 * covers: boot smoke; cross-page reveal (same-titled rows); reveal into a card
 * that starts collapsed (storage blocked, and late mount after leaving the page);
 * reveal cancellation on sidebar navigation (Playwright clock); keyboard and IME
 * handling of the combobox; active-option reconciliation; locked results routed
 * to their enabler; debug gating; pageOnly navigation; no-result state; and the
 * German boot-language path.
 * Selectors that depend on translated text use the en/de translation files as
 * the source of truth, or locale-neutral attributes.
 */

const RESULT_ID_PREFIX = "settings-search-option-";
const HIGHLIGHT_CLASS = "settings-search-highlight";

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

// The "listening" card starts collapsed (defaultOpen: false in settingsPlacement.ts).
const listeningCard = (page: Page): Locator =>
  page.locator('[data-group-id="listening"]');
const listeningHeader = (page: Page): Locator =>
  listeningCard(page).getByRole("button").first();
const wakeWordRow = (page: Page): Locator =>
  page.locator('[data-group-id="listening"] [data-setting-title="Wake Word"]');

/** Precondition: the listening card is rendered and collapsed, its row hidden. */
async function expectListeningCollapsed(page: Page): Promise<void> {
  await expect(listeningHeader(page)).toHaveAttribute("aria-expanded", "false");
  await expect(wakeWordRow(page)).toHaveCount(1);
  await expect(wakeWordRow(page)).toBeHidden();
}

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
    // (Models > Spoken replies) has another row titled "Model".
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
      .filter({ hasText: "Spoken replies" })
      .first()
      .click();

    await expect(section(page, "models")).toBeVisible();
    await expect(highlighted(page)).toHaveCount(1);
    await expect(highlighted(page)).toHaveAttribute(
      "data-setting-title",
      "Model",
    );
    await expect(
      section(page, "models").locator(`.${HIGHLIGHT_CLASS}`),
    ).toHaveCount(1);
    await expect(highlighted(page)).toBeInViewport();
  });

  test("(2) a closed card opens from another page when localStorage getItem and setItem throw", async ({
    page,
  }) => {
    // storageThrows overrides Storage.prototype.getItem and setItem.
    await installTauriMock(page, {
      storageThrows: true,
      settings: { experimental_enabled: true },
    });
    await page.goto("/");

    // Precondition: the listening card is collapsed on the General page.
    await expect(section(page, "general")).toBeVisible();
    await expectListeningCollapsed(page);

    // Leave for another page, then reveal from search.
    await navItem(page, "Advanced").click();
    await expect(section(page, "advanced")).toBeVisible();
    await searchBox(page).fill("wake word");
    await options(page).filter({ hasText: "Wake Word" }).first().click();

    await expect(section(page, "general")).toBeVisible();
    await expect(listeningHeader(page)).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(wakeWordRow(page)).toBeVisible();
    await expect(wakeWordRow(page)).toHaveClass(new RegExp(HIGHLIGHT_CLASS));
  });

  test("(3) a card left collapsed opens after a late mount when re-entered from search", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    // Precondition: visit General, confirm the card is collapsed, leave.
    await expect(section(page, "general")).toBeVisible();
    await expectListeningCollapsed(page);
    await navItem(page, "Advanced").click();
    await expect(section(page, "advanced")).toBeVisible();
    // The General page is unmounted now; search mounts it late.
    await expect(section(page, "general")).toHaveCount(0);

    await searchBox(page).fill("wake word");
    await options(page).filter({ hasText: "Wake Word" }).first().click();

    await expect(section(page, "general")).toBeVisible();
    await expect(listeningHeader(page)).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(wakeWordRow(page)).toBeVisible();
    await expect(wakeWordRow(page)).toHaveClass(new RegExp(HIGHLIGHT_CLASS));
  });

  test("(10) a reveal held while the page changes is cancelled by sidebar navigation", async ({
    page,
  }) => {
    // Mechanism: the reveal advances in requestAnimationFrame and timers, which the
    // Playwright clock controls in the page. The clock is installed and paused, so
    // after the search selection the reveal's first frame is queued but has not run.
    // The user goes to Advanced and back to General, then time advances past
    // the reveal timeout. Sidebar.tsx's nav-click wrapper calls cancelReveal() before
    // onSectionChange. Removing that cancelReveal() call makes the queued frame run
    // once General is back: it would open the card, scroll to the row and
    // highlight it, so the assertions below would fail.
    const REVEAL_WINDOW_MS = 1500;
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.clock.install({ time: new Date("2026-10-16T10:00:00Z") });
    await page.goto("/");
    await expect(navItem(page, "General")).toBeVisible();
    // The installed clock keeps running in real time; pause well ahead so a slow page load
    // cannot overtake the pause time ("Cannot fast-forward to the past").
    await page.clock.pauseAt(new Date("2026-10-16T10:00:30Z"));

    // Precondition: the listening card is collapsed on the General page.
    await expect(section(page, "general")).toBeVisible();
    await expectListeningCollapsed(page);
    await navItem(page, "Advanced").click();
    await expect(section(page, "advanced")).toBeVisible();

    // Count scrolls of the Wake Word row (its own scrollIntoView, nothing else).
    await page.evaluate(() => {
      const spy = window as unknown as { __wakeWordScrolls: number };
      spy.__wakeWordScrolls = 0;
      const original = Element.prototype.scrollIntoView;
      Element.prototype.scrollIntoView = function (
        this: Element,
        ...args: [(boolean | ScrollIntoViewOptions)?]
      ): void {
        if (this.getAttribute("data-setting-title") === "Wake Word") {
          spy.__wakeWordScrolls += 1;
        }
        original.apply(this, args);
      };
    });

    // Select the result: the reveal starts, its frame is queued, nothing has run.
    await searchBox(page).fill("wake word");
    await options(page).filter({ hasText: "Wake Word" }).first().click();
    await expect(section(page, "general")).toBeVisible();
    await expectListeningCollapsed(page);
    await expect(highlighted(page)).toHaveCount(0);

    // Leave during the reveal, then return to General.
    await navItem(page, "Advanced").click();
    await expect(section(page, "advanced")).toBeVisible();
    await navItem(page, "General").click();
    await expect(section(page, "general")).toBeVisible();

    // Let time pass the reveal timeout: a surviving reveal would run now.
    await page.clock.runFor(REVEAL_WINDOW_MS);
    await expect(highlighted(page)).toHaveCount(0);
    await expectListeningCollapsed(page);
    const scrolls = await page.evaluate(
      () =>
        (window as unknown as { __wakeWordScrolls: number }).__wakeWordScrolls,
    );
    expect(scrolls).toBe(0);
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
    await expect(section(page, "advanced")).toBeVisible();
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
    // The mock reports macOS, where the search box listens for the Command key. `ControlOrMeta`
    // would press Control on a Linux runner and never match, so press Meta explicitly.
    await installTauriMock(page, { platform: "macos" });
    await page.goto("/");

    await page.locator("h2").first().click();
    await page.keyboard.press("Meta+f");
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

    await expect(section(page, "advanced")).toBeVisible();
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

test.describe("Settings search: new settings controls", () => {
  test("(11) a control added in this change is found by search and rendered on its settings page", async ({
    page,
  }) => {
    // Wake word cooldown is a slider in the General page Listening group (WakeWordToggle).
    const cooldownLabel = textAt(
      translation("en"),
      "settings.advanced.wakeWord.cooldownLabel",
    );
    await installTauriMock(page, { settings: { experimental_enabled: true } });
    await page.goto("/");

    await searchBox(page).fill("wake word cooldown");
    await options(page).filter({ hasText: cooldownLabel }).first().click();

    await expect(section(page, "general")).toBeVisible();
    await expect(
      section(page, "general")
        .getByText(cooldownLabel, { exact: true })
        .first(),
    ).toBeVisible();
  });
});
