import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  getMockCalls,
  getMockEmits,
  installTauriMock,
} from "./fixtures/tauriMock";
import { ACCENTS } from "../src/styles/accents";

/*
 * Appearance > Orb colour (DESIGN Revision 3, D3, D4). The group is a radiogroup of "Auto" and
 * "Follow app colour" plus the six accents. Choosing one saves `orb_accent` through
 * change_orb_accent_setting ("auto" for Auto, "app" for Follow app colour, the accent id otherwise),
 * the committed snapshot is published to the consumer windows, the live preview ring
 * follows the choice, Reset returns it to follow, and the group is never disabled.
 */

const ORB_CALL = "change_orb_accent_setting";
const ACCENT_CALL = "change_accent_setting";
const THEME_CALL = "change_theme_setting";
const AUTO = "Auto (changes with what Vox is doing)";
const FOLLOW_APP = "Follow app colour";

const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });

async function openAppearance(
  page: Page,
  settings: Record<string, unknown> = {},
): Promise<void> {
  await installTauriMock(page, { settings });
  await page.goto("/");
  await navItem(page, "Appearance").click();
  await expect(section(page, "appearance")).toBeVisible();
}

const orbGroup = (page: Page): Locator =>
  page.getByRole("radiogroup", { name: "Orb colour" });
const orbRadio = (page: Page, name: string): Locator =>
  orbGroup(page).getByRole("radio", { name, exact: true });

const hexToRgb = (hex: string): string => {
  const n = parseInt(hex.slice(1), 16);
  return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
};

const ORB_NAMES = [
  AUTO,
  FOLLOW_APP,
  "Amber",
  "Blue",
  "Violet",
  "Emerald",
  "Rose",
  "Slate",
];

test.describe("Orb colour: structure", () => {
  test("a radiogroup named Orb colour with Auto first, then Follow app colour, then the six accents", async ({
    page,
  }) => {
    await openAppearance(page);
    await expect(orbGroup(page)).toBeVisible();
    const radios = orbGroup(page).getByRole("radio");
    await expect(radios).toHaveCount(8);
    for (let i = 0; i < ORB_NAMES.length; i += 1) {
      await expect(radios.nth(i)).toHaveAccessibleName(ORB_NAMES[i]);
    }
    // Auto is the default selection.
    await expect(orbRadio(page, AUTO)).toHaveAttribute("aria-checked", "true");
    await expect(orbRadio(page, AUTO)).toHaveAttribute("tabindex", "0");
  });

  test("every swatch is at least 24 by 24 px", async ({ page }) => {
    await openAppearance(page);
    const radios = orbGroup(page).getByRole("radio");
    const count = await radios.count();
    for (let i = 0; i < count; i += 1) {
      const box = await radios.nth(i).boundingBox();
      expect(box?.width ?? 0).toBeGreaterThanOrEqual(24);
      expect(box?.height ?? 0).toBeGreaterThanOrEqual(24);
    }
  });

  test("the choice is announced under the swatches (not colour alone)", async ({
    page,
  }) => {
    await openAppearance(page, { orb_accent: "rose" });
    await expect(orbRadio(page, "Rose")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(
      section(page, "appearance").getByText("Rose", { exact: true }),
    ).toBeVisible();
  });
});

test.describe("Orb colour: persistence", () => {
  test("choosing an accent calls changeOrbAccentSetting once with that accent", async ({
    page,
  }) => {
    await openAppearance(page);
    await orbRadio(page, "Blue").click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, ORB_CALL))[0]).toEqual({ color: "blue" });
    await expect(orbRadio(page, "Blue")).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  test("choosing Follow app colour sends app and is announced by a snapshot event", async ({
    page,
  }) => {
    await openAppearance(page, { orb_accent: "violet" });
    await orbRadio(page, FOLLOW_APP).click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, ORB_CALL))[0]).toEqual({ color: "app" });
    await expect
      .poll(
        async () =>
          (await getMockEmits(page)).filter(
            (emit) => emit.label === "orb" && emit.event === "appearance",
          ).length,
      )
      .toBeGreaterThan(0);
    const last = (await getMockEmits(page)).filter(
      (emit) => emit.label === "orb" && emit.event === "appearance",
    );
    expect(last.at(-1)?.payload).toMatchObject({ orb_accent: "app" });
  });

  test("the orb colour does not change the app accent", async ({ page }) => {
    await openAppearance(page);
    await orbRadio(page, "Emerald").click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    expect(await getMockCalls(page, ACCENT_CALL)).toHaveLength(0);
  });
});

test.describe("Orb colour: keyboard", () => {
  test("arrow keys, Home and End move and choose with roving focus", async ({
    page,
  }) => {
    await openAppearance(page);
    await orbRadio(page, AUTO).focus();
    await page.keyboard.press("ArrowRight");
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, ORB_CALL))[0]).toEqual({ color: "app" });
    await expect(orbRadio(page, FOLLOW_APP)).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(2);
    expect((await getMockCalls(page, ORB_CALL))[1]).toEqual({
      color: "amber",
    });
    await expect(orbRadio(page, "Amber")).toBeFocused();
    await page.keyboard.press("End");
    expect((await getMockCalls(page, ORB_CALL)).at(-1)).toEqual({
      color: "slate",
    });
    await page.keyboard.press("Home");
    expect((await getMockCalls(page, ORB_CALL)).at(-1)).toEqual({
      color: "auto",
    });
    await expect(orbRadio(page, AUTO)).toBeFocused();
  });

  test("the focused swatch shows a visible focus ring", async ({ page }) => {
    await openAppearance(page);
    await orbRadio(page, AUTO).focus();
    const ring = await orbRadio(page, AUTO).evaluate((el) => {
      const style = getComputedStyle(el);
      return `${style.outlineStyle} ${style.outlineWidth} ${style.boxShadow}`;
    });
    expect(ring).not.toBe("none 0px none");
  });
});

test.describe("Orb colour: live preview", () => {
  test("the preview ring follows the chosen orb accent (light bucket uses the text role)", async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await openAppearance(page);
    const orb = page.getByTestId("appearance-preview-orb");
    for (const name of ["Rose", "Blue"] as const) {
      const id = name.toLowerCase() as keyof typeof ACCENTS;
      await orbRadio(page, name).click();
      const expected = hexToRgb(ACCENTS[id].light.text);
      await expect(orb).toHaveAttribute("data-ring", ACCENTS[id].light.text);
      await expect(orb.locator(".vox-ring-1")).toHaveCSS(
        "stroke",
        new RegExp(expected.replace(/[()]/g, "\\$&")),
      );
    }
  });

  test("Follow app colour makes the preview ring use the app accent", async ({
    page,
  }) => {
    await page.emulateMedia({ colorScheme: "light" });
    await openAppearance(page, { accent: "rose", orb_accent: "blue" });
    const orb = page.getByTestId("appearance-preview-orb");
    await expect(orb).toHaveAttribute("data-ring", ACCENTS.blue.light.text);
    await orbRadio(page, FOLLOW_APP).click();
    // Follow app draws the ring from the text role on light (A6): the reported ring is that token,
    // and the drawn .vox-ring-1 stroke is the rose text role (V3).
    await expect(orb).toHaveAttribute("data-ring", "var(--color-accent-text)");
    await expect(orb.locator(".vox-ring-1")).toHaveCSS(
      "stroke",
      new RegExp(hexToRgb(ACCENTS.rose.light.text).replace(/[()]/g, "\\$&")),
    );
  });
});

test.describe("Orb colour: themes, reset and failure", () => {
  test("is not disabled under Aurora or Custom", async ({ page }) => {
    for (const theme of ["aurora", "custom"]) {
      await openAppearance(page, { theme });
      await expect(orbRadio(page, "Blue")).toBeEnabled();
      await expect(orbGroup(page)).not.toHaveAttribute("aria-disabled", "true");
      await orbRadio(page, "Blue").click();
      await expect
        .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
        .toBe(1);
      expect((await getMockCalls(page, ORB_CALL))[0]).toEqual({
        color: "blue",
      });
      await page.goto("/");
    }
  });

  test("Reset to defaults returns the orb colour to Auto", async ({ page }) => {
    await openAppearance(page, { orb_accent: "blue" });
    await page
      .getByRole("button", { name: "Reset appearance to defaults" })
      .click();
    await page.getByRole("button", { name: "Confirm reset" }).click();
    await expect
      .poll(async () => (await getMockCalls(page, ORB_CALL)).length)
      .toBe(1);
    expect((await getMockCalls(page, ORB_CALL))[0]).toEqual({ color: "auto" });
    await expect(orbRadio(page, AUTO)).toHaveAttribute("aria-checked", "true");
    expect((await getMockCalls(page, THEME_CALL)).at(-1)).toEqual({
      theme: "system",
    });
  });

  test("a failed save shows an error, rolls back, and keeps the previous choice", async ({
    page,
  }) => {
    await openAppearance(page, { orb_accent: "rose" });
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
    }, ORB_CALL);
    await orbRadio(page, "Blue").click();
    await expect(page.getByRole("alert")).toContainText(
      "Could not save the orb colour.",
    );
    await expect(orbRadio(page, "Rose")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(orbRadio(page, "Blue")).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });
});

test.describe("Orb colour: layout and copy", () => {
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

  test("the floating orb settings say a click starts a conversation and point to Orb colour", async ({
    page,
  }) => {
    await installTauriMock(page, {
      commands: { get_orb_support: { supported: true, block: null } },
    });
    await page.goto("/");
    await navItem(page, "General").click();
    await expect(section(page, "general")).toBeVisible();
    const header = page
      .locator('[data-group-id="conversation"]')
      .getByRole("button")
      .first();
    await header.click();
    await expect(header).toHaveAttribute("aria-expanded", "true");
    await expect(
      page.getByText(
        "A small orb that stays on top of other apps. Click it to start or stop a voice conversation.",
      ),
    ).toBeVisible();
    await expect(
      page.getByText(
        "To change the orb's colour, open Appearance > Orb colour.",
      ),
    ).toBeVisible();
  });
});
