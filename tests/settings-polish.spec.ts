import { test, expect, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
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

/*
 * UI enterprise S4b (U16): narrow widths and long-string locales. Settings at 760x600 and the
 * General, Appearance and Models pages at 760 px with German and Russian (long strings) must not
 * scroll sideways and must not clip a label: a clipped element either wraps, or ellipsises with a
 * title tooltip.
 */
const NARROW = { width: 760, height: 600 };
const LONG_LOCALE_PAGES = ["general", "appearance", "models"] as const;

/** The sidebar label of a page in a locale, read from the shipped translation file. */
function sidebarLabel(language: string, key: string): string {
  const file = new URL(
    `../src/i18n/locales/${language}/translation.json`,
    import.meta.url,
  );
  const json = JSON.parse(readFileSync(file, "utf8")) as {
    sidebar: Record<string, string>;
  };
  return json.sidebar[key];
}

async function openLocale(
  page: Page,
  language: string,
  size: { width: number; height: number },
) {
  await page.setViewportSize(size);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      app_language: language,
    },
  });
  await page.goto("/");
  await expect(
    page
      .locator("p")
      .filter({ hasText: new RegExp(`^${sidebarLabel(language, "general")}$`) })
      .first(),
  ).toBeVisible();
}

async function horizontalOverflow(page: Page): Promise<boolean> {
  return page.evaluate(
    () =>
      document.documentElement.scrollWidth >
      document.documentElement.clientWidth + 1,
  );
}

async function clippedLabels(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const out: string[] = [];
    const selector = "label, button, span, p, h1, h2, h3";
    for (const el of Array.from(
      document.querySelectorAll<HTMLElement>(selector),
    )) {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) continue;
      const style = getComputedStyle(el);
      if (style.overflowX === "visible") continue;
      if (el.scrollWidth <= el.clientWidth + 1) continue;
      const ellipsised =
        style.textOverflow === "ellipsis" && el.title.length > 0;
      if (!ellipsised) out.push((el.textContent ?? "").trim().slice(0, 60));
    }
    return out;
  });
}

test.describe("settings narrow widths and long-string locales (U16)", () => {
  test("settings at 760x600 has no horizontal scroll and no clipped label", async ({
    page,
  }) => {
    await openLocale(page, "en", NARROW);
    for (const label of PAGES) {
      await page.locator("p", { hasText: new RegExp(`^${label}$`) }).click();
      expect(await horizontalOverflow(page), `${label} overflows`).toBe(false);
      expect(await clippedLabels(page), `${label} clips`).toEqual([]);
    }
  });

  for (const language of ["de", "ru"]) {
    for (const key of LONG_LOCALE_PAGES) {
      test(`${key} at 760 px in ${language} has no overflow and no clipped label`, async ({
        page,
      }) => {
        await openLocale(page, language, NARROW);
        const name = sidebarLabel(language, key);
        await page
          .locator("p")
          .filter({ hasText: new RegExp(`^${name}$`) })
          .first()
          .click();
        expect(
          await horizontalOverflow(page),
          `${key} ${language} overflows`,
        ).toBe(false);
        expect(await clippedLabels(page), `${key} ${language} clips`).toEqual(
          [],
        );
      });
    }
  }
});
