/*
 * N1e spec: notice bubbles on the bubble page (DESIGN.md A-C3) against the Vite dev server with the
 * Tauri mock. Deliberately NO page.clock: the known `clock.pauseAt` harness failure on main must not
 * touch this file. Dismissal uses real short timers (the page's own tick, 4 s lifetime).
 * Authoritative: do not weaken an assertion; report a wrong one.
 *
 * Contract under test:
 *   - A snapshot item with kind "notice" and notice_code renders the localised text for that
 *     code, in a role="status" node with aria-live="polite".
 *   - An unknown or missing code renders the generic text. A raw code is never rendered.
 *   - The notice dismisses by its ttl (4 s) on the page.
 *   - With reduced motion the notice has no animation.
 */

import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

const PAGE = "/src/bubbles/index.html?instance=9";
const INSTANCE = 9;
const EVENT = "bubbles-state";
const NOTICE_TTL_MS = 4000;

const TTS_OFF_TEXT = "Spoken replies are off. Turn them on in Settings.";
const UNKNOWN_TEXT = "Vox cannot start a conversation right now.";

type NoticeWire = {
  id: number;
  role: "assistant";
  text: string;
  age_ms: number;
  ttl_ms: number;
  kind: "notice";
  notice_code: string;
};

function snapshot(rev: number, items: NoticeWire[]) {
  return { instance: INSTANCE, rev, anchor: "bottom", items };
}

function notice(code: string, ageMs = 0): NoticeWire {
  return {
    id: 0,
    role: "assistant",
    text: "",
    age_ms: ageMs,
    ttl_ms: NOTICE_TTL_MS,
    kind: "notice",
    notice_code: code,
  };
}

const bubbles = (page: Page): Locator => page.getByTestId("bubble");

async function open(page: Page): Promise<void> {
  await installTauriMock(page);
  await page.goto(PAGE);
  // The page is ready only after its listener is registered and bubbles_ready is sent.
  await expect.poll(() => getMockCalls(page, "bubbles_ready")).toHaveLength(1);
}

test.describe("Bubble notices", () => {
  test.use({ viewport: { width: 320, height: 240 } });

  test("a notice shows its localised text in a polite status node", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, [notice("tts_off")]));
    await expect(bubbles(page)).toHaveCount(1);
    const shown = bubbles(page).first();
    await expect(shown).toHaveText(TTS_OFF_TEXT);
    await expect(shown).toHaveAttribute("role", "status");
    await expect(shown).toHaveAttribute("aria-live", "polite");
    await expect(shown).toHaveAttribute("data-kind", "notice");
  });

  test("an unknown code shows the generic text, never the raw code", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, [notice("some_future_code")]));
    const shown = bubbles(page).first();
    await expect(shown).toHaveText(UNKNOWN_TEXT);
    await expect(page.getByText("some_future_code")).toHaveCount(0);
  });

  test("a notice is dismissed after 4 seconds", async ({ page }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, [notice("tts_off")]));
    await expect(bubbles(page)).toHaveCount(1);
    await expect(bubbles(page)).toHaveCount(0, {
      timeout: NOTICE_TTL_MS + 3000,
    });
  });

  test("reduced motion: the notice has no animation", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, [notice("composer_off")]));
    const shown = bubbles(page).first();
    await expect(shown).toHaveText("Set up a reply backend in Settings first.");
    const animation = await shown.evaluate(
      (node) => getComputedStyle(node).animationName,
    );
    expect(animation).toBe("none");
  });
});
