import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";
import { CHAT_PALETTES } from "../src/chat/chatTheme";
import { contrastRatio } from "../src/styles/wcag";

/*
 * T1b (UI enterprise, DESIGN Revision 4: U1, U2, U3, U4, B2b, B4). Standalone conversation window
 * against the Vite dev server with the Tauri mock. The harness records every invoke, serves one
 * authoritative snapshot (with the active session id) and controls chat_stop_conversation.
 */

interface WireSnapshot {
  seq: number;
  entries: {
    id: number;
    tsMs: number;
    role: "user" | "assistant";
    text: string;
  }[];
  state: string;
  end: string | null;
  recording: boolean;
  activeSession: number | null;
}

interface StopConfig {
  /** "true" | "false" resolve; "reject" throws; "pending" never settles. */
  stop: "true" | "false" | "reject" | "pending";
}

interface HarnessLike {
  __TAURI_INTERNALS__: {
    invoke: (
      name: string,
      args?: unknown,
      options?: unknown,
    ) => Promise<unknown>;
  };
  __pwInvokes: string[];
  __pwStopArgs: Array<{ sessionId: number }>;
}

const EVENT = "converse-chat";
const CHAT_PAGE = "/src/chat/index.html";
const HIDE_COMMAND = "chat_hide_window";
const CLEAR_COMMAND = "clear_chat_log";
const SNAPSHOT_COMMAND = "get_chat_snapshot";
const STOP_TEXT = "Could not stop the conversation.";

function snap(overrides: Partial<WireSnapshot> = {}): WireSnapshot {
  return {
    seq: 1,
    entries: [],
    state: "idle",
    end: null,
    recording: true,
    activeSession: null,
    ...overrides,
  };
}

async function installHarness(
  page: Page,
  snapshot: WireSnapshot,
  stop: StopConfig = { stop: "true" },
): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript(
    ({ s, cfg }: { s: WireSnapshot; cfg: StopConfig }) => {
      const win = window as unknown as HarnessLike;
      win.__pwInvokes = [];
      win.__pwStopArgs = [];
      const original = win.__TAURI_INTERNALS__.invoke;
      win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
        win.__pwInvokes.push(name);
        if (name === "get_chat_snapshot") {
          return Promise.resolve(JSON.parse(JSON.stringify(s)));
        }
        if (name === "chat_hide_window") return Promise.resolve(null);
        if (name === "clear_chat_log") {
          return Promise.resolve({ status: "ok", data: null });
        }
        if (name === "send_chat_message") return Promise.resolve(null);
        // Literal name: this function runs in the page, where module constants do not exist.
        if (name === "chat_stop_conversation") {
          win.__pwStopArgs.push(args as { sessionId: number });
          if (cfg.stop === "reject") return Promise.reject(new Error("boom"));
          if (cfg.stop === "pending")
            return new Promise<unknown>(() => undefined);
          return Promise.resolve(cfg.stop === "true");
        }
        return original(name, args, options);
      };
    },
    { s: snapshot, cfg: stop },
  );
}

async function openChat(
  page: Page,
  snapshot: WireSnapshot,
  options: { size?: { width: number; height: number }; stop?: StopConfig } = {},
): Promise<void> {
  if (options.size) await page.setViewportSize(options.size);
  await installHarness(page, snapshot, options.stop);
  await page.goto(CHAT_PAGE);
  await expect
    .poll(() => countInvokes(page, SNAPSHOT_COMMAND))
    .toBeGreaterThanOrEqual(1);
}

async function countInvokes(page: Page, command: string): Promise<number> {
  return page.evaluate(
    (name) =>
      (window as unknown as HarnessLike).__pwInvokes.filter((n) => n === name)
        .length,
    command,
  );
}

async function stopArgs(page: Page): Promise<Array<{ sessionId: number }>> {
  return page.evaluate(() => (window as unknown as HarnessLike).__pwStopArgs);
}

const button = (page: Page, name: string): Locator =>
  page.getByRole("button", { name, exact: true });
const hideButton = (page: Page): Locator => button(page, "Hide conversation");
const clearButton = (page: Page): Locator => button(page, "Clear");
const cancelButton = (page: Page): Locator => button(page, "Cancel");
const confirmButton = (page: Page): Locator => button(page, "Confirm clear");
const sendButton = (page: Page): Locator => button(page, "Send");
const stopButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Stop the conversation", exact: true });
const clearBar = (page: Page): Locator =>
  page.getByRole("group", {
    name: "Clear the conversation log? This cannot be undone.",
  });

async function box(
  locator: Locator,
): Promise<{ x: number; y: number; width: number; height: number }> {
  const b = await locator.boundingBox();
  expect(b, "element has a box").not.toBeNull();
  return b ?? { x: 0, y: 0, width: 0, height: 0 };
}

function disjoint(
  a: { x: number; y: number; width: number; height: number },
  b: { x: number; y: number; width: number; height: number },
): boolean {
  return (
    a.x + a.width <= b.x ||
    b.x + b.width <= a.x ||
    a.y + a.height <= b.y ||
    b.y + b.height <= a.y
  );
}

/** "rgb(r, g, b)" or "rgba(...)" to #rrggbb. */
function toHex(rgb: string): string {
  const parts = rgb.match(/[\d.]+/g)?.map(Number) ?? [0, 0, 0];
  return `#${parts
    .slice(0, 3)
    .map((v) => Math.round(v).toString(16).padStart(2, "0"))
    .join("")}`;
}

test.describe("Conversation window: Hide control and drag region (U1, U4)", () => {
  test("Hide is a visible, named icon button of at least 36 px", async ({
    page,
  }) => {
    await openChat(page, snap());
    const hide = hideButton(page);
    await expect(hide).toBeVisible();
    await expect(hide).toHaveAttribute("title", "Hide conversation");
    const b = await box(hide);
    expect(b.width).toBeGreaterThanOrEqual(36);
    expect(b.height).toBeGreaterThanOrEqual(36);
  });

  test("clicking Hide calls chat_hide_window once", async ({ page }) => {
    await openChat(page, snap());
    await hideButton(page).click();
    await expect.poll(() => countInvokes(page, HIDE_COMMAND)).toBe(1);
  });

  test("two clicks in the same tick hide the window once", async ({ page }) => {
    await openChat(page, snap());
    await hideButton(page).evaluate((el) => {
      (el as HTMLButtonElement).click();
      (el as HTMLButtonElement).click();
    });
    await expect.poll(() => countInvokes(page, HIDE_COMMAND)).toBe(1);
    await page.waitForTimeout(150);
    expect(await countInvokes(page, HIDE_COMMAND)).toBe(1);
  });

  test("the header is the drag region; the brand area is too, and no button is", async ({
    page,
  }) => {
    await openChat(page, snap());
    await expect(
      page.locator("header.chat-header[data-tauri-drag-region]"),
    ).toHaveCount(1);
    // The brand uses "deep" so a press on the logo or name (its children) starts a drag too.
    await expect(
      page.locator('.chat-brand[data-tauri-drag-region="deep"]'),
    ).toHaveCount(1);
    await expect(page.locator(".chat-brand button")).toHaveCount(0);
    // Tauri starts a drag when the mousedown target carries the attribute ("deep" also covers its
    // descendants). The buttons carry none, so they stay clickable; no button may carry it.
    await expect(page.locator("button[data-tauri-drag-region]")).toHaveCount(0);
    await expect(
      page.locator(".chat-header-end[data-tauri-drag-region]"),
    ).toHaveCount(0);
  });
});

test.describe("Conversation window: Clear confirmation bar (U3)", () => {
  for (const size of [
    { width: 360, height: 600 },
    { width: 480, height: 720 },
  ]) {
    test(`at ${size.width}x${size.height} the bar sits below the header, clears the logo, and both buttons are inside the window`, async ({
      page,
    }) => {
      await openChat(page, snap({ entries: [] }), { size });
      await clearButton(page).click();
      await expect(clearBar(page)).toBeVisible();
      const barBox = await box(clearBar(page));
      const headerBox = await box(page.locator("header.chat-header"));
      const logoBox = await box(page.locator(".chat-brand-icon"));
      const questionBox = await box(page.locator("#chat-clear-question"));
      expect(barBox.y, "bar is below the header").toBeGreaterThanOrEqual(
        headerBox.y + headerBox.height - 1,
      );
      expect(
        disjoint(questionBox, logoBox),
        "question does not cover the logo",
      ).toBe(true);
      expect(disjoint(barBox, logoBox), "bar does not cover the logo").toBe(
        true,
      );
      for (const control of [cancelButton(page), confirmButton(page)]) {
        const b = await box(control);
        expect(b.x, "left edge inside").toBeGreaterThanOrEqual(0);
        expect(b.x + b.width, "right edge inside").toBeLessThanOrEqual(
          size.width,
        );
        expect(b.y + b.height, "bottom edge inside").toBeLessThanOrEqual(
          size.height,
        );
        expect(b.height).toBeGreaterThanOrEqual(36);
      }
      expect(
        await page.evaluate(
          () =>
            document.documentElement.scrollWidth >
            document.documentElement.clientWidth + 1,
        ),
        "no horizontal scroll",
      ).toBe(false);
    });
  }

  test("Cancel has focus when the bar opens", async ({ page }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await expect(cancelButton(page)).toBeFocused();
  });

  test("the bar shows a countdown that starts at 4 seconds", async ({
    page,
  }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await expect(page.locator(".chat-clear-countdown")).toHaveText(
      "Cancels in 4 s",
    );
  });

  test("Escape cancels the bar first and the header returns to normal", async ({
    page,
  }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await expect(clearBar(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(clearBar(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeVisible();
    expect(await countInvokes(page, HIDE_COMMAND)).toBe(0);
    expect(await countInvokes(page, CLEAR_COMMAND)).toBe(0);
  });

  test("the bar cancels itself after the confirmation window and the header returns to normal", async ({
    page,
  }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await expect(clearBar(page)).toBeVisible();
    await expect(clearBar(page)).toHaveCount(0, { timeout: 6000 });
    await expect(clearButton(page)).toBeVisible();
    expect(await countInvokes(page, CLEAR_COMMAND)).toBe(0);
  });

  test("focus returns to Clear after Cancel", async ({ page }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await cancelButton(page).click();
    await expect(clearBar(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeFocused();
  });

  test("focus returns to Clear after Escape", async ({ page }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await page.keyboard.press("Escape");
    await expect(clearBar(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeFocused();
  });

  test("focus returns to Clear after the automatic cancel", async ({
    page,
  }) => {
    await openChat(page, snap());
    await clearButton(page).click();
    await expect(clearBar(page)).toHaveCount(0, { timeout: 6000 });
    await expect(clearButton(page)).toBeFocused();
  });

  test("focus returns to an enabled Clear after Confirm clear", async ({
    page,
  }) => {
    await openChat(page, snap({ entries: [] }));
    await clearButton(page).click();
    await confirmButton(page).click();
    await expect.poll(() => countInvokes(page, CLEAR_COMMAND)).toBe(1);
    await expect(clearBar(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeEnabled();
    await expect(clearButton(page)).toBeFocused();
  });

  test("a persistent live region exists before the bar opens and carries the question while it is open", async ({
    page,
  }) => {
    await openChat(page, snap());
    const announce = page.getByTestId("chat-clear-announce");
    await expect(announce).toHaveCount(1);
    await expect(announce).toHaveAttribute("role", "status");
    await expect(announce).toHaveAttribute("aria-live", "polite");
    await expect(announce).toHaveText("");
    await clearButton(page).click();
    await expect(announce).toHaveText(
      "Clear the conversation log? This cannot be undone.",
    );
    // The visible question is not a live region of its own, so it is read once, not twice.
    await expect(page.locator("#chat-clear-question")).not.toHaveAttribute(
      "role",
      "status",
    );
  });
});

test.describe("Conversation window: Stop control (U2, B2b)", () => {
  for (const state of ["listening", "thinking", "acting", "speaking"]) {
    test(`Stop is shown in state ${state} with an active session`, async ({
      page,
    }) => {
      await openChat(page, snap({ state, activeSession: 7 }));
      await expect(stopButton(page)).toBeVisible();
      const b = await box(stopButton(page));
      expect(b.height).toBeGreaterThanOrEqual(36);
      expect(b.width).toBeGreaterThanOrEqual(36);
    });
  }

  test("Stop is hidden when idle, even with a session id", async ({ page }) => {
    await openChat(page, snap({ state: "idle", activeSession: 7 }));
    await expect(stopButton(page)).toHaveCount(0);
  });

  test("Stop is hidden in a busy state without a session id", async ({
    page,
  }) => {
    await openChat(page, snap({ state: "speaking", activeSession: null }));
    await expect(stopButton(page)).toHaveCount(0);
  });

  test("a click calls chat_stop_conversation once with the rendered id", async ({
    page,
  }) => {
    await openChat(page, snap({ state: "acting", activeSession: 7 }));
    await stopButton(page).click();
    await expect.poll(async () => (await stopArgs(page)).length).toBe(1);
    expect(await stopArgs(page)).toEqual([{ sessionId: 7 }]);
  });

  test("Stop is disabled while the call is in flight", async ({ page }) => {
    await openChat(page, snap({ state: "speaking", activeSession: 7 }), {
      stop: { stop: "pending" },
    });
    await stopButton(page).click();
    await expect(stopButton(page)).toBeDisabled();
    expect(await stopArgs(page)).toHaveLength(1);
  });

  test("a false result is silent", async ({ page }) => {
    await openChat(page, snap({ state: "listening", activeSession: 7 }), {
      stop: { stop: "false" },
    });
    await stopButton(page).click();
    await expect.poll(async () => (await stopArgs(page)).length).toBe(1);
    await expect(page.getByText(STOP_TEXT, { exact: true })).toHaveCount(0);
  });

  test("a rejected call shows the notice once", async ({ page }) => {
    await openChat(page, snap({ state: "listening", activeSession: 7 }), {
      stop: { stop: "reject" },
    });
    await stopButton(page).click();
    await expect(page.getByText(STOP_TEXT, { exact: true })).toBeVisible();
    await expect(page.getByText(STOP_TEXT, { exact: true })).toHaveCount(1);
  });

  test("a state event with a new session id changes the id the next click uses", async ({
    page,
  }) => {
    await openChat(page, snap({ state: "listening", activeSession: 7 }));
    await emitMockEvent(page, EVENT, {
      seq: 2,
      kind: { type: "state", state: "speaking", end: null },
      activeSession: 9,
    });
    await expect(stopButton(page)).toBeVisible();
    await stopButton(page).click();
    await expect.poll(async () => (await stopArgs(page)).length).toBe(1);
    expect(await stopArgs(page)).toEqual([{ sessionId: 9 }]);
  });

  test("an idle state event removes the Stop control", async ({ page }) => {
    await openChat(page, snap({ state: "speaking", activeSession: 7 }));
    await expect(stopButton(page)).toBeVisible();
    await emitMockEvent(page, EVENT, {
      seq: 2,
      kind: { type: "state", state: "idle", end: "user_ended" },
      activeSession: null,
    });
    await expect(stopButton(page)).toHaveCount(0);
  });
});

test.describe("Conversation window: footer, targets and order (U8, B4)", () => {
  test("the footer mentions Esc and reaches 4.5:1 on the dark palette at 12 px or more", async ({
    page,
  }) => {
    await openChat(page, snap());
    const footer = page.locator(".chat-footer");
    await expect(footer).toContainText("Esc hides this window.");
    const style = await footer.evaluate((el) => {
      const s = getComputedStyle(el);
      return { color: s.color, size: parseFloat(s.fontSize) };
    });
    expect(style.size).toBeGreaterThanOrEqual(12);
    expect(
      contrastRatio(toHex(style.color), CHAT_PALETTES.dark.bg),
    ).toBeGreaterThanOrEqual(4.5);
  });

  test("Clear, Hide, Send and the confirm buttons are at least 36 px tall", async ({
    page,
  }) => {
    await openChat(page, snap({ state: "speaking", activeSession: 7 }));
    for (const control of [
      clearButton(page),
      hideButton(page),
      sendButton(page),
      stopButton(page),
    ]) {
      expect((await box(control)).height).toBeGreaterThanOrEqual(36);
    }
    await clearButton(page).click();
    for (const control of [cancelButton(page), confirmButton(page)]) {
      expect((await box(control)).height).toBeGreaterThanOrEqual(36);
    }
  });

  test("Tab order runs header, then transcript, then composer, then Send", async ({
    page,
  }) => {
    await openChat(page, snap({ entries: [] }));
    // Send is disabled while the box is empty (disabled buttons are not tabbable), so type first.
    await page.getByTestId("chat-input").fill("hello");
    await page.evaluate(() =>
      (document.activeElement as HTMLElement | null)?.blur(),
    );
    const order: string[] = [];
    for (let i = 0; i < 8; i += 1) {
      await page.keyboard.press("Tab");
      order.push(
        await page.evaluate(() => {
          const el = document.activeElement as HTMLElement | null;
          if (!el) return "none";
          if (el.getAttribute("aria-label") === "Hide conversation")
            return "hide";
          if (el.textContent?.trim() === "Clear") return "clear";
          if (el.getAttribute("data-testid") === "chat-input") return "input";
          if (el.getAttribute("data-testid") === "chat-send") return "send";
          return el.tagName.toLowerCase();
        }),
      );
    }
    const at = (name: string): number => order.indexOf(name);
    expect(at("clear")).toBeGreaterThanOrEqual(0);
    expect(at("hide")).toBeGreaterThan(at("clear"));
    expect(at("input")).toBeGreaterThan(at("hide"));
    expect(at("send")).toBeGreaterThan(at("input"));
  });

  test("review screenshots for the header, the bar, Stop and the footer", async ({
    page,
  }) => {
    const dir = process.env.T1B_SHOTS_DIR;
    test.skip(!dir, "set T1B_SHOTS_DIR to write review screenshots");
    await openChat(page, snap({ state: "acting", activeSession: 7 }), {
      size: { width: 360, height: 600 },
    });
    await page.screenshot({ path: `${dir}/header-acting-stop-360.png` });
    await page.setViewportSize({ width: 360, height: 600 });
    await clearButton(page).click();
    await page.screenshot({ path: `${dir}/clear-bar-360.png` });
    await page.setViewportSize({ width: 480, height: 720 });
    await page.screenshot({ path: `${dir}/clear-bar-480.png` });
  });
});
