import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";
import { CHAT_PALETTES } from "../src/chat/chatTheme";
import { contrastRatio } from "../src/styles/wcag";

/*
 * Conversation window polish (UI enterprise S4b, U10/U13/U15/U16). Runs against the Vite dev
 * server with the Tauri mock. The page may call only get_chat_snapshot and clear_chat_log (plus the
 * event plugin); the harness below records every invoke and serves snapshots from a queue.
 */

interface WireEntry {
  id: number;
  tsMs: number;
  role: "user" | "assistant" | "system";
  text: string;
}

interface WireSnapshot {
  seq: number;
  entries: WireEntry[];
  state: string;
  end: string | null;
  recording: boolean;
}

interface Config {
  snapshots: WireSnapshot[];
}

interface ChatPageLike {
  __TAURI_INTERNALS__: {
    invoke: (
      name: string,
      args?: unknown,
      options?: unknown,
    ) => Promise<unknown>;
  };
  __pwAllInvokes: string[];
  __pwSnapshotQueue: WireSnapshot[];
}

const EVENT = "converse-chat";
const CHAT_PAGE = "/src/chat/index.html";
const CLEAR_COMMAND = "clear_chat_log";
const SNAPSHOT_COMMAND = "get_chat_snapshot";

const entry = (id: number, text: string): WireEntry => ({
  id,
  tsMs: 1000 + id,
  role: "user",
  text,
});

const snapshot = (seq: number, entries: WireEntry[]): WireSnapshot => ({
  seq,
  entries,
  state: "idle",
  end: null,
  recording: true,
});

async function installHarness(page: Page, config: Config): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript((cfg: Config) => {
    const win = window as unknown as ChatPageLike;
    win.__pwAllInvokes = [];
    win.__pwSnapshotQueue = cfg.snapshots.slice();
    let last: WireSnapshot | null = null;
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      win.__pwAllInvokes.push(name);
      if (name === "clear_chat_log") return Promise.resolve(null);
      if (name === "get_chat_snapshot") {
        const next = win.__pwSnapshotQueue.shift();
        if (next !== undefined) last = next;
        return Promise.resolve(
          last === null ? null : JSON.parse(JSON.stringify(last)),
        );
      }
      return original(name, args, options);
    };
  }, config);
}

async function invokes(page: Page): Promise<string[]> {
  return page.evaluate(
    () => (window as unknown as ChatPageLike).__pwAllInvokes,
  );
}

async function openChat(
  page: Page,
  config: Config,
  size?: { width: number; height: number },
): Promise<void> {
  if (size) await page.setViewportSize(size);
  await installHarness(page, config);
  await page.goto(CHAT_PAGE);
  await expect
    .poll(async () => (await invokes(page)).includes(SNAPSHOT_COMMAND))
    .toBe(true);
}

const clearButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Clear", exact: true });
const confirmButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Confirm clear", exact: true });
const cancelButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Cancel", exact: true });
const sendButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Send", exact: true });
const jumpButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Jump to latest", exact: true });
const log = (page: Page): Locator => page.getByRole("log");

async function clearCalls(page: Page): Promise<number> {
  return (await invokes(page)).filter((name) => name === CLEAR_COMMAND).length;
}

async function colours(
  locator: Locator,
): Promise<{ color: string; background: string; borderStyle: string }> {
  return locator.evaluate((el) => {
    const style = getComputedStyle(el as HTMLElement);
    return {
      color: style.color,
      background: style.backgroundColor,
      borderStyle: style.borderTopStyle,
    };
  });
}

/** "rgb(r, g, b)" or "rgba(...)" to #rrggbb over an opaque background. */
function toHex(rgb: string): string {
  const parts = rgb.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0];
  return `#${parts
    .slice(0, 3)
    .map((v) => Math.round(v).toString(16).padStart(2, "0"))
    .join("")}`;
}

test.describe("Conversation window: inline Clear confirmation (U15a)", () => {
  test("Clear asks first: Confirm clear and Cancel appear, nothing is cleared yet, and a status announces it", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [entry(1, "keep me")])] });
    await clearButton(page).click();
    await expect(confirmButton(page)).toBeVisible();
    await expect(cancelButton(page)).toBeVisible();
    await expect(
      page.getByRole("status").filter({ hasText: "cannot be undone" }),
    ).toBeVisible();
    expect(await clearCalls(page)).toBe(0);
    await cancelButton(page).click();
    await expect(clearButton(page)).toBeVisible();
    expect(await clearCalls(page)).toBe(0);
  });

  test("the confirmation reverts to Clear by itself after 4 seconds", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [entry(1, "keep me")])] });
    await clearButton(page).click();
    await expect(confirmButton(page)).toBeVisible();
    await expect(clearButton(page)).toBeVisible({ timeout: 6000 });
    expect(await clearCalls(page)).toBe(0);
  });

  test("Confirm clear calls clear_chat_log once", async ({ page }) => {
    await openChat(page, { snapshots: [snapshot(0, [entry(1, "gone")])] });
    await clearButton(page).click();
    await confirmButton(page).click();
    await expect.poll(() => clearCalls(page)).toBe(1);
  });
});

test.describe("Conversation window: legibility and layout (U10, U13, U15, U16)", () => {
  test("the disabled Send button is still legible and shows a dashed outline, not only a faded fill", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])] });
    await expect(sendButton(page)).toBeDisabled();
    const send = await colours(sendButton(page));
    expect(send.borderStyle).toBe("dashed");
    const ratio = contrastRatio(toHex(send.color), toHex(send.background));
    expect(ratio, "disabled Send label contrast").toBeGreaterThanOrEqual(3);
  });

  test("the composer hint and the timestamps reach 4.5:1 on the dark palette", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(0, [entry(1, "a message with a time")])],
    });
    const footer = await colours(page.locator(".chat-footer"));
    const bg = CHAT_PALETTES.dark.bg;
    expect(contrastRatio(toHex(footer.color), bg)).toBeGreaterThanOrEqual(4.5);
    const time = await colours(page.locator(".chat-time").first());
    expect(contrastRatio(toHex(time.color), bg)).toBeGreaterThanOrEqual(4.5);
  });

  test("a focused control shows the shared 2px focus ring (keyboard focus only)", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])] });
    await page.keyboard.press("Tab");
    const focused = await page.evaluate(() => {
      const el = document.activeElement;
      if (!el) return null;
      const style = getComputedStyle(el);
      return {
        outlineStyle: style.outlineStyle,
        outlineWidth: style.outlineWidth,
      };
    });
    expect(focused).not.toBeNull();
    expect(focused?.outlineStyle).toBe("solid");
    expect(focused?.outlineWidth).toBe("2px");
  });

  test("at 360x420 the conversation has no horizontal scroll and Clear and Send stay inside the window", async ({
    page,
  }) => {
    await openChat(
      page,
      { snapshots: [snapshot(0, [entry(1, "narrow")])] },
      {
        width: 360,
        height: 420,
      },
    );
    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth >
        document.documentElement.clientWidth + 1,
    );
    expect(overflow, "horizontal scroll at 360 px").toBe(false);
    for (const button of [clearButton(page), sendButton(page)]) {
      const box = await button.boundingBox();
      expect(box, "control has a box").not.toBeNull();
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(361);
    }
  });

  test("the log is a polite live region that announces additions", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])] });
    await expect(log(page)).toHaveAttribute("aria-live", "polite");
    await expect(log(page)).toHaveAttribute("aria-relevant", "additions");
    await emitMockEvent(page, EVENT, {
      seq: 1,
      kind: { type: "entry", entry: entry(1, "announced") },
    });
    await expect(log(page)).toContainText("announced");
  });
});
