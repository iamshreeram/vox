import { test, expect, type Locator, type Page } from "@playwright/test";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";
import { ACCENTS } from "../src/styles/accents";
import { CHAT_PALETTES } from "../src/chat/chatTheme";

/*
 * Appearance settings page (UI enterprise S3, DESIGN Revision 3 A3/A5/A9, D3). Runs against the
 * Vite dev server with the Tauri mock. Every control must reach its command with the right
 * argument, the main window must update at once, and the page must work from the keyboard alone.
 */

const ACCENT_CALL = "change_accent_setting";
const CHAT_THEME_CALL = "change_chat_theme_setting";
const STYLE_CALL = "change_visual_style_setting";
const THEME_CALL = "change_theme_setting";
const OPACITY_CALL = "change_chat_opacity_setting";

const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });
const searchBox = (page: Page): Locator =>
  page.getByRole("combobox", { name: /search settings/i });

async function openAppearance(
  page: Page,
  settings: Record<string, unknown> = {},
): Promise<void> {
  await installTauriMock(page, { settings });
  await page.goto("/");
  await navItem(page, "Appearance").click();
  await expect(section(page, "appearance")).toBeVisible();
}

const accentRadio = (page: Page, name: string): Locator =>
  page.getByRole("radiogroup", { name: "Accent colour" }).getByRole("radio", {
    name,
  });

const hexToRgb = (hex: string): string => {
  const n = parseInt(hex.slice(1), 16);
  return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
};

test.describe("Appearance page: reachable and placed", () => {
  test("reachable from the sidebar; General no longer holds the Appearance group", async ({
    page,
  }) => {
    await openAppearance(page);
    await expect(
      page.getByRole("radiogroup", { name: "Accent colour" }),
    ).toBeVisible();
    await navItem(page, "General").click();
    await expect(section(page, "general")).toBeVisible();
    await expect(
      page.getByText("Application Theme", { exact: true }),
    ).toHaveCount(0);
    await expect(
      page.locator('[data-testid="chat-opacity-slider"]'),
    ).toHaveCount(0);
  });

  test("reachable by search: the accent row opens the page", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");
    await searchBox(page).fill("accent");
    await page
      .getByRole("option", { name: /Accent colour/ })
      .first()
      .click();
    await expect(section(page, "appearance")).toBeVisible();
  });

  test("the old opacity query still finds the moved slider", async ({
    page,
  }) => {
    await installTauriMock(page);
    await page.goto("/");
    await searchBox(page).fill("transparency");
    await page
      .getByRole("option", { name: /Conversation window opacity/ })
      .first()
      .click();
    await expect(section(page, "appearance")).toBeVisible();
    await expect(
      page.locator('[data-testid="chat-opacity-slider"]'),
    ).toBeVisible();
  });
});

test.describe("Appearance page: controls reach their commands", () => {
  test("each accent calls changeAccentSetting once and sets data-accent (absent for amber)", async ({
    page,
  }) => {
    await openAppearance(page);
    const order = ["Blue", "Violet", "Emerald", "Rose", "Slate", "Amber"];
    let expectedCalls = 0;
    for (const name of order) {
      await accentRadio(page, name).click();
      expectedCalls += 1;
      const id = name.toLowerCase();
      await expect
        .poll(async () => (await getMockCalls(page, ACCENT_CALL)).length)
        .toBe(expectedCalls);
      expect((await getMockCalls(page, ACCENT_CALL)).at(-1)).toEqual({
        accent: id,
      });
      if (id === "amber") {
        await expect(page.locator("html")).not.toHaveAttribute(
          "data-accent",
          /.*/,
        );
      } else {
        await expect(page.locator("html")).toHaveAttribute("data-accent", id);
      }
    }
  });

  test("accent is disabled with an explanation under Aurora and Custom", async ({
    page,
  }) => {
    for (const theme of ["aurora", "custom"]) {
      await openAppearance(page, { theme });
      const group = page.getByRole("radiogroup", { name: "Accent colour" });
      await expect(accentRadio(page, "Blue")).toBeDisabled();
      await expect(group).toHaveAttribute("aria-disabled", "true");
      await expect(page.getByTestId("accent-locked-note")).toBeVisible();
      await page.goto("/");
    }
  });

  test("visual style toggles data-style on the main window", async ({
    page,
  }) => {
    await openAppearance(page);
    await page
      .getByRole("radiogroup", { name: "Visual style" })
      .getByRole("radio", { name: "Restrained" })
      .click();
    await expect(page.locator("html")).toHaveAttribute(
      "data-style",
      "restrained",
    );
    expect((await getMockCalls(page, STYLE_CALL)).at(-1)).toEqual({
      visualStyle: "restrained",
    });
    await page
      .getByRole("radiogroup", { name: "Visual style" })
      .getByRole("radio", { name: "Vibrant" })
      .click();
    await expect(page.locator("html")).not.toHaveAttribute("data-style", /.*/);
  });

  test("conversation window theme calls changeChatThemeSetting", async ({
    page,
  }) => {
    await openAppearance(page);
    await page
      .getByRole("radiogroup", { name: "Conversation window theme" })
      .getByRole("radio", { name: "Light" })
      .click();
    await expect
      .poll(async () => (await getMockCalls(page, CHAT_THEME_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, CHAT_THEME_CALL))[0]).toEqual({
      chatTheme: "light",
    });
    await expect(
      page.getByText(
        "Drag the conversation window by its top edge. Press Escape, use the shortcut or the tray icon to hide it.",
      ),
    ).toBeVisible();
  });

  test("the transparency slider calls changeChatOpacitySetting once after the quiet period", async ({
    page,
  }) => {
    await openAppearance(page);
    // S4a housekeeping: the chat-window card is open by default (was collapsed), so no click.
    await expect(
      page.locator('[data-group-id="chatWindow"] button').first(),
    ).toHaveAttribute("aria-expanded", "true");
    const slider = page.locator('[data-testid="chat-opacity-slider"]');
    await expect(slider).toBeVisible();
    await slider.fill("50");
    await expect
      .poll(async () => (await getMockCalls(page, OPACITY_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, OPACITY_CALL))[0]).toEqual({ pct: 50 });
  });
});

test.describe("Appearance page: preview and reset", () => {
  test("preview follows the chosen accent on the primary button and the user bubble", async ({
    page,
  }) => {
    await openAppearance(page);
    await accentRadio(page, "Rose").click();
    const expected = hexToRgb(ACCENTS.rose.light.fillA);
    await expect(page.getByTestId("appearance-preview-primary")).toHaveCSS(
      "background-color",
      expected,
    );
    await expect(page.getByTestId("appearance-preview-user")).toHaveCSS(
      "background-color",
      expected,
    );
    await expect(page.getByTestId("appearance-preview-chat")).toHaveAttribute(
      "data-palette",
      "dark",
    );
    // The chat palette the preview shows is the one resolveAppearance gives for the choice.
    await expect(page.getByTestId("appearance-preview-chat")).toHaveCSS(
      "background-color",
      hexToRgb(CHAT_PALETTES.dark.surface),
    );
  });

  test("Reset needs a second confirm and then issues the default command set", async ({
    page,
  }) => {
    await openAppearance(page, {
      theme: "dark",
      accent: "blue",
      chat_theme: "light",
      visual_style: "restrained",
      chat_opacity_pct: 40,
    });
    await page
      .getByRole("button", { name: "Reset appearance to defaults" })
      .click();
    expect(await getMockCalls(page, THEME_CALL)).toHaveLength(0);
    expect(await getMockCalls(page, ACCENT_CALL)).toHaveLength(0);
    await page.getByRole("button", { name: "Confirm reset" }).click();
    await expect
      .poll(async () => (await getMockCalls(page, OPACITY_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, THEME_CALL)).at(-1)).toEqual({
      theme: "system",
    });
    expect((await getMockCalls(page, ACCENT_CALL)).at(-1)).toEqual({
      accent: "amber",
    });
    expect((await getMockCalls(page, CHAT_THEME_CALL)).at(-1)).toEqual({
      chatTheme: "dark",
    });
    expect((await getMockCalls(page, STYLE_CALL)).at(-1)).toEqual({
      visualStyle: "vibrant",
    });
    expect((await getMockCalls(page, OPACITY_CALL))[0]).toEqual({ pct: 70 });
    await expect(page.getByRole("status")).toContainText(
      "Appearance reset to defaults.",
    );
  });

  for (const [label, failingCommand] of [
    ["chat opacity", OPACITY_CALL],
    ["accent", ACCENT_CALL],
  ] as const) {
    test(`a failed ${label} write during Reset shows an error and no success message`, async ({
      page,
    }) => {
      await openAppearance(page, {
        theme: "dark",
        accent: "blue",
        chat_theme: "light",
        visual_style: "restrained",
        chat_opacity_pct: 40,
      });
      // Make exactly one reset write reject; the mock still records the others.
      await page.evaluate((command) => {
        const internals = (
          window as unknown as {
            __TAURI_INTERNALS__: {
              invoke: (cmd: string, args?: unknown, opts?: unknown) => unknown;
            };
          }
        ).__TAURI_INTERNALS__;
        const original = internals.invoke;
        internals.invoke = (cmd, args, opts) =>
          cmd === command
            ? Promise.reject(new Error("blocked by test"))
            : original(cmd, args, opts);
      }, failingCommand);
      await page
        .getByRole("button", { name: "Reset appearance to defaults" })
        .click();
      await page.getByRole("button", { name: "Confirm reset" }).click();
      await expect(page.getByRole("alert")).toContainText(
        "Some appearance settings could not be reset.",
      );
      await expect(page.getByText("Appearance reset to defaults.")).toHaveCount(
        0,
      );
      // The reset still attempts the remaining writes.
      await expect
        .poll(async () => (await getMockCalls(page, THEME_CALL)).length)
        .toBe(1);
    });
  }
});

test.describe("Appearance page: keyboard and layout", () => {
  test("the accent radio group is keyboard operable with roving focus", async ({
    page,
  }) => {
    await openAppearance(page);
    await accentRadio(page, "Amber").focus();
    await page.keyboard.press("ArrowRight");
    await expect
      .poll(async () => (await getMockCalls(page, ACCENT_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, ACCENT_CALL))[0]).toEqual({
      accent: "blue",
    });
    await expect(accentRadio(page, "Blue")).toBeFocused();
    await page.keyboard.press("End");
    expect((await getMockCalls(page, ACCENT_CALL)).at(-1)).toEqual({
      accent: "slate",
    });
    await page.keyboard.press("Home");
    expect((await getMockCalls(page, ACCENT_CALL)).at(-1)).toEqual({
      accent: "amber",
    });
    const ring = await accentRadio(page, "Amber").evaluate((el) => {
      const style = getComputedStyle(el);
      return `${style.outlineStyle} ${style.outlineWidth} ${style.boxShadow}`;
    });
    expect(ring).not.toBe("none 0px none");
  });

  test("no horizontal overflow at 760 x 600", async ({ page }) => {
    await page.setViewportSize({ width: 760, height: 600 });
    await openAppearance(page);
    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);
  });
});
