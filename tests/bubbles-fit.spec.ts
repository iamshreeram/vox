import { mkdirSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * T4 bubble fit (DESIGN.md Revision 4, B3). The overlay's reply bubbles must clamp to whole
 * lines only, signal truncation with an ellipsis, keep the full message in the DOM for screen
 * readers, stay non-interactive, and stay inside the overlay window at the widths it uses.
 *
 * Widths: 200 (MIN_BUBBLE_W_LOGICAL), 260 (mid), 320 (BUBBLE_W_LOGICAL); height 240
 * (BUBBLE_H_LOGICAL), all from src-tauri/src/bubble_geometry_tests.rs. Read only.
 */

const PAGE = "/src/bubbles/index.html?instance=7";
const INSTANCE = 7;
const EVENT = "bubbles-state";
const T0 = new Date("2026-01-01T00:00:00Z");
const WIDTHS = [200, 260, 320];
const HEIGHT = 240;
const LENGTHS = [20, 120, 240] as const;
const COUNTS = [1, 3, 5] as const;
const SHOT_DIR = "/tmp/voxux/t4-shots";

type Role = "user" | "assistant";
type WireItem = {
  id: number;
  role: Role;
  text: string;
  age_ms: number;
  ttl_ms: number;
};

const bubbles = (page: Page): Locator => page.getByTestId("bubble");

/** Ordinary words, cut to exactly `length` characters. */
function sentence(length: number): string {
  const words =
    "The quick brown fox jumps over the lazy dog while the overlay keeps a steady reply. ";
  return words.repeat(Math.ceil(length / words.length) + 1).slice(0, length);
}

/** Message texts used by the matrix; the last one is one unbroken 240-character token. */
const MESSAGES: Array<{ label: string; text: string }> = [
  ...LENGTHS.map((length) => ({
    label: `len${length}`,
    text: sentence(length),
  })),
  { label: "token240", text: "a".repeat(240) },
];

async function open(page: Page, width: number): Promise<void> {
  await page.setViewportSize({ width, height: HEIGHT });
  await installTauriMock(page);
  await page.clock.install({ time: T0 });
  await page.goto(PAGE);
  await expect
    .poll(async () =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              __voxMock: { calls: Array<{ command: string }> };
            }
          ).__voxMock.calls.filter((c) => c.command === "bubbles_ready").length,
      ),
    )
    .toBe(1);
  await page.clock.pauseAt(new Date(T0.getTime() + 1000));
}

function snapshotOf(items: WireItem[]) {
  return { instance: INSTANCE, rev: 1, items };
}

test.describe("Bubble fit (T4)", () => {
  test.beforeAll(() => {
    mkdirSync(SHOT_DIR, { recursive: true });
  });

  for (const width of WIDTHS) {
    for (const message of MESSAGES) {
      for (const count of COUNTS) {
        test(`${message.label} x ${count} bubble(s) at ${width}px: whole lines, full text, inert, inside`, async ({
          page,
        }) => {
          await open(page, width);
          const items: WireItem[] = Array.from({ length: count }, (_, i) => ({
            id: i + 1,
            role: i % 2 === 0 ? "assistant" : "user",
            text: message.text,
            age_ms: 0,
            ttl_ms: 60000,
          }));
          await emitMockEvent(page, EVENT, snapshotOf(items));
          // The fit keeps the newest bubbles that stack whole inside the window: at least one,
          // at most min(count, 3). Which of them fit depends on the measured heights.
          await expect
            .poll(() => bubbles(page).count())
            .toBeLessThanOrEqual(Math.min(count, 3));
          expect(await bubbles(page).count()).toBeGreaterThanOrEqual(1);
          await expect(bubbles(page).last()).toBeVisible();
          // Let the 180 ms CSS enter animation finish (CSS animations use real time).
          await page.waitForTimeout(300);

          const metrics = await bubbles(page).evaluateAll((nodes) =>
            nodes.map((el) => {
              const style = getComputedStyle(el);
              const padding =
                parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
              const lineHeight = parseFloat(style.lineHeight);
              const rect = el.getBoundingClientRect();
              return {
                textHeight: el.clientHeight - padding,
                lineHeight,
                text: el.textContent ?? "",
                pointerEvents: style.pointerEvents,
                inlineHandlers: Array.from(el.attributes)
                  .map((a) => a.name)
                  .filter((name) => name.startsWith("on")),
                hasPointerProps: [
                  (el as HTMLElement).onpointerdown,
                  (el as HTMLElement).onpointerup,
                  (el as HTMLElement).onclick,
                  (el as HTMLElement).onmousedown,
                ].some((h) => h !== null),
                left: rect.left,
                right: rect.right,
                top: rect.top,
                bottom: rect.bottom,
              };
            }),
          );
          const viewport = await page.evaluate(() => ({
            w: window.innerWidth,
            h: window.innerHeight,
            scrollW: document.documentElement.scrollWidth,
          }));

          for (const [index, m] of metrics.entries()) {
            // Whole lines only: the text box is an integer number of line boxes (1 px slack).
            expect(
              Number.isFinite(m.lineHeight),
              `bubble ${index} line-height px`,
            ).toBe(true);
            const lines = m.textHeight / m.lineHeight;
            expect(
              Math.abs(m.textHeight - Math.round(lines) * m.lineHeight),
              `bubble ${index} text box ${m.textHeight}px is whole lines of ${m.lineHeight}px`,
            ).toBeLessThanOrEqual(1);
            expect(
              Math.round(lines),
              `bubble ${index} at least one line`,
            ).toBeGreaterThanOrEqual(1);
            expect(
              Math.round(lines),
              `bubble ${index} at most 4 lines`,
            ).toBeLessThanOrEqual(4);

            // Full message in the DOM (clamping is visual only).
            expect(m.text, `bubble ${index} full text`).toBe(message.text);

            // Non-interactive.
            expect(m.pointerEvents, `bubble ${index} pointer-events`).toBe(
              "none",
            );
            expect(m.inlineHandlers, `bubble ${index} inline handlers`).toEqual(
              [],
            );
            expect(m.hasPointerProps, `bubble ${index} pointer handlers`).toBe(
              false,
            );

            // Inside the overlay horizontally and vertically.
            expect(m.left, `bubble ${index} left edge`).toBeGreaterThanOrEqual(
              0,
            );
            expect(m.right, `bubble ${index} right edge`).toBeLessThanOrEqual(
              viewport.w + 0.5,
            );
            expect(m.top, `bubble ${index} top edge`).toBeGreaterThanOrEqual(0);
            expect(m.bottom, `bubble ${index} bottom edge`).toBeLessThanOrEqual(
              viewport.h + 0.5,
            );
          }
          expect(
            viewport.scrollW,
            "no horizontal overflow",
          ).toBeLessThanOrEqual(viewport.w);

          await page.screenshot({
            path: `${SHOT_DIR}/${message.label}-n${count}-w${width}.png`,
            clip: { x: 0, y: 0, width, height: HEIGHT },
          });
        });
      }
    }
  }
});
