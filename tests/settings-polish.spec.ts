import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Settings polish (UI enterprise S4a, DESIGN Revision 3 U6-U9). Runs against the Vite dev server
 * with the Tauri mock. Every settings page has exactly one h1 (its title), the search placeholder
 * and the footer status fit at the two supported window sizes, and the version never shows as a
 * bare "v".
 */

const SIZES = [
  { width: 1100, height: 760 },
  { width: 760, height: 600 },
];

const PAGES = [
  "General",
  "Appearance",
  "History",
  "Models",
  "Brain",
  "Advanced",
  "About",
];

async function open(page: Page, size: { width: number; height: number }) {
  await page.setViewportSize(size);
  await installTauriMock(page, {
    settings: { brain_provider_id: "", brain_model: "" },
  });
  await page.goto("/");
  await expect(page.locator("p", { hasText: /^General$/ })).toBeVisible();
}

test.describe("settings polish", () => {
  test("every settings page has exactly one h1, and it is the page title", async ({
    page,
  }) => {
    await open(page, SIZES[0]);
    for (const label of PAGES) {
      await page.locator("p", { hasText: new RegExp(`^${label}$`) }).click();
      const headings = page.getByRole("heading", { level: 1 });
      await expect(headings).toHaveCount(1);
      await expect(headings).toHaveText(new RegExp(label, "i"));
    }
  });

  for (const size of SIZES) {
    test(`footer status is not clipped and the version never shows a bare v at ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await open(page, size);
      const status = page.locator('button[title^="Model status:"]');
      await expect(status).toBeVisible();
      const label = status.locator("span").first();
      const fits = await label.evaluate(
        (element) => element.scrollWidth <= element.clientWidth + 1,
      );
      expect(fits, "model status text is clipped").toBe(true);
      await expect(label).toHaveAttribute("title", /.+/);

      const footerText = await page
        .locator("div.border-t")
        .filter({ hasText: "Model" })
        .last()
        .innerText();
      expect(footerText).not.toMatch(/(^|\s)v(\s|$)/);
    });

    test(`the search placeholder fits the sidebar field at ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await open(page, size);
      const fits = await page
        .getByRole("combobox", { name: /search settings/i })
        .evaluate((input: HTMLInputElement) => {
          const style = getComputedStyle(input);
          const canvas = document.createElement("canvas").getContext("2d");
          if (!canvas) return false;
          canvas.font = style.font;
          const text = canvas.measureText(input.placeholder).width;
          const room =
            input.clientWidth -
            parseFloat(style.paddingLeft) -
            parseFloat(style.paddingRight);
          return text <= room;
        });
      expect(fits, "placeholder is cut").toBe(true);
    });
  }
});
