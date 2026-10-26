import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * Standalone conversation window (src/chat/index.html) against the Vite dev server
 * with the Tauri mock. The page may call only get_chat_snapshot and clear_chat_log
 * (plus the event plugin used to subscribe). An init script records every invoke and
 * serves get_chat_snapshot responses from a queue, so each test controls the order of
 * snapshot responses (needed for the gap and stale-response cases).
 * Wire shapes: event payload { seq, kind: { type, ... } }; snapshot { seq, entries, state, end, recording }.
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

interface HarnessConfig {
  snapshots: WireSnapshot[];
  clearFails: boolean;
}

interface ChatWindowLike {
  __TAURI_INTERNALS__: {
    invoke: (
      command: string,
      args?: unknown,
      options?: unknown,
    ) => Promise<unknown>;
  };
  __pwAllInvokes: string[];
  __pwSnapshotQueue: WireSnapshot[];
}

const EVENT = "converse-chat";
const SNAPSHOT_COMMAND = "get_chat_snapshot";
const CLEAR_COMMAND = "clear_chat_log";
const ALLOWED_PREFIX = "plugin:event|";
const CHAT_PAGE = "/src/chat/index.html";

function entry(
  id: number,
  text: string,
  role: "user" | "assistant" = "user",
): WireEntry {
  return { id, tsMs: 1000 + id, role, text };
}

function snapshot(
  seq: number,
  entries: WireEntry[],
  extra: Partial<WireSnapshot> = {},
): WireSnapshot {
  return { seq, entries, state: "idle", end: null, recording: true, ...extra };
}

function entryEvent(
  seq: number,
  e: WireEntry,
): { seq: number; kind: { type: "entry"; entry: WireEntry } } {
  return { seq, kind: { type: "entry", entry: e } };
}

async function installChatHarness(
  page: Page,
  config: HarnessConfig,
): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript((cfg: HarnessConfig) => {
    const win = window as unknown as ChatWindowLike;
    win.__pwAllInvokes = [];
    win.__pwSnapshotQueue = cfg.snapshots.slice();
    let last: WireSnapshot | null = null;
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      win.__pwAllInvokes.push(name);
      if (name === "clear_chat_log" && cfg.clearFails) {
        return Promise.reject(new Error("boom"));
      }
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
    () => (window as unknown as ChatWindowLike).__pwAllInvokes,
  );
}

async function snapshotCalls(page: Page): Promise<number> {
  return (await invokes(page)).filter((n) => n === SNAPSHOT_COMMAND).length;
}

async function clearCalls(page: Page): Promise<number> {
  return (await invokes(page)).filter((n) => n === CLEAR_COMMAND).length;
}

/** The subscription is registered before the first snapshot fetch, so once that fetch is seen, live events are delivered. */
async function waitForSubscription(page: Page): Promise<void> {
  await expect.poll(() => snapshotCalls(page)).toBeGreaterThanOrEqual(1);
}

const log = (page: Page): Locator => page.getByRole("log");
const bubbles = (page: Page): Locator =>
  page.getByRole("log").locator("[data-role]");
const clearButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Clear", exact: true });
const jumpButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Jump to latest", exact: true });

async function openChat(page: Page, config: HarnessConfig): Promise<void> {
  await installChatHarness(page, config);
  await page.goto(CHAT_PAGE);
  await waitForSubscription(page);
}

test.describe("Conversation window: hydration and content", () => {
  test("shows the empty state when the snapshot has no entries", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    await expect(
      page.getByText("No conversation yet. Say your wake phrase to start.", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(bubbles(page)).toHaveCount(0);
  });

  test("hydrates entries, status caption and recording-off note from the snapshot", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(3, [entry(1, "hi"), entry(2, "hello back", "assistant")], {
          state: "listening",
          recording: false,
        }),
      ],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["hi", "hello back"]);
    await expect(page.locator('[data-role="user"]')).toHaveCount(1);
    await expect(page.locator('[data-role="assistant"]')).toHaveCount(1);
    await expect(page.getByText("Listening", { exact: true })).toBeVisible();
    await expect(
      page.getByText("Recording is off", { exact: true }),
    ).toBeVisible();
  });

  test("shows the ended reason in the caption when the session has ended", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(2, [], { state: "idle", end: "turn_limit" })],
      clearFails: false,
    });
    await expect(
      page.getByText("Session ended", { exact: true }),
    ).toBeVisible();
  });

  test("the transcript is a polite live log and the avatar is hidden from assistive tech", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    await expect(log(page)).toHaveAttribute("aria-live", "polite");
    await expect(
      page.locator('svg[aria-hidden="true"]').first(),
    ).toBeAttached();
  });

  test("renders markup in entry text as literal text (no elements, raw text kept)", async ({
    page,
  }) => {
    const hostile = [
      "<img src=x onerror=alert(1)>",
      "<b>x</b>",
      "[link](http://evil)",
    ];
    const dialogs: string[] = [];
    page.on("dialog", (d) => {
      dialogs.push(d.message());
      void d.dismiss();
    });
    await openChat(page, {
      snapshots: [
        snapshot(
          3,
          hostile.map((t, i) => entry(i + 1, t)),
        ),
      ],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(hostile);
    await expect(log(page).locator("img, b, a")).toHaveCount(0);
    expect(dialogs).toEqual([]);
  });
});

test.describe("Conversation window: live events, scrolling and resync", () => {
  test("live entries append in order and stay pinned to the bottom", async ({
    page,
  }) => {
    const filler = Array.from({ length: 60 }, (_, i) =>
      entry(i + 1, `old ${i + 1}`),
    );
    await openChat(page, {
      snapshots: [snapshot(60, filler)],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveCount(60);
    await emitMockEvent(page, EVENT, entryEvent(61, entry(61, "fresh A")));
    await emitMockEvent(page, EVENT, entryEvent(62, entry(62, "fresh B")));
    await expect(bubbles(page).last()).toHaveText("fresh B");
    await expect(bubbles(page)).toHaveCount(62);
    await expect
      .poll(() =>
        log(page).evaluate(
          (el) => el.scrollHeight - el.scrollTop - el.clientHeight,
        ),
      )
      .toBeLessThanOrEqual(48);
  });

  test("Jump to latest appears when scrolled away and new entries arrive, and scrolls to the bottom", async ({
    page,
  }) => {
    const filler = Array.from({ length: 60 }, (_, i) =>
      entry(i + 1, `old ${i + 1}`),
    );
    await openChat(page, {
      snapshots: [snapshot(60, filler)],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveCount(60);
    await log(page).evaluate((el) => {
      el.scrollTop = 0;
      // Chrome delivers scroll events asynchronously; dispatch one so the page has handled the
      // scroll before the next entry arrives (otherwise the page still thinks it is pinned).
      el.dispatchEvent(new Event("scroll"));
    });
    await emitMockEvent(page, EVENT, entryEvent(61, entry(61, "while away")));
    await expect(jumpButton(page)).toBeVisible();
    await jumpButton(page).click();
    await expect(jumpButton(page)).toHaveCount(0);
    await expect(bubbles(page).last()).toHaveText("while away");
  });

  test("a gap in the sequence triggers exactly one extra snapshot fetch", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(2, [entry(1, "one"), entry(2, "two")]),
        snapshot(4, [entry(1, "one"), entry(2, "two"), entry(4, "four")]),
      ],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["one", "two"]);
    // seq 3 is missing: the window must resync.
    await emitMockEvent(page, EVENT, entryEvent(4, entry(4, "four")));
    await expect(bubbles(page)).toHaveText(["one", "two", "four"]);
    expect(await snapshotCalls(page)).toBe(2);
    // A later normal event shows the window is healthy and did not refetch again.
    await emitMockEvent(page, EVENT, entryEvent(5, entry(5, "five")));
    await expect(bubbles(page)).toHaveText(["one", "two", "four", "five"]);
    expect(await snapshotCalls(page)).toBe(2);
  });

  test("a stale snapshot response is discarded and refetched at most once more", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(2, [entry(1, "current")]),
        snapshot(1, [entry(1, "STALE-RESPONSE")]),
        snapshot(4, [entry(1, "current"), entry(4, "fresh")]),
      ],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["current"]);
    await emitMockEvent(page, EVENT, entryEvent(4, entry(4, "fresh")));
    await expect(bubbles(page)).toHaveText(["current", "fresh"]);
    expect(await snapshotCalls(page)).toBe(3);
    await emitMockEvent(page, EVENT, entryEvent(5, entry(5, "next")));
    await expect(bubbles(page)).toHaveText(["current", "fresh", "next"]);
    expect(await snapshotCalls(page)).toBe(3);
    await expect(page.getByText("STALE-RESPONSE", { exact: true })).toHaveCount(
      0,
    );
  });

  test("a duplicate or older event is ignored", async ({ page }) => {
    await openChat(page, {
      snapshots: [snapshot(2, [entry(1, "one"), entry(2, "two")])],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["one", "two"]);
    await emitMockEvent(page, EVENT, entryEvent(2, entry(9, "DUPLICATE")));
    await emitMockEvent(page, EVENT, entryEvent(1, entry(8, "OLDER")));
    await emitMockEvent(page, EVENT, entryEvent(3, entry(3, "three")));
    await expect(bubbles(page)).toHaveText(["one", "two", "three"]);
    expect(await snapshotCalls(page)).toBe(1);
  });

  test("a window focus event refetches the snapshot exactly once", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(2, [entry(1, "one")])],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["one"]);
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await expect.poll(() => snapshotCalls(page)).toBe(2);
    await emitMockEvent(page, EVENT, entryEvent(3, entry(3, "after focus")));
    await expect(bubbles(page).last()).toHaveText("after focus");
    expect(await snapshotCalls(page)).toBe(2);
  });
});

test.describe("Conversation window: Clear and command allow-list", () => {
  test("Clear calls clear_chat_log once and empties the view on success", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, "gone soon")])],
      clearFails: false,
    });
    await expect(bubbles(page)).toHaveText(["gone soon"]);
    await clearButton(page).click();
    await expect(
      page.getByText("No conversation yet. Say your wake phrase to start.", {
        exact: true,
      }),
    ).toBeVisible();
    expect(await clearCalls(page)).toBe(1);
  });

  test("a failed Clear keeps the transcript and shows the failure note", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, "still here")])],
      clearFails: true,
    });
    await expect(bubbles(page)).toHaveText(["still here"]);
    await clearButton(page).click();
    await expect(
      page.getByText("Could not clear the conversation. Try again.", {
        exact: true,
      }),
    ).toBeVisible();
    await expect(bubbles(page)).toHaveText(["still here"]);
  });

  test("the page calls no command other than get_chat_snapshot and clear_chat_log", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    await clearButton(page).click();
    await expect.poll(() => clearCalls(page)).toBe(1);
    const names = await invokes(page);
    for (const name of names) {
      const allowed =
        name === SNAPSHOT_COMMAND ||
        name === CLEAR_COMMAND ||
        name.startsWith(ALLOWED_PREFIX);
      expect(allowed, `unexpected command ${name}`).toBe(true);
    }
  });
});
