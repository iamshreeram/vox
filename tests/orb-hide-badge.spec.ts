import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * T3 spec (DESIGN.md Rev 4, B5 and B8): the orb's Hide badge. A small X sibling of the globe,
 * visible on hover and keyboard focus, that calls `orb_dismiss` once. Esc on the focused orb or
 * badge calls `orb_dismiss` once too. The badge never starts a press, an activation or a drag, and
 * the orb gets no Stop badge in this round. Authoritative: do not weaken an assertion.
 */

const ORB_PAGE = "/src/orb/index.html";
const SIZES = [48, 72, 96, 128, 192] as const;
const SHOTS = "/tmp/voxux/t3-shots";

async function open(page: Page, size: number = 96): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face: "idle" } },
  });
  await page.setViewportSize({ width: size, height: size });
  await page.goto(ORB_PAGE);
  await expect(orb(page)).toBeVisible();
}

const orb = (page: Page) => page.getByTestId("orb");
const badge = (page: Page) => page.getByTestId("orb-hide");

async function opacity(page: Page, selector: string): Promise<string> {
  return page.evaluate(
    (s) =>
      getComputedStyle(document.querySelector(s) as Element).getPropertyValue(
        "opacity",
      ),
    selector,
  );
}

async function box(page: Page, selector: string) {
  const found = await page.locator(selector).boundingBox();
  if (found === null) throw new Error(`${selector} has no box`);
  return found;
}

async function hoverCentre(page: Page): Promise<void> {
  const globe = await box(page, '[data-testid="orb"]');
  await page.mouse.move(globe.x + globe.width / 2, globe.y + globe.height / 2);
}

test.describe("Orb Hide badge", () => {
  test("is hidden by default and shown on hover of the orb", async ({
    page,
  }) => {
    await open(page);
    await expect(badge(page)).toHaveCSS("opacity", "0");
    await hoverCentre(page);
    await expect
      .poll(() => opacity(page, '[data-testid="orb-hide"]'))
      .toBe("1");
    // The orb window fills the viewport, so the pointer must leave the page to stop hovering.
    await page.mouse.move(-5, -5);
    await expect
      .poll(() => opacity(page, '[data-testid="orb-hide"]'))
      .toBe("0");
  });

  test("is shown on keyboard focus with a visible focus ring", async ({
    page,
  }) => {
    await open(page);
    await orb(page).focus();
    await expect
      .poll(() => opacity(page, '[data-testid="orb-hide"]'))
      .toBe("1");
    await page.keyboard.press("Tab");
    await expect(badge(page)).toBeFocused();
    await expect(badge(page)).toHaveCSS("outline-style", "solid");
    await expect(badge(page)).toHaveCSS("opacity", "1");
  });

  test("is labelled with the Hide Vox orb text as aria-label and title", async ({
    page,
  }) => {
    await open(page);
    await expect(badge(page)).toHaveAttribute("aria-label", "Hide Vox orb");
    await expect(badge(page)).toHaveAttribute("title", "Hide Vox orb");
    await expect(badge(page)).toHaveAttribute("type", "button");
  });

  test("is a sibling of the globe button, never nested inside it", async ({
    page,
  }) => {
    await open(page);
    expect(await orb(page).locator('[data-testid="orb-hide"]').count()).toBe(0);
    const parentIsGlobeOwner = await page.evaluate(() => {
      const b = document.querySelector('[data-testid="orb-hide"]');
      const g = document.querySelector('[data-testid="orb"]');
      return b !== null && g !== null && b.parentElement === g.parentElement;
    });
    expect(parentIsGlobeOwner).toBe(true);
  });

  test("a click issues exactly one orb_dismiss and no press or activation", async ({
    page,
  }) => {
    await open(page);
    // The badge takes no pointer until the orb is hovered, so hover first (as a user does).
    await hoverCentre(page);
    await badge(page).click();
    await expect
      .poll(async () => (await getMockCalls(page, "orb_dismiss")).length)
      .toBe(1);
    await page.waitForTimeout(150);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(1);
    expect(await getMockCalls(page, "orb_pointer_down")).toHaveLength(0);
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(0);
    expect(await getMockCalls(page, "orb_menu")).toHaveLength(0);
  });

  test("Enter on the focused badge issues one orb_dismiss", async ({
    page,
  }) => {
    await open(page);
    await orb(page).focus();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(1);
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(0);
  });

  test("Esc on the focused globe issues one orb_dismiss", async ({ page }) => {
    await open(page);
    await orb(page).focus();
    await page.keyboard.press("Escape");
    await expect
      .poll(async () => (await getMockCalls(page, "orb_dismiss")).length)
      .toBe(1);
    await page.waitForTimeout(150);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(1);
    expect(await getMockCalls(page, "orb_menu")).toHaveLength(0);
  });

  test("Esc on the focused badge issues one orb_dismiss", async ({ page }) => {
    await open(page);
    await orb(page).focus();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(150);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(1);
  });

  test("keyboard Tab order is globe, then badge", async ({ page }) => {
    await open(page);
    await page.keyboard.press("Tab");
    await expect(orb(page)).toBeFocused();
    await page.keyboard.press("Tab");
    await expect(badge(page)).toBeFocused();
  });

  test("while hidden the badge takes no pointer: a press over it lands on the globe", async ({
    page,
  }) => {
    await open(page, 48);
    const found = await box(page, '[data-testid="orb-hide"]');
    const point = {
      x: found.x + found.width / 2,
      y: found.y + found.height / 2,
    };
    await expect(badge(page)).toHaveCSS("opacity", "0");
    await expect(badge(page)).toHaveCSS("pointer-events", "none");
    const hitIsGlobe = await page.evaluate(({ x, y }) => {
      const hit = document.elementFromPoint(x, y);
      return (
        hit !== null &&
        hit.closest('[data-testid="orb-hide"]') === null &&
        hit.closest('[data-testid="orb"]') !== null
      );
    }, point);
    expect(hitIsGlobe).toBe(true);
    // A press at the badge position reaches the globe handler, not the badge.
    await page.evaluate(({ x, y }) => {
      const hit = document.elementFromPoint(x, y) as HTMLElement;
      hit.dispatchEvent(
        new PointerEvent("pointerdown", {
          bubbles: true,
          cancelable: true,
          pointerId: 1,
          isPrimary: true,
          button: 0,
          buttons: 1,
          clientX: x,
          clientY: y,
        }),
      );
    }, point);
    await expect
      .poll(async () => (await getMockCalls(page, "orb_pointer_down")).length)
      .toBe(1);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(0);
  });

  test("after hover the badge takes pointer and a click dismisses once", async ({
    page,
  }) => {
    await open(page, 48);
    await hoverCentre(page);
    await expect(badge(page)).toHaveCSS("pointer-events", "auto");
    await badge(page).click();
    await expect
      .poll(async () => (await getMockCalls(page, "orb_dismiss")).length)
      .toBe(1);
    expect(await getMockCalls(page, "orb_pointer_down")).toHaveLength(0);
  });

  test("right-click on the badge prevents the native menu and opens the orb menu", async ({
    page,
  }) => {
    await open(page, 48);
    await hoverCentre(page);
    await page.evaluate(() => {
      (window as unknown as { __ctxPrevented: boolean }).__ctxPrevented = false;
      window.addEventListener("contextmenu", (event) => {
        (window as unknown as { __ctxPrevented: boolean }).__ctxPrevented =
          event.defaultPrevented;
      });
    });
    await badge(page).click({ button: "right" });
    await expect
      .poll(async () => (await getMockCalls(page, "orb_menu")).length)
      .toBe(1);
    expect(
      await page.evaluate(
        () => (window as unknown as { __ctxPrevented: boolean }).__ctxPrevented,
      ),
    ).toBe(true);
    expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(0);
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(0);
  });

  for (const size of SIZES) {
    test(`a globe click still activates and a drag still starts at ${size} px`, async ({
      page,
    }) => {
      await open(page, size);
      const globe = await box(page, '[data-testid="orb"]');
      const cx = globe.x + globe.width / 2;
      const cy = globe.y + globe.height / 2;
      await page.mouse.move(cx, cy);
      await page.mouse.down();
      await expect(orb(page)).toHaveClass(/is-grabbing/);
      expect(await getMockCalls(page, "orb_pointer_down")).toHaveLength(1);
      await page.mouse.up();
      expect(await getMockCalls(page, "orb_dismiss")).toHaveLength(0);
    });
  }

  for (const size of SIZES) {
    test(`the badge hit area and placement hold at ${size} px`, async ({
      page,
    }) => {
      await open(page, size);
      await hoverCentre(page);
      const target = await box(page, '[data-testid="orb-hide"]');
      const globe = await box(page, '[data-testid="orb"]');
      expect(target.width).toBeGreaterThanOrEqual(24);
      expect(target.height).toBeGreaterThanOrEqual(24);
      expect(target.width).toBeLessThanOrEqual(28);
      expect(target.height).toBeLessThanOrEqual(28);
      // Inside the orb window bounds.
      expect(target.x).toBeGreaterThanOrEqual(globe.x - 0.5);
      expect(target.y).toBeGreaterThanOrEqual(globe.y - 0.5);
      expect(target.x + target.width).toBeLessThanOrEqual(
        globe.x + globe.width + 0.5,
      );
      expect(target.y + target.height).toBeLessThanOrEqual(
        globe.y + globe.height + 0.5,
      );
      // The badge does not cover the globe centre.
      const centreX = globe.x + globe.width / 2;
      const centreY = globe.y + globe.height / 2;
      const covers =
        centreX > target.x &&
        centreX < target.x + target.width &&
        centreY > target.y &&
        centreY < target.y + target.height;
      expect(covers).toBe(false);
    });
  }

  test("reduced motion: the badge does not transition", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await emitMockEvent(page, "orb-gaze", { x: 0, y: 0 });
    const duration = await page.evaluate(
      () =>
        getComputedStyle(
          document.querySelector('[data-testid="orb-hide"]') as Element,
        ).transitionDuration,
    );
    expect(duration.split(",").every((d) => parseFloat(d) === 0)).toBe(true);
  });

  test("the badge never shows a Stop control or a session id", async ({
    page,
  }) => {
    await open(page);
    const text = (await badge(page).textContent()) ?? "";
    expect(text.trim()).toBe("");
    await expect(page.locator('[data-testid="orb-stop"]')).toHaveCount(0);
  });

  test("screenshots of the badge on light and dark backdrops", async ({
    page,
  }) => {
    mkdirSync(SHOTS, { recursive: true });
    for (const size of [48, 96, 192] as const) {
      for (const [name, colour] of [
        ["light", "#f2f2f2"],
        ["dark", "#101014"],
      ] as const) {
        await open(page, size);
        await page.addStyleTag({
          content: `html, body, #root { background: ${colour} !important; }`,
        });
        await hoverCentre(page);
        await expect(badge(page)).toHaveCSS("opacity", "1");
        await page.screenshot({
          path: `${SHOTS}/badge-${size}-${name}.png`,
        });
      }
    }
  });
});
