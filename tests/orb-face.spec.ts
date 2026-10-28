import { test, expect, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * G3d spec (DESIGN.md 'Dim-orb fix'): the orb page never shows the dim 'sleep' face, so a resting
 * orb stays visible on dark backgrounds. Written first; workers must not edit this file.
 *
 * Paths covered (same mock paths as tests/orb-page.spec.ts):
 *   - boot: orb_get_state answers {face: "sleep"} -> the avatar renders data-face="idle"
 *   - event: converse-chat {kind: state, state: "idle", end: "user_ended"} (which maps to sleep
 *     in avatarFace) -> data-face="idle"
 *   - non-sleep faces still render as themselves
 */

const ORB_PAGE = "/src/orb/index.html";

async function open(page: Page, face: string): Promise<void> {
  await installTauriMock(page, {
    commands: { orb_get_state: { face } },
  });
  await page.goto(ORB_PAGE);
  await expect(page.getByTestId("orb")).toBeVisible();
}

const avatar = (page: Page) => page.getByTestId("orb").locator(".vox-avatar");

test.describe("Orb page never shows the sleep face", () => {
  test("a boot face of sleep renders as idle", async ({ page }) => {
    await open(page, "sleep");
    // Wait until the initial state request has been answered and applied.
    await expect
      .poll(async () => (await getMockCalls(page, "orb_get_state")).length)
      .toBe(1);
    await expect(avatar(page)).toHaveAttribute("data-face", "idle");
  });

  test("a converse-chat state that means sleep renders as idle", async ({
    page,
  }) => {
    await open(page, "idle");
    await emitMockEvent(page, "converse-chat", {
      seq: 1,
      kind: { type: "state", state: "listening", end: null },
    });
    await expect(avatar(page)).toHaveAttribute("data-face", "listening");
    await emitMockEvent(page, "converse-chat", {
      seq: 2,
      kind: { type: "state", state: "idle", end: "user_ended" },
    });
    await expect(avatar(page)).toHaveAttribute("data-face", "idle");
    await expect(avatar(page)).not.toHaveAttribute("data-face", "sleep");
  });

  test("the other faces still render as themselves", async ({ page }) => {
    await open(page, "idle");
    await emitMockEvent(page, "converse-chat", {
      seq: 1,
      kind: { type: "state", state: "speaking", end: null },
    });
    await expect(avatar(page)).toHaveAttribute("data-face", "speaking");
    await emitMockEvent(page, "converse-chat", {
      seq: 2,
      kind: { type: "state", state: "thinking", end: null },
    });
    await expect(avatar(page)).toHaveAttribute("data-face", "thinking");
  });
});
