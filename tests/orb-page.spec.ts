import { test, expect, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * G3b-A spec (DESIGN.md Rev 5): the orb page (src/orb/index.html) against the Vite dev server
 * with the Tauri mock. The orb window may invoke ONLY orb_get_state, orb_pointer_down,
 * orb_activate, orb_menu and orb_set_reduced_motion (plus the event plugin). It receives
 * `orb-gaze {x,y}`, `orb-gesture {id,result}` and the `converse-chat` state events.
 * Authoritative: do not weaken an assertion; if a test is wrong, report it.
 */

const ORB_PAGE = "/src/orb/index.html";
const ALLOWED = new Set([
  "orb_get_state",
  "orb_pointer_down",
  "orb_activate",
  "orb_menu",
  "orb_set_reduced_motion",
]);

async function open(page: Page, face = "idle"): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face } },
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

async function usedCommands(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const mock = (
      window as unknown as {
        __voxMock: { calls: Array<{ command: string }> };
      }
    ).__voxMock;
    return mock.calls
      .map((c) => c.command)
      .filter((c) => !c.startsWith("plugin:"));
  });
}

test.describe("Orb page", () => {
  test("boots as an accessible button with the initial face and reports motion preference", async ({
    page,
  }) => {
    await open(page, "sleep");
    await expect(orb(page)).toHaveAttribute("role", "button");
    await expect(orb(page)).toHaveAttribute("tabindex", "0");
    const label = await orb(page).getAttribute("aria-label");
    expect((label ?? "").length).toBeGreaterThan(0);
    await expect(orb(page).locator(".vox-avatar")).toHaveAttribute(
      "data-face",
      "sleep",
    );
    expect(await getMockCalls(page, "orb_get_state")).toHaveLength(1);
    await expect
      .poll(async () =>
        (await getMockCalls(page, "orb_set_reduced_motion")).at(-1),
      )
      .toEqual({ reduced: false });
  });

  test("the face follows converse-chat state events and never shows entry text", async ({
    page,
  }) => {
    await open(page);
    const face = orb(page).locator(".vox-avatar");
    await emitMockEvent(page, "converse-chat", {
      seq: 1,
      kind: { type: "state", state: "listening", end: null },
    });
    await expect(face).toHaveAttribute("data-face", "listening");
    await emitMockEvent(page, "converse-chat", {
      seq: 2,
      kind: {
        type: "entry",
        entry: { id: 1, tsMs: 1, role: "user", text: "TOP SECRET WORDS" },
      },
    });
    await emitMockEvent(page, "converse-chat", {
      seq: 3,
      kind: { type: "state", state: "speaking", end: null },
    });
    await expect(face).toHaveAttribute("data-face", "speaking");
    // A stale state event does not move it back.
    await emitMockEvent(page, "converse-chat", {
      seq: 3,
      kind: { type: "state", state: "thinking", end: null },
    });
    await expect(face).toHaveAttribute("data-face", "speaking");
    expect(await page.content()).not.toContain("TOP SECRET WORDS");
    await emitMockEvent(page, "converse-chat", {
      seq: 4,
      kind: { type: "state", state: "idle", end: "user_ended" },
    });
    await expect(face).toHaveAttribute("data-face", "sleep");
  });

  test("hovering lifts the orb", async ({ page }) => {
    await open(page);
    await expect(orb(page)).not.toHaveClass(/is-hover/);
    await orb(page).hover();
    await expect(orb(page)).toHaveClass(/is-hover/);
    await page.mouse.move(0, 0);
    await expect(orb(page)).not.toHaveClass(/is-hover/);
  });

  test("the gaze event moves the highlight through CSS variables", async ({
    page,
  }) => {
    await open(page);
    expect(await cssVar(page, "--gaze-x")).toBe("0");
    await emitMockEvent(page, "orb-gaze", { x: 1, y: -0.5 });
    await expect.poll(() => cssVar(page, "--gaze-x")).toBe("1");
    expect(await cssVar(page, "--gaze-y")).toBe("-0.5");
    // Malformed payloads are ignored.
    await emitMockEvent(page, "orb-gaze", { x: "bad", y: null });
    expect(await cssVar(page, "--gaze-x")).toBe("1");
  });

  test("pointer down reports a numbered gesture; only a click result pounces", async ({
    page,
  }) => {
    await open(page);
    const box = await orb(page).boundingBox();
    if (box === null) throw new Error("orb has no box");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await expect(orb(page)).toHaveClass(/is-grabbing/);
    expect(await getMockCalls(page, "orb_pointer_down")).toEqual([{ id: 1 }]);
    await page.mouse.up();
    // The page itself decides nothing on mouse up: the native side answers.
    await expect(orb(page)).toHaveClass(/is-grabbing/);
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "click" });
    await expect(orb(page)).not.toHaveClass(/is-grabbing/);
    await expect(orb(page)).toHaveClass(/is-pouncing/);
    await expect(orb(page)).not.toHaveClass(/is-pouncing/, { timeout: 3000 });

    // Second gesture: a drag does not pounce, and a stale id is ignored.
    await page.mouse.down();
    expect(await getMockCalls(page, "orb_pointer_down")).toEqual([
      { id: 1 },
      { id: 2 },
    ]);
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "click" });
    await expect(orb(page)).toHaveClass(/is-grabbing/);
    await expect(orb(page)).not.toHaveClass(/is-pouncing/);
    await emitMockEvent(page, "orb-gesture", { id: 2, result: "drag" });
    await expect(orb(page)).not.toHaveClass(/is-grabbing/);
    await expect(orb(page)).not.toHaveClass(/is-pouncing/);
    await page.mouse.up();
  });

  test("a cancelled gesture releases the grab without a pounce", async ({
    page,
  }) => {
    await open(page);
    await orb(page).hover();
    await page.mouse.down();
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "cancel" });
    await expect(orb(page)).not.toHaveClass(/is-grabbing/);
    await expect(orb(page)).not.toHaveClass(/is-pouncing/);
    await page.mouse.up();
  });

  test("right click, Enter, Space, ContextMenu and Shift+F10 map to the menu and activate commands", async ({
    page,
  }) => {
    await open(page);
    await orb(page).click({ button: "right" });
    expect(await getMockCalls(page, "orb_menu")).toHaveLength(1);
    await orb(page).focus();
    await page.keyboard.press("Enter");
    await page.keyboard.press("Space");
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(2);
    await page.keyboard.press("ContextMenu");
    await page.keyboard.press("Shift+F10");
    expect(await getMockCalls(page, "orb_menu")).toHaveLength(3);
    await page.keyboard.press("Escape");
    await page.keyboard.press("a");
    expect(await getMockCalls(page, "orb_activate")).toHaveLength(2);
    expect(await getMockCalls(page, "orb_menu")).toHaveLength(3);
  });

  test("reduced motion: reported to the native side, no pounce, no gaze", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await expect(orb(page)).toHaveClass(/is-reduced/);
    await expect
      .poll(async () =>
        (await getMockCalls(page, "orb_set_reduced_motion")).at(-1),
      )
      .toEqual({ reduced: true });
    await orb(page).hover();
    await page.mouse.down();
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "click" });
    await expect(orb(page)).not.toHaveClass(/is-pouncing/);
    await page.mouse.up();
    await emitMockEvent(page, "orb-gaze", { x: 1, y: 1 });
    expect(await cssVar(page, "--gaze-x")).toBe("0");
    const animations = await page.evaluate(
      () =>
        document.getAnimations().filter((a) => a.playState === "running")
          .length,
    );
    expect(animations).toBe(0);
  });

  test("the page only ever calls the five orb commands", async ({ page }) => {
    await open(page);
    await orb(page).hover();
    await page.mouse.down();
    await emitMockEvent(page, "orb-gesture", { id: 1, result: "click" });
    await page.mouse.up();
    await orb(page).click({ button: "right" });
    await orb(page).focus();
    await page.keyboard.press("Enter");
    const used = await usedCommands(page);
    expect(used.length).toBeGreaterThan(0);
    for (const command of used) {
      expect(ALLOWED.has(command), `unexpected command ${command}`).toBe(true);
    }
  });
});
