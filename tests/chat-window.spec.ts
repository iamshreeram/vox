import {
  test,
  expect,
  type BrowserContext,
  type Locator,
  type Page,
} from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";
import { CHAT_COLORS, FACE_ACCENT } from "../src/chat/chatTheme";

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
  /** TYPING SPEC: reject send_chat_message with this error code (tauri-specta turns a non-Error rejection into status:error). */
  sendError?: string;
  /** TYPING SPEC: resolve send_chat_message only after this many ms. */
  sendDelayMs?: number;
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
  __pwSent: string[];
}

const EVENT = "converse-chat";
const SNAPSHOT_COMMAND = "get_chat_snapshot";
const CLEAR_COMMAND = "clear_chat_log";
const SEND_COMMAND = "send_chat_message";
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
    win.__pwSent = [];
    win.__pwSnapshotQueue = cfg.snapshots.slice();
    let last: WireSnapshot | null = null;
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      win.__pwAllInvokes.push(name);
      if (name === "clear_chat_log" && cfg.clearFails) {
        return Promise.reject(new Error("boom"));
      }
      if (name === "send_chat_message") {
        const text = (args as { text?: string } | undefined)?.text ?? "";
        win.__pwSent.push(text);
        if (cfg.sendError !== undefined) {
          return Promise.reject(cfg.sendError);
        }
        return new Promise((resolve) =>
          setTimeout(() => resolve(null), cfg.sendDelayMs ?? 0),
        );
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

async function sentTexts(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as ChatWindowLike).__pwSent);
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
// UI-REFRESH (DESIGN-UI Rev 2): each [data-role] entry holds its plain entry text in exactly one
// [data-testid="chat-text"] element; sender label and time live in separate meta elements.
const bubbles = (page: Page): Locator =>
  page.getByRole("log").locator('[data-role] [data-testid="chat-text"]');
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
      page.getByText(
        "No conversation yet. Type a message below, or say your wake phrase.",
        {
          exact: true,
        },
      ),
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
      page.getByText(
        "No conversation yet. Type a message below, or say your wake phrase.",
        {
          exact: true,
        },
      ),
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

  test("the page calls no command other than get_chat_snapshot, clear_chat_log and send_chat_message", async ({
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
        name === SEND_COMMAND ||
        name.startsWith(ALLOWED_PREFIX);
      expect(allowed, `unexpected command ${name}`).toBe(true);
    }
  });
});

// ---------------------------------------------------------------------------------------------
// UI-REFRESH SPEC (DESIGN-UI Rev 2, written by ram-ai; workers must not weaken or delete these)
// Markup contract: .chat-root[data-face] carries style --chat-accent (set from chatTheme);
// header: img[data-testid=chat-brand-icon][alt=""], [data-testid=chat-brand-name]="Vox",
// [data-testid=chat-brand-sub]="Conversation", Clear button, recording-off badge;
// stage: svg orb, .chat-eq bars, ONE status pill .chat-pill holding the status text;
// bubbles: li[data-role] > .chat-meta(.chat-sender, time.chat-time) + [data-testid=chat-text];
// system notes have no sender. Backgrounds of root, bubbles, pill, header are opaque background-color.
// ---------------------------------------------------------------------------------------------

const FACE_CASES = [
  { face: "idle", state: "idle", end: null },
  { face: "listening", state: "listening", end: null },
  { face: "thinking", state: "thinking", end: null },
  { face: "acting", state: "acting", end: null },
  { face: "speaking", state: "speaking", end: null },
  { face: "sleep", state: "idle", end: "user_ended" },
  { face: "concerned", state: "idle", end: "audio_unavailable" },
] as const;

/**
 * One harness installation per page (C8): every iteration of a multi-state test gets its own
 * fresh page, so init scripts never accumulate and each page sees only its own snapshot.
 */
async function withFreshChat<T>(
  context: BrowserContext,
  config: HarnessConfig,
  fn: (page: Page) => Promise<T>,
  prepare?: (page: Page) => Promise<void>,
): Promise<T> {
  const page = await context.newPage();
  try {
    if (prepare) await prepare(page);
    await openChat(page, config);
    return await fn(page);
  } finally {
    await page.close();
  }
}

const root = (page: Page): Locator => page.locator(".chat-root");

async function rootAccent(page: Page): Promise<string> {
  return root(page).evaluate((el) =>
    getComputedStyle(el).getPropertyValue("--chat-accent").trim().toLowerCase(),
  );
}

test.describe("Conversation window: enterprise look and state colours", () => {
  for (const c of FACE_CASES) {
    test(`face ${c.face}: root data-face and --chat-accent come from the palette`, async ({
      page,
    }) => {
      await openChat(page, {
        snapshots: [
          snapshot(1, [entry(1, "hi")], { state: c.state, end: c.end }),
        ],
        clearFails: false,
      });
      await expect(root(page)).toHaveAttribute("data-face", c.face);
      expect(await rootAccent(page)).toBe(FACE_ACCENT[c.face].toLowerCase());
    });
  }

  test("listening and speaking use different accents, both different from idle", async ({
    context,
  }) => {
    const seen: Record<string, string> = {};
    for (const face of ["idle", "listening", "speaking"] as const) {
      const c = FACE_CASES.find((x) => x.face === face);
      if (!c) throw new Error("case");
      seen[face] = await withFreshChat(
        context,
        {
          snapshots: [snapshot(1, [], { state: c.state, end: c.end })],
          clearFails: false,
        },
        rootAccent,
      );
    }
    expect(new Set(Object.values(seen)).size).toBe(3);
  });

  test("a live state event changes the accent without a reload", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(1, [])], clearFails: false });
    expect(await rootAccent(page)).toBe(FACE_ACCENT.idle.toLowerCase());
    await emitMockEvent(page, EVENT, {
      seq: 2,
      kind: { type: "state", state: "listening", end: null },
    });
    await expect
      .poll(() => rootAccent(page))
      .toBe(FACE_ACCENT.listening.toLowerCase());
    await emitMockEvent(page, EVENT, {
      seq: 3,
      kind: { type: "state", state: "speaking", end: null },
    });
    await expect
      .poll(() => rootAccent(page))
      .toBe(FACE_ACCENT.speaking.toLowerCase());
  });

  test("the header shows the Vox icon, name and subtitle; the icon loads", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    const icon = page.getByTestId("chat-brand-icon");
    await expect(icon).toBeVisible();
    await expect(icon).toHaveAttribute("alt", "");
    const loaded = await icon.evaluate(
      (el) =>
        (el as HTMLImageElement).complete &&
        (el as HTMLImageElement).naturalWidth > 0,
    );
    expect(loaded).toBe(true);
    await expect(page.getByTestId("chat-brand-name")).toHaveText("Vox");
    await expect(page.getByTestId("chat-brand-sub")).toHaveText("Conversation");
  });

  test("the status text appears exactly once (in the pill)", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(1, [], { state: "speaking" })],
      clearFails: false,
    });
    await expect(page.getByText("Speaking", { exact: true })).toHaveCount(1);
    await expect(page.locator(".chat-pill")).toContainText("Speaking");
  });

  test("bubbles carry a sender label and a visible time; system notes carry no sender", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(
          3,
          [
            entry(1, "hello"),
            entry(2, "hi there", "assistant"),
            {
              id: 3,
              tsMs: 1003,
              role: "system",
              text: "system:started",
            },
          ],
          { state: "listening" },
        ),
      ],
      clearFails: false,
    });
    const user = page.locator('[data-role="user"]');
    const assistant = page.locator('[data-role="assistant"]');
    const system = page.locator('[data-role="system"]');
    await expect(user.locator(".chat-sender")).toHaveText("You");
    await expect(assistant.locator(".chat-sender")).toHaveText("Vox");
    await expect(user.locator("time.chat-time")).toBeVisible();
    await expect(assistant.locator("time.chat-time")).not.toHaveText("");
    await expect(system.locator(".chat-sender")).toHaveCount(0);
    await expect(user.locator('[data-testid="chat-text"]')).toHaveText("hello");
    await expect(assistant.locator('[data-testid="chat-text"]')).toHaveText(
      "hi there",
    );
  });

  test("user and assistant bubbles are visually distinct and aligned to opposite sides", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(2, [entry(1, "hello"), entry(2, "hi there", "assistant")]),
      ],
      clearFails: false,
    });
    const ub = await page.locator('[data-role="user"]').boundingBox();
    const ab = await page.locator('[data-role="assistant"]').boundingBox();
    const lb = await log(page).boundingBox();
    if (!ub || !ab || !lb) throw new Error("boxes");
    expect(ub.x + ub.width / 2).toBeGreaterThan(lb.x + lb.width / 2);
    expect(ab.x + ab.width / 2).toBeLessThan(lb.x + lb.width / 2);
    const bg = async (sel: string) =>
      page.locator(sel).evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(await bg('[data-role="user"]')).not.toBe(
      await bg('[data-role="assistant"]'),
    );
  });

  test("small text meets 4.5:1 on solid, gradient-free, opaque backgrounds in every state", async ({
    context,
  }) => {
    for (const c of FACE_CASES) {
      const failures = await withFreshChat(
        context,
        {
          snapshots: [
            snapshot(
              4,
              [
                entry(1, "hello"),
                entry(2, "hi there", "assistant"),
                { id: 3, tsMs: 1003, role: "system", text: "system:started" },
                { id: 4, tsMs: 1004, role: "system", text: "system:end:idle" },
              ],
              { state: c.state, end: c.end },
            ),
          ],
          clearFails: false,
        },
        (page) =>
          page.evaluate(() => {
            const parse = (
              v: string,
            ): [number, number, number, number] | null => {
              const m = v.match(/rgba?\(([^)]+)\)/);
              if (!m || !m[1]) return null;
              const p = m[1]
                .split(/[ ,/]+/)
                .filter(Boolean)
                .map(Number);
              return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0, p[3] ?? 1];
            };
            const lum = (r: number, g: number, b: number) => {
              const f = (x: number) => {
                const s = x / 255;
                return s <= 0.03928
                  ? s / 12.92
                  : Math.pow((s + 0.055) / 1.055, 2.4);
              };
              return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
            };
            const out: string[] = [];
            const sels = [
              ".chat-time",
              ".chat-sender",
              ".chat-pill",
              ".chat-footer",
              '[data-testid="chat-brand-name"]',
              '[data-testid="chat-brand-sub"]',
              '[data-role="system"] [data-testid="chat-text"]',
              '[data-role="user"] [data-testid="chat-text"]',
              '[data-role="assistant"] [data-testid="chat-text"]',
            ];
            for (const sel of sels) {
              const found = Array.from(document.querySelectorAll(sel));
              if (found.length === 0) out.push(`${sel}: not found`);
              for (const el of found) {
                const fg = parse(getComputedStyle(el).color);
                let node: Element | null = el;
                let bg: [number, number, number, number] | null = null;
                while (node) {
                  const cs = getComputedStyle(node);
                  if (cs.backgroundImage !== "none") {
                    out.push(
                      `${sel}: gradient/image behind text on ${node.tagName}.${node.getAttribute("class")}`,
                    );
                    break;
                  }
                  const b = parse(cs.backgroundColor);
                  if (b && b[3] >= 0.99) {
                    bg = b;
                    break;
                  }
                  if (b && b[3] > 0) {
                    out.push(
                      `${sel}: translucent fill behind text on ${node.tagName}.${node.getAttribute("class")}`,
                    );
                    break;
                  }
                  node = node.parentElement;
                }
                if (!fg || !bg) continue;
                const l1 = lum(fg[0], fg[1], fg[2]);
                const l2 = lum(bg[0], bg[1], bg[2]);
                const ratio =
                  (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
                if (ratio < 4.5) out.push(`${sel}: ${ratio.toFixed(2)}`);
              }
            }
            return out;
          }),
      );
      expect(failures, `face ${c.face}`).toEqual([]);
    }
  });

  test("reduced motion switches off every decorative animation and transition in every state", async ({
    context,
  }) => {
    for (const c of FACE_CASES) {
      const result = await withFreshChat(
        context,
        {
          snapshots: [
            snapshot(2, [entry(1, "x"), entry(2, "y", "assistant")], {
              state: c.state,
              end: c.end,
            }),
          ],
          clearFails: false,
        },
        async (page) => {
          const active = await page.evaluate(() => {
            const els = Array.from(
              document.querySelectorAll(".chat-root, .chat-root *"),
            );
            const bad: string[] = [];
            for (const el of els) {
              const cs = getComputedStyle(el);
              if (cs.animationName !== "none")
                bad.push(`anim:${el.getAttribute("class") ?? el.tagName}`);
              const longest = Math.max(
                0,
                ...cs.transitionDuration
                  .split(",")
                  .map((d) => parseFloat(d) || 0),
              );
              if (longest > 0)
                bad.push(
                  `transition:${el.getAttribute("class") ?? el.tagName}`,
                );
            }
            return bad;
          });
          const smooth = await log(page).evaluate(
            (el) => getComputedStyle(el).scrollBehavior,
          );
          return { active, smooth };
        },
        (page) => page.emulateMedia({ reducedMotion: "reduce" }),
      );
      expect(result.active, `face ${c.face}`).toEqual([]);
      expect(result.smooth).not.toBe("smooth");
    }
  });

  test("decorative animations do run in the speaking state without reduced motion", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await openChat(page, {
      snapshots: [snapshot(1, [], { state: "speaking" })],
      clearFails: false,
    });
    const running = await page.evaluate(
      () =>
        Array.from(
          document.querySelectorAll(".vox-avatar *, .chat-eq *"),
        ).filter((el) => getComputedStyle(el).animationName !== "none").length,
    );
    expect(running).toBeGreaterThan(0);
  });

  test("at 360px wide nothing overflows horizontally, even with an unbroken long word", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 360, height: 640 });
    await openChat(page, {
      snapshots: [
        snapshot(
          2,
          [entry(1, "A".repeat(400)), entry(2, "B".repeat(400), "assistant")],
          {
            state: "idle",
            end: "audio_unavailable",
          },
        ),
      ],
      clearFails: false,
    });
    const overflow = await page.evaluate(() => ({
      doc:
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
      header:
        (document.querySelector(".chat-header") as HTMLElement).scrollWidth -
        (document.querySelector(".chat-header") as HTMLElement).clientWidth,
      log:
        (document.querySelector('[role="log"]') as HTMLElement).scrollWidth -
        (document.querySelector('[role="log"]') as HTMLElement).clientWidth,
    }));
    expect(overflow.doc).toBeLessThanOrEqual(0);
    expect(overflow.header).toBeLessThanOrEqual(0);
    expect(overflow.log).toBeLessThanOrEqual(0);
  });

  test("keyboard focus on Clear is clearly visible", async ({ page }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    for (let i = 0; i < 6; i += 1) {
      await page.keyboard.press("Tab");
      const isClear = await clearButton(page).evaluate(
        (el) => el === document.activeElement,
      );
      if (isClear) break;
    }
    await expect(clearButton(page)).toBeFocused();
    const style = await clearButton(page).evaluate((el) => {
      const s = getComputedStyle(el);
      return {
        outline: s.outlineStyle !== "none" && parseFloat(s.outlineWidth) > 0,
        shadow: s.boxShadow !== "none",
      };
    });
    expect(style.outline || style.shadow).toBe(true);
  });

  test("the palette module is the one the page uses (text colour token)", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])], clearFails: false });
    const bg = await root(page).evaluate(
      (el) => getComputedStyle(el).backgroundColor,
    );
    // The root background is opaque and dark (luma below 0.1) regardless of the OS theme.
    const m = bg.match(/rgba?\(([^)]+)\)/);
    expect(m).not.toBeNull();
    const [r, g, b] = (m?.[1] ?? "")
      .split(/[ ,/]+/)
      .filter(Boolean)
      .map(Number);
    expect(((r ?? 255) + (g ?? 255) + (b ?? 255)) / 3).toBeLessThan(40);
    expect(CHAT_COLORS.bg).toMatch(/^#[0-9a-f]{6}$/i);
  });

  test("the dark theme does not depend on the OS colour scheme", async ({
    context,
  }) => {
    for (const scheme of ["light", "dark"] as const) {
      const bg = await withFreshChat(
        context,
        { snapshots: [snapshot(0, [])], clearFails: false },
        (page) =>
          root(page).evaluate((el) => getComputedStyle(el).backgroundColor),
        (page) => page.emulateMedia({ colorScheme: scheme }),
      );
      const nums = (bg.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
      expect(nums.reduce((a, b) => a + b, 0) / 3).toBeLessThan(40);
    }
  });
});

// ---------------------------------------------------------------------------------------------
// TYPING SPEC (DESIGN-TYPE Rev 1, written by ram-ai; workers must not weaken or delete these)
// Markup contract: composer is <form data-testid="chat-composer"> below the log and above the
// footer, holding <textarea data-testid="chat-input" aria-label="Message to Vox"
// placeholder="Type a message to Vox…" maxlength=2000 rows=1> and
// <button type="submit" data-testid="chat-send" accessible name "Send">. Enter sends, Shift+Enter
// is a newline, Enter while composing (IME) does nothing. Errors render as role=alert text.
// The transcript line is NOT added locally: it arrives from the backend through the events.
// ---------------------------------------------------------------------------------------------

const input = (page: Page): Locator => page.getByTestId("chat-input");
const sendButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Send", exact: true });

async function openTyping(
  page: Page,
  extra: Partial<HarnessConfig> = {},
): Promise<void> {
  await openChat(page, {
    snapshots: [snapshot(0, [])],
    clearFails: false,
    ...extra,
  });
}

test.describe("Conversation window: typing", () => {
  test("the composer sits between the transcript and the footer and is labelled", async ({
    page,
  }) => {
    await openTyping(page);
    await expect(input(page)).toBeVisible();
    await expect(input(page)).toHaveAttribute("aria-label", "Message to Vox");
    await expect(input(page)).toHaveAttribute(
      "placeholder",
      "Type a message to Vox…",
    );
    await expect(input(page)).toHaveAttribute("maxlength", "2000");
    const [l, c, f] = await Promise.all([
      log(page).boundingBox(),
      page.getByTestId("chat-composer").boundingBox(),
      page.locator(".chat-footer").boundingBox(),
    ]);
    if (!l || !c || !f) throw new Error("boxes");
    expect(c.y).toBeGreaterThanOrEqual(l.y + l.height - 1);
    expect(f.y).toBeGreaterThanOrEqual(c.y + c.height - 1);
  });

  test("Send is disabled for empty or whitespace-only text and enabled otherwise", async ({
    page,
  }) => {
    await openTyping(page);
    await expect(sendButton(page)).toBeDisabled();
    await input(page).fill("   ");
    await expect(sendButton(page)).toBeDisabled();
    await input(page).fill("hello");
    await expect(sendButton(page)).toBeEnabled();
  });

  test("clicking Send calls send_chat_message once with the trimmed text, clears the box and keeps focus", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("  what is the weather  ");
    await sendButton(page).click();
    await expect.poll(() => sentTexts(page)).toEqual(["what is the weather"]);
    await expect(input(page)).toHaveValue("");
    await expect(input(page)).toBeFocused();
  });

  test("Enter sends and Shift+Enter inserts a newline without sending", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).click();
    await page.keyboard.type("line one");
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type("line two");
    expect(await sentTexts(page)).toEqual([]);
    await expect(input(page)).toHaveValue("line one\nline two");
    await page.keyboard.press("Enter");
    await expect.poll(() => sentTexts(page)).toEqual(["line one\nline two"]);
    await expect(input(page)).toHaveValue("");
  });

  test("Enter while an IME composition is active does not send", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("konnichiwa");
    await input(page).evaluate((el) => {
      el.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          isComposing: true,
          bubbles: true,
          cancelable: true,
        }),
      );
    });
    await page.waitForTimeout(100);
    expect(await sentTexts(page)).toEqual([]);
    await expect(input(page)).toHaveValue("konnichiwa");
  });

  test("a message in flight blocks a second send; a newer draft survives the first one succeeding", async ({
    page,
  }) => {
    await openTyping(page, { sendDelayMs: 400 });
    await input(page).fill("first");
    await page.keyboard.press("Enter");
    await input(page).fill("second");
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    expect(await sentTexts(page)).toEqual(["first"]);
    await expect(sendButton(page)).toBeDisabled();
    await expect(sendButton(page)).toBeEnabled({ timeout: 3000 });
    // the first send succeeded but the box now holds a newer draft: it must not be wiped
    await expect(input(page)).toHaveValue("second");
    await page.keyboard.press("Enter");
    await expect.poll(() => sentTexts(page)).toEqual(["first", "second"]);
    await expect(input(page)).toHaveValue("");
  });

  test("two submits in the same tick invoke the command once", async ({
    page,
  }) => {
    await openTyping(page, { sendDelayMs: 300 });
    await input(page).fill("only once");
    await input(page).evaluate((el) => {
      for (let i = 0; i < 2; i += 1) {
        el.dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            bubbles: true,
            cancelable: true,
          }),
        );
      }
    });
    await page.waitForTimeout(500);
    expect(await sentTexts(page)).toEqual(["only once"]);
  });

  test("the typed line is not echoed locally; it appears when the backend event arrives", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("hello vox");
    await page.keyboard.press("Enter");
    await expect.poll(() => sentTexts(page)).toEqual(["hello vox"]);
    await expect(bubbles(page)).toHaveCount(0);
    await emitMockEvent(page, EVENT, entryEvent(1, entry(1, "hello vox")));
    await expect(bubbles(page)).toHaveText(["hello vox"]);
  });

  const ERROR_TEXT: Array<[string, string]> = [
    ["not_configured", "Set up a reply model in Settings first."],
    ["busy", "Vox is busy. Try again in a moment."],
    ["too_long", "That message is too long."],
    ["failed", "Could not send your message."],
    ["something_new", "Could not send your message."],
  ];
  for (const [code, text] of ERROR_TEXT) {
    test(`a ${code} failure keeps the text, shows "${text}" as an alert and clears when typing resumes`, async ({
      page,
    }) => {
      await openTyping(page, { sendError: code });
      await input(page).fill("keep me");
      await page.keyboard.press("Enter");
      await expect(page.getByRole("alert")).toHaveText(text);
      await expect(input(page)).toHaveValue("keep me");
      await input(page).press("End");
      await page.keyboard.type("!");
      await expect(page.getByRole("alert")).toHaveCount(0);
    });
  }

  test("the character counter appears from 1800 characters and the box is capped at 2000", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("a".repeat(1799));
    await expect(page.getByTestId("chat-count")).toHaveCount(0);
    await input(page).fill("a".repeat(1850));
    await expect(page.getByTestId("chat-count")).toContainText("1850");
    await expect(page.getByTestId("chat-count")).toContainText("2000");
    await input(page).fill("a".repeat(2500));
    const len = await input(page).evaluate(
      (el) => (el as HTMLTextAreaElement).value.length,
    );
    expect(len).toBeLessThanOrEqual(2000);
  });

  test("the box grows with its content up to four lines, then scrolls", async ({
    page,
  }) => {
    await openTyping(page);
    const h = async () => (await input(page).boundingBox())?.height ?? 0;
    const one = await h();
    await input(page).fill("1\n2\n3");
    const three = await h();
    expect(three).toBeGreaterThan(one);
    await input(page).fill(
      Array.from({ length: 12 }, (_, i) => `${i}`).join("\n"),
    );
    const many = await h();
    expect(many).toBeLessThanOrEqual(one * 4 + 24);
    const scrollable = await input(page).evaluate(
      (el) => el.scrollHeight > el.clientHeight,
    );
    expect(scrollable).toBe(true);
  });

  test("typed text is shown and sent as plain text (markup stays literal)", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("<img src=x onerror=alert(1)> & <b>bold</b>");
    await page.keyboard.press("Enter");
    await expect
      .poll(() => sentTexts(page))
      .toEqual(["<img src=x onerror=alert(1)> & <b>bold</b>"]);
    await expect(page.locator(".chat-root img[src='x']")).toHaveCount(0);
  });

  test("at 360px the composer does not overflow and Send stays visible", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 360, height: 640 });
    await openTyping(page);
    await input(page).fill("x".repeat(500));
    const o = await page.evaluate(() => {
      const f = document.querySelector(
        '[data-testid="chat-composer"]',
      ) as HTMLElement;
      return {
        doc:
          document.documentElement.scrollWidth -
          document.documentElement.clientWidth,
        form: f.scrollWidth - f.clientWidth,
      };
    });
    expect(o.doc).toBeLessThanOrEqual(0);
    expect(o.form).toBeLessThanOrEqual(0);
    await expect(sendButton(page)).toBeVisible();
  });

  test("input text, placeholder and Send meet 4.5:1 contrast on opaque backgrounds", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("hello");
    const failures = await page.evaluate(() => {
      const parse = (v: string): [number, number, number, number] | null => {
        const m = v.match(/rgba?\(([^)]+)\)/);
        if (!m || !m[1]) return null;
        const p = m[1]
          .split(/[ ,/]+/)
          .filter(Boolean)
          .map(Number);
        return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0, p[3] ?? 1];
      };
      const lum = (r: number, g: number, b: number) => {
        const f = (x: number) => {
          const s = x / 255;
          return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
        };
        return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
      };
      const bgOf = (el: Element) => {
        let node: Element | null = el;
        while (node) {
          const cs = getComputedStyle(node);
          if (cs.backgroundImage !== "none") return "gradient";
          const b = parse(cs.backgroundColor);
          if (b && b[3] >= 0.99) return b;
          if (b && b[3] > 0) return "translucent";
          node = node.parentElement;
        }
        return null;
      };
      const out: string[] = [];
      const ta = document.querySelector(
        '[data-testid="chat-input"]',
      ) as HTMLElement;
      const btn = document.querySelector(
        '[data-testid="chat-send"]',
      ) as HTMLElement;
      const checks: Array<[string, Element, string | null]> = [
        ["input text", ta, getComputedStyle(ta).color],
        ["placeholder", ta, getComputedStyle(ta, "::placeholder").color],
        ["send", btn, getComputedStyle(btn).color],
      ];
      for (const [name, el, color] of checks) {
        const fg = color ? parse(color) : null;
        const bg = bgOf(el);
        if (!fg || !bg || typeof bg === "string") {
          out.push(`${name}: bad colours (${String(bg)})`);
          continue;
        }
        const a = lum(fg[0], fg[1], fg[2]);
        const b = lum(bg[0], bg[1], bg[2]);
        const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
        if (name !== "send" && ratio < 4.5)
          out.push(`${name}: ${ratio.toFixed(2)}`);
      }
      return out;
    });
    // the Send button is enabled here (text typed) so it is also checked
    expect(failures).toEqual([]);
  });

  test("keyboard focus on the input and on Send is clearly visible", async ({
    page,
  }) => {
    await openTyping(page);
    await input(page).fill("hello");
    await input(page).focus();
    const visible = async (loc: Locator) =>
      loc.evaluate((el) => {
        const s = getComputedStyle(el);
        return (
          (s.outlineStyle !== "none" && parseFloat(s.outlineWidth) > 0) ||
          s.boxShadow !== "none"
        );
      });
    expect(await visible(input(page))).toBe(true);
    await page.keyboard.press("Tab");
    await expect(sendButton(page)).toBeFocused();
    expect(await visible(sendButton(page))).toBe(true);
  });

  test("typing works in the ended and error states too", async ({ page }) => {
    await openTyping(page, {
      snapshots: [snapshot(1, [], { state: "idle", end: "user_ended" })],
    });
    await expect(input(page)).toBeEnabled();
    await input(page).fill("start again");
    await page.keyboard.press("Enter");
    await expect.poll(() => sentTexts(page)).toEqual(["start again"]);
  });
});
