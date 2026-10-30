import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Escape hides the standalone conversation window (DESIGN D1b). The page calls only
 * get_chat_snapshot, clear_chat_log, send_chat_message, chat_hide_window (plus the event plugin).
 * Same mock-harness pattern as chat-window.spec.ts; no clock.pauseAt (real timers, real Esc).
 */

interface HideWindowLike {
  __TAURI_INTERNALS__: {
    invoke: (
      command: string,
      args?: unknown,
      options?: unknown,
    ) => Promise<unknown>;
  };
  __pwAllInvokes: string[];
}

const SNAPSHOT_COMMAND = "get_chat_snapshot";
const CLEAR_COMMAND = "clear_chat_log";
const SEND_COMMAND = "send_chat_message";
const HIDE_COMMAND = "chat_hide_window";
const ALLOWED_PREFIX = "plugin:event|";
const CHAT_PAGE = "/src/chat/index.html";

async function installHarness(page: Page): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript(() => {
    const win = window as unknown as HideWindowLike;
    win.__pwAllInvokes = [];
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      win.__pwAllInvokes.push(name);
      if (name === "get_chat_snapshot") {
        return Promise.resolve({
          seq: 1,
          entries: [],
          state: "idle",
          end: null,
          recording: true,
        });
      }
      if (name === "chat_hide_window" || name === "clear_chat_log") {
        return Promise.resolve(null);
      }
      if (name === "send_chat_message") return Promise.resolve(null);
      return original(name, args, options);
    };
  });
}

async function invokes(page: Page): Promise<string[]> {
  return page.evaluate(
    () => (window as unknown as HideWindowLike).__pwAllInvokes,
  );
}

async function countOf(page: Page, command: string): Promise<number> {
  return (await invokes(page)).filter((n) => n === command).length;
}

async function openChat(page: Page): Promise<void> {
  await installHarness(page);
  await page.goto(CHAT_PAGE);
  await expect
    .poll(() => countOf(page, SNAPSHOT_COMMAND))
    .toBeGreaterThanOrEqual(1);
}

const composer = (page: Page) => page.getByRole("textbox");
const clearButton = (page: Page) =>
  page.getByRole("button", { name: "Clear", exact: true });
const confirmButton = (page: Page) =>
  page.getByRole("button", { name: "Confirm clear", exact: true });

test.describe("Conversation window: Escape hides the window", () => {
  test("Escape calls chat_hide_window exactly once", async ({ page }) => {
    await openChat(page);
    await composer(page).focus();
    await page.keyboard.press("Escape");
    await expect.poll(() => countOf(page, HIDE_COMMAND)).toBe(1);
  });

  test("Escape during the Clear confirmation only closes the confirmation", async ({
    page,
  }) => {
    await openChat(page);
    await clearButton(page).click();
    await expect(confirmButton(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(confirmButton(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeVisible();
    expect(await countOf(page, HIDE_COMMAND)).toBe(0);
    expect(await countOf(page, CLEAR_COMMAND)).toBe(0);
    // A second Escape, with no confirmation open, hides the window.
    await page.keyboard.press("Escape");
    await expect.poll(() => countOf(page, HIDE_COMMAND)).toBe(1);
  });

  test("the draft text survives Escape", async ({ page }) => {
    await openChat(page);
    await composer(page).fill("half a thought");
    await composer(page).focus();
    await page.keyboard.press("Escape");
    await expect.poll(() => countOf(page, HIDE_COMMAND)).toBe(1);
    await expect(composer(page)).toHaveValue("half a thought");
  });

  test("the page calls only allow-listed commands", async ({ page }) => {
    await openChat(page);
    await composer(page).focus();
    await page.keyboard.press("Escape");
    await expect.poll(() => countOf(page, HIDE_COMMAND)).toBe(1);
    for (const name of await invokes(page)) {
      const allowed =
        name === SNAPSHOT_COMMAND ||
        name === CLEAR_COMMAND ||
        name === SEND_COMMAND ||
        name === HIDE_COMMAND ||
        name.startsWith(ALLOWED_PREFIX);
      expect(allowed, `unexpected command ${name}`).toBe(true);
    }
  });
});
