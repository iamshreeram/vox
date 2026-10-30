import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Restrained visual style (UI enterprise S4a, DESIGN Revision 3 A5 / U5). Runs against the Vite
 * dev server with the Tauri mock. With data-style="restrained" the sidebar icons are neutral
 * (currentColor, accent only on the selected item), cards have no accent bar and no background
 * image, and the page background is flat. Vibrant (the default) keeps its look.
 */

const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });

// Each sidebar entry is a row: its icon (svg) and its label (p) share a parent.
const navRows = (page: Page): Locator =>
  page.locator("p.text-sm.font-medium.truncate[title]").locator("xpath=..");

async function openGeneral(
  page: Page,
  visualStyle: "vibrant" | "restrained",
): Promise<void> {
  await installTauriMock(page, { settings: { visual_style: visualStyle } });
  await page.goto("/");
  await expect(navItem(page, "General")).toBeVisible();
}

test.describe("restrained visual style", () => {
  test("restrained: sidebar icons match the label colour except the selected item", async ({
    page,
  }) => {
    await openGeneral(page, "restrained");
    await expect(page.locator("html")).toHaveAttribute(
      "data-style",
      "restrained",
    );
    const rows = navRows(page);
    const count = await rows.count();
    expect(count).toBeGreaterThanOrEqual(5);
    const result = await rows.evaluateAll((elements) =>
      elements.map((row) => {
        const icon = row.querySelector("svg");
        const label = row.querySelector("p");
        return {
          text: label?.textContent ?? "",
          icon: icon ? getComputedStyle(icon).color : "",
          label: label ? getComputedStyle(label).color : "",
          fill: getComputedStyle(row).backgroundColor,
        };
      }),
    );
    // The selected row is the only one with a tinted background (bg-accent-soft).
    const inactive = result.filter((row) => row.fill === "rgba(0, 0, 0, 0)");
    expect(inactive.length).toBeGreaterThanOrEqual(4);
    for (const row of inactive) {
      expect(row.icon, `icon of ${row.text}`).toBe(row.label);
    }
    const selected = result.filter((row) => row.fill !== "rgba(0, 0, 0, 0)");
    expect(selected.length).toBe(1);
  });

  test("restrained: cards have no accent bar, no background image; the page background is flat", async ({
    page,
  }) => {
    await openGeneral(page, "restrained");
    const card = page.locator(".settings-card").first();
    await expect(card).toBeVisible();
    const style = await card.evaluate((element) => {
      const before = getComputedStyle(element, "::before");
      const own = getComputedStyle(element);
      return {
        beforeContent: before.content,
        beforeBackground: before.backgroundImage,
        cardImage: own.backgroundImage,
        radius: own.borderRadius,
      };
    });
    expect(["none", "normal", ""]).toContain(style.beforeContent);
    expect(style.cardImage).toBe("none");
    // One consistent card radius (0.5rem at the app's 15px root size, about 7.5px).
    expect(parseFloat(style.radius)).toBeGreaterThanOrEqual(6);
    expect(parseFloat(style.radius)).toBeLessThanOrEqual(10);

    const html = await page.evaluate(() => ({
      image: getComputedStyle(document.documentElement).backgroundImage,
      body: getComputedStyle(document.body).backgroundImage,
    }));
    expect(html.image).toBe("none");
    expect(html.body).toBe("none");
  });

  test("vibrant (the default) keeps the warm page gradient, the accent bar and the per-section hues", async ({
    page,
  }) => {
    await openGeneral(page, "vibrant");
    await expect(page.locator("html")).not.toHaveAttribute("data-style", /.+/);
    const bar = await page
      .locator(".settings-card")
      .first()
      .evaluate((element) => getComputedStyle(element, "::before").content);
    expect(bar).not.toBe("none");
    const background = await page.evaluate(
      () => getComputedStyle(document.documentElement).backgroundImage,
    );
    expect(background).toContain("gradient");
    const icons = await navRows(page).evaluateAll((rows) =>
      rows.map(
        (row) => getComputedStyle(row.querySelector("svg") as Element).color,
      ),
    );
    expect(new Set(icons).size).toBeGreaterThan(2);
  });
});
