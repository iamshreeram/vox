import { test, expect, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * G3b-A gaze cap (design A5): the gaze moves the body and the highlight by AT MOST 4 SVG
 * units in total, per axis. The highlight may lead the body but must not exceed the cap.
 * Measured on the rendered page: getBoundingClientRect shifts relative to gaze (0,0),
 * converted to viewBox units (viewBox 160 mapped onto the rendered svg, xMidYMid meet).
 * Authoritative: do not weaken an assertion; if a test is wrong, report it.
 */

const ORB_PAGE = "/src/orb/index.html";
const VIEWBOX = 160;
const CAP_UNITS = 4;
const TOLERANCE = 0.01;

interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Measure {
  svg: Box;
  body: Box;
  shine: Box;
}

interface Shift {
  dx: number;
  dy: number;
}

async function open(page: Page): Promise<void> {
  await page.setViewportSize({ width: 200, height: 200 });
  await installTauriMock(page, {
    commands: { orb_get_state: { face: "idle" } },
  });
  await page.goto(ORB_PAGE);
  await expect(page.getByTestId("orb")).toBeVisible();
  // Freeze breathing, glints and transitions so only the gaze moves the shapes.
  await page.addStyleTag({
    content:
      "*, *::before, *::after { animation: none !important; transition: none !important; }",
  });
}

async function cssVar(page: Page, name: string): Promise<string> {
  return page.evaluate(
    (n) =>
      getComputedStyle(document.querySelector('[data-testid="orb"]') as Element)
        .getPropertyValue(n)
        .trim(),
    name,
  );
}

async function measure(page: Page): Promise<Measure> {
  return page.evaluate(() => {
    const rect = (selector: string): Box => {
      const element = document.querySelector(`[data-testid="orb"] ${selector}`);
      if (element === null) throw new Error(`missing ${selector}`);
      const r = element.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height };
    };
    return {
      svg: rect(".vox-avatar"),
      body: rect(".vox-body"),
      shine: rect(".vox-shine"),
    };
  });
}

/** Shift of a box between two measurements, in viewBox units. */
function shiftUnits(before: Measure, after: Measure, key: "body" | "shine") {
  const scale = Math.min(before.svg.width, before.svg.height) / VIEWBOX;
  return {
    dx: (after[key].x - before[key].x) / scale,
    dy: (after[key].y - before[key].y) / scale,
  };
}

async function gazeShift(
  page: Page,
  target: { x: number; y: number },
): Promise<{ body: Shift; shine: Shift }> {
  await emitMockEvent(page, "orb-gaze", { x: 0, y: 0 });
  await expect.poll(() => cssVar(page, "--gaze-x")).toBe("0");
  const base = await measure(page);
  await emitMockEvent(page, "orb-gaze", target);
  await expect.poll(() => cssVar(page, "--gaze-x")).toBe(String(target.x));
  const moved = await measure(page);
  return {
    body: shiftUnits(base, moved, "body"),
    shine: shiftUnits(base, moved, "shine"),
  };
}

test.describe("Orb gaze cap (A5)", () => {
  for (const target of [
    { x: 1, y: 1 },
    { x: -1, y: -1 },
  ]) {
    test(`gaze ${target.x},${target.y} moves body and highlight by at most ${CAP_UNITS} units and both move`, async ({
      page,
    }) => {
      await open(page);
      const { body, shine } = await gazeShift(page, target);

      for (const [name, shift] of [
        ["body", body],
        ["shine", shine],
      ] as const) {
        expect
          .soft(Math.abs(shift.dx), `${name} x shift`)
          .toBeLessThanOrEqual(CAP_UNITS + TOLERANCE);
        expect
          .soft(Math.abs(shift.dy), `${name} y shift`)
          .toBeLessThanOrEqual(CAP_UNITS + TOLERANCE);
        // The effect must not be silently removed.
        expect.soft(Math.abs(shift.dx), `${name} x moves`).toBeGreaterThan(1);
        expect.soft(Math.abs(shift.dy), `${name} y moves`).toBeGreaterThan(1);
      }

      // The highlight leads the body: it moves at least as far on each axis.
      expect(Math.abs(shine.dx)).toBeGreaterThanOrEqual(
        Math.abs(body.dx) - TOLERANCE,
      );
      expect(Math.abs(shine.dy)).toBeGreaterThanOrEqual(
        Math.abs(body.dy) - TOLERANCE,
      );
    });
  }
});
