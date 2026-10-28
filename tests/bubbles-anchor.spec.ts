import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * Issue #113 SPEC: the bubble stack anchor (src/bubbles/index.html, instance 7) against the Vite
 * dev server with the Tauri mock. Helpers are copied from bubbles-page.spec.ts, which stays
 * unedited. Authoritative: do not weaken an assertion; report a wrong one.
 *
 * Contract under test:
 *   - Snapshot payload may carry `anchor: "top" | "bottom"`. Missing or unknown means "bottom".
 *   - anchor "top": the first bubble's top edge is within 16 px of the window top (the stack
 *     hugs the top, newest last).
 *   - anchor "bottom" or missing: the last bubble's bottom edge is within 16 px of the window
 *     bottom.
 *   - A change of anchor with the same revision keeps the same DOM nodes and does not restart
 *     any lifetime.
 */

const PAGE = "/src/bubbles/index.html?instance=7";
const INSTANCE = 7;
const EVENT = "bubbles-state";
const T0 = new Date("2026-01-01T00:00:00Z");
const EDGE_PX = 16;

type WireItem = {
  id: number;
  role: "user" | "assistant";
  text: string;
  age_ms: number;
  ttl_ms: number;
};

function snapshot(
  rev: number,
  items: WireItem[],
  anchor?: string,
  instance = INSTANCE,
) {
  return anchor === undefined
    ? { instance, rev, items }
    : { instance, rev, anchor, items };
}

const bubbles = (page: Page): Locator => page.getByTestId("bubble");
const log = (page: Page): Locator => page.getByRole("log");

/** Moves the fake clock forward in 50 ms steps so the page's ticks see every step. */
async function advance(page: Page, ms: number): Promise<void> {
  for (let left = ms; left > 0; left -= 50) {
    await page.clock.runFor(Math.min(50, left));
  }
}

async function open(page: Page): Promise<void> {
  await installTauriMock(page);
  await page.clock.install({ time: T0 });
  await page.goto(PAGE);
  // The page is ready only after its listener is registered and bubbles_ready is sent.
  await expect.poll(() => getMockCalls(page, "bubbles_ready")).toHaveLength(1);
  await page.clock.pauseAt(new Date(T0.getTime() + 1000));
}

const twoBubbles: WireItem[] = [
  { id: 1, role: "user", text: "older question", age_ms: 0, ttl_ms: 9000 },
  { id: 2, role: "assistant", text: "newest answer", age_ms: 0, ttl_ms: 9000 },
];

test.describe("Bubble stack anchor", () => {
  test.use({ viewport: { width: 320, height: 240 } });

  test("anchor top: the first bubble hugs the top edge of the window", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, twoBubbles, "top"));
    await expect(bubbles(page)).toHaveCount(2);
    await expect(log(page)).toHaveAttribute("data-anchor", "top");

    const first = await bubbles(page).first().boundingBox();
    expect(first, "first bubble has a box").toBeTruthy();
    const top = (first as { y: number }).y;
    expect(
      top,
      "top edge is within 16 px of the window top",
    ).toBeLessThanOrEqual(EDGE_PX);
    expect(top).toBeGreaterThanOrEqual(0);

    // Newest is still the last item, nearest the orb in the stack order.
    await expect(bubbles(page).last()).toContainText("newest answer");
  });

  test("anchor bottom: the last bubble hugs the bottom edge of the window", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, twoBubbles, "bottom"));
    await expect(bubbles(page)).toHaveCount(2);
    await expect(log(page)).toHaveAttribute("data-anchor", "bottom");

    const last = await bubbles(page).last().boundingBox();
    expect(last, "last bubble has a box").toBeTruthy();
    const bottom = last as { y: number; height: number };
    const gap = 240 - (bottom.y + bottom.height);
    expect(
      gap,
      "bottom edge is within 16 px of the window bottom",
    ).toBeLessThanOrEqual(EDGE_PX);
    expect(gap).toBeGreaterThanOrEqual(0);
  });

  test("a missing anchor behaves as bottom (old payloads unchanged)", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, twoBubbles));
    await expect(bubbles(page)).toHaveCount(2);
    await expect(log(page)).toHaveAttribute("data-anchor", "bottom");

    const last = await bubbles(page).last().boundingBox();
    const bottom = last as { y: number; height: number };
    expect(240 - (bottom.y + bottom.height)).toBeLessThanOrEqual(EDGE_PX);
  });

  test("an unknown anchor value behaves as bottom", async ({ page }) => {
    await open(page);
    await emitMockEvent(page, EVENT, snapshot(1, twoBubbles, "sideways"));
    await expect(bubbles(page)).toHaveCount(2);
    await expect(log(page)).toHaveAttribute("data-anchor", "bottom");
  });

  test("switching the anchor with the same rev keeps the DOM nodes and the lifetime", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(
        1,
        [{ id: 1, role: "user", text: "steady", age_ms: 0, ttl_ms: 4000 }],
        "bottom",
      ),
    );
    await expect(bubbles(page)).toHaveCount(1);
    // Tag the node so a re-mount is visible.
    await bubbles(page)
      .first()
      .evaluate((el) => {
        el.setAttribute("data-probe", "original");
      });

    await advance(page, 2000);
    // Same revision, new anchor: the orb moved to the other side of the screen.
    await emitMockEvent(
      page,
      EVENT,
      snapshot(
        1,
        [{ id: 1, role: "user", text: "steady", age_ms: 2000, ttl_ms: 4000 }],
        "top",
      ),
    );
    await expect(log(page)).toHaveAttribute("data-anchor", "top");
    await expect(bubbles(page).first()).toHaveAttribute(
      "data-probe",
      "original",
    );
    await expect(bubbles(page)).toHaveCount(1);

    // The lifetime was not restarted: gone at 4000 ms of age, not 4000 ms after the switch.
    await advance(page, 1999);
    await expect(bubbles(page)).toHaveCount(1);
    await advance(page, 1);
    await expect(bubbles(page)).toHaveCount(0);
  });
});
