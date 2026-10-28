import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * G4b SPEC (DESIGN.md R6-4, A7-2, A8-2, A8-3, Revision 10 item 4): the bubble page
 * (src/bubbles/index.html, instance 7) against the Vite dev server with the Tauri mock.
 * Written first; workers must not edit this file.
 *
 * Contract under test:
 *   - The page registers its listener for the Tauri event `bubbles-state` FIRST, then invokes
 *     `bubbles_ready` with {instance: 7} exactly once. It invokes no other app command (only
 *     the event plugin's own plugin:event|* commands are allowed).
 *   - Snapshot payload: { instance, rev, items: [{ id, role, text, age_ms, ttl_ms }] }, oldest
 *     first. Applied only when instance matches and rev is greater than the last applied rev.
 *   - Stable selectors: container role="log" (aria-live="polite", aria-atomic="false");
 *     each bubble data-testid="bubble" with data-role="user" | "assistant".
 *   - Lifetime: opacity 1 until ttl-600, then linear to 0; removed at ttl. Reduced motion:
 *     no transition/animation, no opacity ramp, removed at ttl.
 *
 * Assumptions (see the report): the page advances with Playwright's fake clock (page.clock),
 * so timers and Date.now move together; the page's own tick is at most every 50 ms; CSS
 * transitions run in real time, so opacity reads are polled.
 */

const PAGE = "/src/bubbles/index.html?instance=7";
const INSTANCE = 7;
const EVENT = "bubbles-state";
const T0 = new Date("2026-01-01T00:00:00Z");

type WireItem = {
  id: number;
  role: "user" | "assistant";
  text: string;
  age_ms: number;
  ttl_ms: number;
};

function snapshot(rev: number, items: WireItem[], instance = INSTANCE) {
  return { instance, rev, items };
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

async function opacityOf(locator: Locator): Promise<number> {
  return locator.evaluate((el) => Number(getComputedStyle(el).opacity));
}

test.describe("Bubble page", () => {
  test.use({ viewport: { width: 320, height: 240 } });

  test("handshake: listens for bubbles-state first, then calls bubbles_ready(7) once, nothing else", async ({
    page,
  }) => {
    await open(page);
    const calls = await page.evaluate(() => {
      const mock = (
        window as unknown as {
          __voxMock: { calls: Array<{ command: string; args: unknown }> };
        }
      ).__voxMock;
      return mock.calls.map((c) => ({ command: c.command, args: c.args }));
    });
    const listenIndex = calls.findIndex(
      (c) =>
        c.command === "plugin:event|listen" &&
        (c.args as { event?: unknown }).event === EVENT,
    );
    const readyIndex = calls.findIndex((c) => c.command === "bubbles_ready");
    expect(
      listenIndex,
      "listener for bubbles-state registered",
    ).toBeGreaterThanOrEqual(0);
    expect(readyIndex, "bubbles_ready invoked").toBeGreaterThan(listenIndex);
    expect(calls.filter((c) => c.command === "bubbles_ready")).toHaveLength(1);
    expect(calls[readyIndex].args).toEqual({ instance: INSTANCE });

    const nonEvent = calls
      .map((c) => c.command)
      .filter((name) => !name.startsWith("plugin:event|"));
    expect(nonEvent).toEqual(["bubbles_ready"]);
  });

  test("user bubbles are right-aligned and assistant bubbles left-aligned, newest last", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        {
          id: 1,
          role: "user",
          text: "older question",
          age_ms: 0,
          ttl_ms: 9000,
        },
        {
          id: 2,
          role: "assistant",
          text: "newest answer",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );
    const items = bubbles(page);
    await expect(items).toHaveCount(2);
    await expect(items.nth(0)).toHaveAttribute("data-role", "user");
    await expect(items.nth(1)).toHaveAttribute("data-role", "assistant");
    await expect(items.nth(1)).toContainText("newest answer");
    await expect(items.nth(0)).toContainText("older question");

    const box = await log(page).boundingBox();
    const userBox = await items.nth(0).boundingBox();
    const assistantBox = await items.nth(1).boundingBox();
    expect(box && userBox && assistantBox).toBeTruthy();
    const mid =
      (box as { x: number; width: number }).x +
      (box as { width: number }).width / 2;
    const userCentre =
      (userBox as { x: number; width: number }).x +
      (userBox as { width: number }).width / 2;
    const assistantCentre =
      (assistantBox as { x: number; width: number }).x +
      (assistantBox as { width: number }).width / 2;
    expect(userCentre, "user bubble sits on the right").toBeGreaterThan(mid);
    expect(assistantCentre, "assistant bubble sits on the left").toBeLessThan(
      mid,
    );
  });

  test("at most 3 bubbles are shown: exactly the 3 newest", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "user", text: "first line", age_ms: 0, ttl_ms: 9000 },
        {
          id: 2,
          role: "assistant",
          text: "second line",
          age_ms: 0,
          ttl_ms: 9000,
        },
        { id: 3, role: "user", text: "third line", age_ms: 0, ttl_ms: 9000 },
        {
          id: 4,
          role: "assistant",
          text: "fourth line",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );
    await expect(bubbles(page)).toHaveCount(3);
    const texts = await bubbles(page).evaluateAll((els) =>
      els.map((el) => el.textContent ?? ""),
    );
    expect(texts[0]).toContain("second line");
    expect(texts[2]).toContain("fourth line");
    expect(texts.join("|")).not.toContain("first line");
  });

  test("a stale instance and a non-increasing rev are ignored", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(2, [
        {
          id: 1,
          role: "assistant",
          text: "kept text",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );
    await expect(bubbles(page)).toContainText("kept text");

    // Another window instance: ignored, even with a higher rev.
    await emitMockEvent(
      page,
      EVENT,
      snapshot(
        3,
        [
          {
            id: 2,
            role: "assistant",
            text: "stale instance text",
            age_ms: 0,
            ttl_ms: 9000,
          },
        ],
        6,
      ),
    );
    // Same rev again, and an older rev: both ignored.
    await emitMockEvent(
      page,
      EVENT,
      snapshot(2, [
        {
          id: 3,
          role: "assistant",
          text: "same rev text",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        {
          id: 4,
          role: "assistant",
          text: "old rev text",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );

    await expect(bubbles(page)).toHaveCount(1);
    await expect(bubbles(page)).toContainText("kept text");
    const body = await page.evaluate(() => document.body.textContent ?? "");
    expect(body).not.toContain("stale instance text");
    expect(body).not.toContain("same rev text");
    expect(body).not.toContain("old rev text");
  });

  test("an empty snapshot removes every bubble", async ({ page }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "user", text: "to be cleared", age_ms: 0, ttl_ms: 9000 },
      ]),
    );
    await expect(bubbles(page)).toHaveCount(1);
    await emitMockEvent(page, EVENT, snapshot(2, []));
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("markup in text renders literally: no element, no dialog", async ({
    page,
  }) => {
    const dialogs: string[] = [];
    page.on("dialog", (dialog) => {
      dialogs.push(dialog.message());
      void dialog.dismiss();
    });
    await open(page);
    const hostile = "<img src=x onerror=alert(1)><script>";
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "assistant", text: hostile, age_ms: 0, ttl_ms: 9000 },
      ]),
    );
    await expect(bubbles(page)).toContainText(hostile);
    expect(await log(page).locator("img, script").count()).toBe(0);
    expect(
      await page.locator("script[data-injected], img[src='x']").count(),
    ).toBe(0);
    expect(dialogs).toEqual([]);
  });

  test("a 240-character text is clamped to 4 lines and three maximal bubbles fit 320x240", async ({
    page,
  }) => {
    await open(page);
    const long = Array.from({ length: 240 }, (_, i) =>
      i % 7 === 6 ? " " : "a",
    ).join("");
    expect(long.length).toBe(240);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "assistant", text: long, age_ms: 0, ttl_ms: 60000 },
        { id: 2, role: "user", text: long, age_ms: 0, ttl_ms: 60000 },
        { id: 3, role: "assistant", text: long, age_ms: 0, ttl_ms: 60000 },
      ]),
    );
    await expect(bubbles(page).last()).toBeVisible();

    // Clamp: one bubble is at most 4 line-heights plus its vertical padding (and 1 px slack).
    const clamp = await bubbles(page)
      .first()
      .evaluate((el) => {
        const style = getComputedStyle(el);
        const size = parseFloat(style.fontSize);
        const lineHeight = Number.parseFloat(style.lineHeight) || size * 1.2;
        const padding =
          parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
        return { limit: 4 * lineHeight + padding + 1, height: el.clientHeight };
      });
    expect(clamp.height).toBeLessThanOrEqual(clamp.limit);

    // Fit: every shown bubble is inside the viewport, and the newest is fully inside.
    const count = await bubbles(page).count();
    for (let i = 0; i < count; i += 1) {
      const box = await bubbles(page).nth(i).boundingBox();
      expect(box, `bubble ${i} has a box`).not.toBeNull();
      const b = box as { x: number; y: number; width: number; height: number };
      expect(b.x, `bubble ${i} left edge`).toBeGreaterThanOrEqual(0);
      expect(b.y, `bubble ${i} top edge`).toBeGreaterThanOrEqual(0);
      expect(b.x + b.width, `bubble ${i} right edge`).toBeLessThanOrEqual(
        320.5,
      );
      expect(b.y + b.height, `bubble ${i} bottom edge`).toBeLessThanOrEqual(
        240.5,
      );
    }
    const overflow = await page.evaluate(() => ({
      w: document.documentElement.scrollWidth,
      h: document.documentElement.scrollHeight,
    }));
    expect(overflow.w).toBeLessThanOrEqual(320);
    expect(overflow.h).toBeLessThanOrEqual(240);
  });

  test("fade: opacity 1 until ttl-600, linear to 0, removed exactly at ttl", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "assistant", text: "fading", age_ms: 0, ttl_ms: 4000 },
      ]),
    );
    const bubble = bubbles(page).first();
    await expect(bubble).toBeVisible();

    await advance(page, 3000); // age 3000 < 3400: fully opaque
    await expect.poll(() => opacityOf(bubble)).toBeCloseTo(1, 2);

    await advance(page, 700); // age 3700: halfway through the 600 ms fade
    await expect.poll(() => opacityOf(bubble)).toBeCloseTo(0.5, 1);

    await advance(page, 299); // age 3999: still present
    await expect(bubbles(page)).toHaveCount(1);

    await advance(page, 1); // age 4000 = ttl: removed
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("a snapshot with age_ms keeps the original lifetime (no restart)", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "user", text: "replayed", age_ms: 3000, ttl_ms: 4000 },
      ]),
    );
    await expect(bubbles(page)).toHaveCount(1);
    await advance(page, 900); // age 3900: still alive
    await expect(bubbles(page)).toHaveCount(1);
    await advance(page, 100); // age 4000: gone, not 4000 ms after arrival
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("replay after a reload does not restart the lifetime", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        {
          id: 1,
          role: "assistant",
          text: "before reload",
          age_ms: 0,
          ttl_ms: 4000,
        },
      ]),
    );
    await advance(page, 2000);
    await page.reload();
    await expect
      .poll(() => getMockCalls(page, "bubbles_ready"))
      .toHaveLength(1);
    const now = await page.evaluate(() => Date.now());
    await page.clock.pauseAt(new Date(now + 1000));
    // Rust resends the snapshot with the age the bubble already has.
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        {
          id: 1,
          role: "assistant",
          text: "replayed",
          age_ms: 2000,
          ttl_ms: 4000,
        },
      ]),
    );
    await expect(bubbles(page)).toHaveCount(1);
    await advance(page, 1999);
    await expect(bubbles(page)).toHaveCount(1);
    await advance(page, 1);
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("reduced motion: no transition or animation, no opacity ramp, removed at ttl", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "assistant", text: "still", age_ms: 0, ttl_ms: 4000 },
      ]),
    );
    const bubble = bubbles(page).first();
    await expect(bubble).toBeVisible();

    const motion = await bubble.evaluate((el) => {
      const style = getComputedStyle(el);
      return {
        animation: style.animationName,
        transitionProperty: style.transitionProperty,
        transitionDurations: style.transitionDuration
          .split(",")
          .map((d) => d.trim()),
      };
    });
    expect(motion.animation).toBe("none");
    const noTransition =
      motion.transitionProperty === "none" ||
      motion.transitionDurations.every((d) => parseFloat(d) === 0);
    expect(noTransition, JSON.stringify(motion)).toBe(true);

    await advance(page, 3900);
    await expect(bubbles(page)).toHaveCount(1);
    expect(await opacityOf(bubble)).toBe(1);

    await advance(page, 100);
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("the container is a polite, non-atomic log region", async ({ page }) => {
    await open(page);
    await expect(log(page)).toHaveCount(1);
    await expect(log(page)).toHaveAttribute("aria-live", "polite");
    await expect(log(page)).toHaveAttribute("aria-atomic", "false");
  });

  test("bubbles are not focusable and contain no controls", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        {
          id: 1,
          role: "user",
          text: "no controls here",
          age_ms: 0,
          ttl_ms: 9000,
        },
      ]),
    );
    const bubble = bubbles(page).first();
    await expect(bubble).toBeVisible();
    await expect(bubble).not.toHaveAttribute("tabindex");
    expect(
      await bubbles(page)
        .locator("button, a, input, select, textarea, [href], [tabindex]")
        .count(),
    ).toBe(0);
  });

  test("body and html backgrounds are fully transparent", async ({ page }) => {
    await open(page);
    const backgrounds = await page.evaluate(() => ({
      html: getComputedStyle(document.documentElement).backgroundColor,
      body: getComputedStyle(document.body).backgroundColor,
    }));
    expect(backgrounds.html).toBe("rgba(0, 0, 0, 0)");
    expect(backgrounds.body).toBe("rgba(0, 0, 0, 0)");
  });

  test("bubble fill is translucent (alpha 0.6 to 0.95) with no blur or filter", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(
      page,
      EVENT,
      snapshot(1, [
        { id: 1, role: "assistant", text: "glass", age_ms: 0, ttl_ms: 9000 },
      ]),
    );
    const bubble = bubbles(page).first();
    await expect(bubble).toBeVisible();
    const style = await bubble.evaluate((el) => {
      const cs = getComputedStyle(el);
      return {
        background: cs.backgroundColor,
        backdrop: cs.backdropFilter,
        filter: cs.filter,
      };
    });
    const match = /^rgba\(([^)]+)\)$/.exec(style.background);
    expect(
      match,
      `translucent colour expected, got ${style.background}`,
    ).not.toBeNull();
    const parts = (match as RegExpExecArray)[1].split(",").map((p) => p.trim());
    expect(parts.length).toBe(4);
    const alpha = Number(parts[3]);
    expect(alpha).toBeGreaterThanOrEqual(0.6);
    expect(alpha).toBeLessThanOrEqual(0.95);
    expect(style.backdrop).toBe("none");
    expect(style.filter).toBe("none");
  });

  test("works in the browser preview with no Tauri runtime", async ({
    page,
  }) => {
    const warnings: string[] = [];
    const errors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "warning") warnings.push(msg.text());
    });
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(PAGE);
    await expect(log(page)).toHaveCount(1);
    expect(errors).toEqual([]);
    expect(warnings.length).toBeLessThanOrEqual(1);
  });
});

test("the vite build has a 'bubbles' entry pointing at src/bubbles/index.html", () => {
  const config = readFileSync(
    new URL("../vite.config.ts", import.meta.url),
    "utf8",
  );
  expect(config).toMatch(
    /bubbles:\s*resolve\(\s*__dirname\s*,\s*["']src\/bubbles\/index\.html["']\s*\)/,
  );
});
