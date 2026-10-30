/*
 * Orb accent and refusal e2e (DESIGN Revision 3, D3 and A-C2/A-C3) against the Vite dev server with
 * the Tauri mock. Deliberately NO page.clock (the known clock.pauseAt harness failure must not touch
 * this file). Authoritative: do not weaken an assertion; report a wrong one.
 *
 * Covered:
 *   - the appearance event with orb_accent changes the ring colour (computed stroke) in light and dark
 *   - orb_accent null follows the app accent again
 *   - orb-refusal {code} shows the localised sr-only status text and the concerned face, then clears
 *   - a garbage orb-refusal payload shows the generic text, never a raw code
 */
import { test, expect, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

const ORB_PAGE = "/src/orb/index.html";
const ORB = '[data-testid="orb"]';
const STATUS = '[data-testid="orb-refusal-status"]';

/** The ring colour is `stroke: var(--chat-accent)` on .vox-ring. */
const RING = `${ORB} .vox-ring`;

function snapshot(over: Record<string, unknown>) {
  return {
    rev: 1,
    theme: "dark",
    custom_colors: null,
    accent: "amber",
    chat_theme: "dark",
    visual_style: "vibrant",
    orb_accent: null,
    ...over,
  };
}

async function open(page: Page): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face: "idle" } },
  });
  await page.goto(ORB_PAGE);
  await expect(page.locator(ORB)).toBeVisible();
}

async function ringStroke(page: Page): Promise<string> {
  return page.evaluate(
    (selector) =>
      getComputedStyle(document.querySelector(selector) as Element).stroke,
    RING,
  );
}

const rgb = (hex: string): string => {
  const n = parseInt(hex.slice(1), 16);
  return `rgb(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255})`;
};

test.describe("orb accent", () => {
  test("an orb accent changes the ring colour in light and in dark", async ({
    page,
  }) => {
    await open(page);
    // Dark: violet's dark fillA (#a78bfa).
    await emitMockEvent(
      page,
      "appearance",
      snapshot({ rev: 2, theme: "dark", orb_accent: "violet" }),
    );
    await expect.poll(() => ringStroke(page)).toBe(rgb("#a78bfa"));

    // Light: violet's light text role (#6d28d9).
    await emitMockEvent(
      page,
      "appearance",
      snapshot({ rev: 3, theme: "light", orb_accent: "violet" }),
    );
    await expect.poll(() => ringStroke(page)).toBe(rgb("#6d28d9"));
  });

  test("a null orb accent follows the app accent again", async ({ page }) => {
    await open(page);
    await emitMockEvent(
      page,
      "appearance",
      snapshot({ rev: 2, theme: "dark", accent: "blue", orb_accent: "rose" }),
    );
    await expect.poll(() => ringStroke(page)).toBe(rgb("#fb7185"));
    await emitMockEvent(
      page,
      "appearance",
      snapshot({ rev: 3, theme: "dark", accent: "blue", orb_accent: null }),
    );
    // Blue's dark fillA (#60a5fa): the app accent, not the orb accent.
    await expect.poll(() => ringStroke(page)).toBe(rgb("#60a5fa"));
  });
});

test.describe("orb refusal", () => {
  test("a refusal shows the status text and the concerned face, then clears", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, "orb-refusal", { code: "dictation_busy" });
    const status = page.locator(STATUS);
    await expect(status).toHaveText("Dictation is using the microphone");
    await expect(page.locator(`${ORB} .vox-avatar`)).toHaveAttribute(
      "data-face",
      "concerned",
    );
    expect(await page.content()).not.toContain("dictation_busy");
    // The face returns after about 1.2 s; the text clears after 4 s.
    await expect(page.locator(`${ORB} .vox-avatar`)).not.toHaveAttribute(
      "data-face",
      "concerned",
      { timeout: 3000 },
    );
    await expect(status).toHaveText("", { timeout: 6000 });
  });

  test("a garbage payload shows the generic text and never a raw code", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, "orb-refusal", { code: "<b>not a code</b>" });
    const status = page.locator(STATUS);
    await expect(status).toHaveText("Could not start the conversation");
    expect(await page.content()).not.toContain("not a code");
  });
});
