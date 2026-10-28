import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Layout guard for the Speech-to-text "Your models" list on the Models page.
 *
 * Regression: the list was rendered straight into the collapsible group body, which has no
 * padding of its own, so the model cards touched the left edge of the group card while the
 * Text-to-speech cards (wrapped in a padded SettingContainer) did not. These checks measure
 * real rendered boxes, because a class-name assertion would not catch a missing wrapper.
 *
 * Set VOX_SHOT_DIR to a folder to also save screenshots of the group.
 */

const STT_GROUP = "sttModels";
// Matches the inset the other rows in the same group use (px-3 = 12px). A little tolerance
// for borders, never less than this.
const MIN_INSET_PX = 12;
const MIN_ROW_GAP_PX = 8;

const NOT_INSTALLED = {
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
};

const BASE_MODEL = {
  description: "A model used by the layout test.",
  source: "Local",
  size_mb: 10,
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

const MODELS = [
  {
    ...BASE_MODEL,
    id: "model-a",
    name: "Layout Model Alpha",
    filename: "a.bin",
    is_downloaded: true,
  },
  {
    ...BASE_MODEL,
    id: "model-b",
    name: "Layout Model Bravo",
    filename: "b.bin",
    is_downloaded: true,
  },
  {
    ...BASE_MODEL,
    id: "model-c",
    name: "Layout Model Charlie",
    filename: "c.bin",
    is_downloaded: false,
  },
];

const group = (page: Page, id: string): Locator =>
  page.locator(`[data-group-id="${id}"]`);

// The row card is the nearest ancestor of the model name that carries the card radius.
const rowOf = (page: Page, name: string): Locator =>
  page
    .getByText(name, { exact: true })
    .first()
    .locator("xpath=ancestor::div[contains(@class,'rounded-xl')][1]");

async function box(locator: Locator) {
  const b = await locator.boundingBox();
  if (!b) throw new Error("element has no bounding box (not rendered?)");
  return b;
}

test.describe("Models page: Speech-to-text list layout", () => {
  test.beforeEach(async ({ page }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: false },
      commands: {
        get_available_models: MODELS,
        get_current_model: "model-a",
        phonon_voice_status: NOT_INSTALLED,
        natural_voice_status: NOT_INSTALLED,
      },
    });
    await page.goto("/");
    await page.locator("p", { hasText: /^Models$/ }).click();
    await expect(group(page, STT_GROUP)).toBeVisible();
    await expect(rowOf(page, "Layout Model Alpha")).toBeVisible();
    await expect(rowOf(page, "Layout Model Bravo")).toBeVisible();
  });

  test("model cards are inset from the left and right edge of their group", async ({
    page,
  }) => {
    const g = await box(group(page, STT_GROUP));
    const row = await box(rowOf(page, "Layout Model Alpha"));

    const leftInset = row.x - g.x;
    const rightInset = g.x + g.width - (row.x + row.width);

    if (process.env.VOX_SHOT_DIR) {
      await group(page, STT_GROUP).screenshot({
        path: `${process.env.VOX_SHOT_DIR}/stt-group.png`,
      });
    }

    expect(leftInset, `left inset was ${leftInset}px`).toBeGreaterThanOrEqual(
      MIN_INSET_PX,
    );
    expect(
      rightInset,
      `right inset was ${rightInset}px`,
    ).toBeGreaterThanOrEqual(MIN_INSET_PX);
  });

  test("the section heading and search box line up with the cards", async ({
    page,
  }) => {
    const search = await box(
      group(page, STT_GROUP)
        .getByPlaceholder(/search/i)
        .first(),
    );
    const row = await box(rowOf(page, "Layout Model Alpha"));
    // Both start at the same x, so the column reads as one edge.
    expect(Math.abs(search.x - row.x)).toBeLessThanOrEqual(1);
  });

  test("two downloaded models have a visible gap between them", async ({
    page,
  }) => {
    const a = await box(rowOf(page, "Layout Model Alpha"));
    const b = await box(rowOf(page, "Layout Model Bravo"));
    const first = a.y < b.y ? a : b;
    const second = a.y < b.y ? b : a;
    const gap = second.y - (first.y + first.height);

    expect(gap, `gap was ${gap}px`).toBeGreaterThanOrEqual(MIN_ROW_GAP_PX);
  });

  // Left edge of the first text node of an element, measured with a DOM Range so padding
  // on the element itself is accounted for.
  async function textLeft(locator: Locator): Promise<number> {
    return locator.evaluate((el) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      let node = walker.nextNode();
      while (node && !(node.textContent ?? "").trim()) node = walker.nextNode();
      if (!node) throw new Error("element has no text node");
      const range = document.createRange();
      range.selectNodeContents(node);
      return range.getBoundingClientRect().left;
    });
  }

  test("the model settings block is not a card nested inside the group", async ({
    page,
  }) => {
    // A nested .settings-card drew a second bordered box with its own accent line, flush
    // against the edges of the group.
    await expect(group(page, STT_GROUP).locator(".settings-card")).toHaveCount(
      0,
    );
  });

  test("the model settings heading lines up with the other sub-heading", async ({
    page,
  }) => {
    const g = await box(group(page, STT_GROUP));
    const modelHeading = group(page, STT_GROUP).getByText(
      "Layout Model Alpha Settings",
      { exact: true },
    );
    const otherHeading = group(page, STT_GROUP).getByText(
      "Transcription models",
      {
        exact: true,
      },
    );
    await expect(modelHeading).toBeVisible();
    const a = await textLeft(modelHeading);
    const b = await textLeft(otherHeading);

    expect(
      Math.abs(a - b),
      `headings at ${a}px and ${b}px`,
    ).toBeLessThanOrEqual(1);
    expect(a - g.x, `heading inset was ${a - g.x}px`).toBeGreaterThanOrEqual(
      MIN_INSET_PX,
    );
  });

  test("the language and translate rows still render in the group", async ({
    page,
  }) => {
    const rows = group(page, STT_GROUP).locator("[data-setting-title]");
    await expect(
      group(page, STT_GROUP).locator('[data-setting-title="Language"]'),
    ).toBeVisible();
    await expect(
      group(page, STT_GROUP).locator(
        '[data-setting-title="Translate to English"]',
      ),
    ).toBeVisible();
    expect(await rows.count()).toBeGreaterThanOrEqual(2);
  });
});
