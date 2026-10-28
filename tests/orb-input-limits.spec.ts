import { test, expect, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * G3b-A input and motion limits (review round 1 minors). Same mock setup as orb-page.spec.ts.
 * Authoritative: do not weaken an assertion; if a test is wrong, report it.
 */

const ORB_PAGE = "/src/orb/index.html";

async function open(page: Page): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face: "idle" } },
  });
  await page.goto(ORB_PAGE);
  await expect(page.getByTestId("orb")).toBeVisible();
}

const orb = (page: Page) => page.getByTestId("orb");

async function cssVar(page: Page, name: string): Promise<string> {
  return page.evaluate(
    (n) =>
      getComputedStyle(document.querySelector('[data-testid="orb"]') as Element)
        .getPropertyValue(n)
        .trim(),
    name,
  );
}

async function centre(page: Page): Promise<{ x: number; y: number }> {
  const box = await orb(page).boundingBox();
  if (box === null) throw new Error("orb has no box");
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

test.describe("Orb input and motion limits", () => {
  test("holding Enter or Space sends one activate, not one per auto-repeat", async ({
    page,
  }) => {
    await open(page);
    await orb(page).focus();
    await page.keyboard.down("Enter");
    await page.keyboard.down("Enter");
    await page.keyboard.up("Enter");
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(1);
    await page.keyboard.down(" ");
    await page.keyboard.down(" ");
    await page.keyboard.up(" ");
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(2);
  });

  test("Ctrl+click does not start a grab gesture; a plain click still does", async ({
    page,
  }) => {
    await open(page);
    const at = await centre(page);
    await page.keyboard.down("Control");
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await expect(orb(page)).not.toHaveClass(/is-grabbing/);
    await page.mouse.up();
    await page.keyboard.up("Control");
    expect(await getMockCalls(page, "orb_pointer_down")).toHaveLength(0);

    await page.mouse.down();
    await expect(orb(page)).toHaveClass(/is-grabbing/);
    expect(await getMockCalls(page, "orb_pointer_down")).toEqual([{ id: 1 }]);
    await page.mouse.up();
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "cancel" });
  });

  test("turning reduced motion on mid-session centres a gaze that was already applied", async ({
    page,
  }) => {
    await open(page);
    await emitMockEvent(page, "orb-gaze", { x: 1, y: 1 });
    await expect.poll(() => cssVar(page, "--gaze-x")).toBe("1");
    await page.emulateMedia({ reducedMotion: "reduce" });
    await expect(orb(page)).toHaveClass(/is-reduced/);
    await expect.poll(() => cssVar(page, "--gaze-x")).toBe("0");
    expect(await cssVar(page, "--gaze-y")).toBe("0");
  });

  test("a rejected pointer-down is cancelled locally so the orb is not stuck grabbing", async ({
    page,
  }) => {
    await open(page);
    await page.evaluate(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (cmd: string, args?: unknown, opts?: unknown) => unknown;
          };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke;
      internals.invoke = (cmd: string, args?: unknown, opts?: unknown) =>
        cmd === "orb_pointer_down"
          ? Promise.reject(new Error("no native side"))
          : original(cmd, args, opts);
    });
    const at = await centre(page);
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await expect(orb(page)).not.toHaveClass(/is-grabbing/);
    await expect(orb(page)).not.toHaveClass(/is-pouncing/);
    await page.mouse.up();
  });
});
