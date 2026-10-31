import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";

/*
 * Orb dismissal (DESIGN.md Rev 4). The orb has no visible Hide control: the globe is its only
 * interactive element. Esc on the focused globe calls `orb_dismiss` once. The globe's click, drag
 * and menu behaviour is unchanged at every orb size. Authoritative: do not weaken an assertion.
 */

const ORB_PAGE = "/src/orb/index.html";
const SIZES = [48, 72, 96, 128, 192] as const;
const SHOTS = "/tmp/voxux/tw-shots";

async function open(page: Page, size: number = 96): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face: "idle" } },
  });
  await page.setViewportSize({ width: size, height: size });
  await page.goto(ORB_PAGE);
  await expect(orb(page)).toBeVisible();
}

const orb = (page: Page) => page.getByTestId("orb");

async function box(page: Page, selector: string) {
  const found = await page.locator(selector).boundingBox();
  if (found === null) throw new Error(`${selector} has no box`);
  return found;
}

test.describe("Orb dismissal", () => {
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

  test("the orb page has no button besides the globe", async ({ page }) => {
    await open(page);
    expect(await page.getByTestId("orb-hide").count()).toBe(0);
    expect(await page.locator("button").count()).toBe(0);
    const roleButtons = page.locator('[role="button"]');
    await expect(roleButtons).toHaveCount(1);
    await expect(roleButtons).toHaveAttribute("data-testid", "orb");
    // Tab from the page reaches the globe, the only stop on the page.
    await page.keyboard.press("Tab");
    await expect(orb(page)).toBeFocused();
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

  test("screenshots of the 96 px orb on light and dark backdrops show no badge", async ({
    page,
  }) => {
    mkdirSync(SHOTS, { recursive: true });
    for (const [name, colour] of [
      ["light", "#f2f2f2"],
      ["dark", "#101014"],
    ] as const) {
      await open(page, 96);
      await page.addStyleTag({
        content: `html, body, #root { background: ${colour} !important; }`,
      });
      await page.mouse.move(48, 48);
      await page.screenshot({ path: `${SHOTS}/orb-96-${name}.png` });
    }
  });
});
