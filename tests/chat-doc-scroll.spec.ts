import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * Document-level scroll guard (UI enterprise T2b). A long transcript must scroll only inside the
 * transcript container. The document, the window root and the header must never move: a document
 * scroll pushes the header off the native window and draws it a second time over the composer.
 * Runs against the Vite dev server with the Tauri mock, like tests/chat-t2-polish.spec.ts.
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
  activeSession: number | null;
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

const CHAT_PAGE = "/src/chat/index.html";
const CHAT_EVENT = "converse-chat";
const eventPayload = (
  seq: number,
  kind: Record<string, unknown>,
  activeSession: number | null = null,
) => ({ seq, kind, activeSession });
const SNAPSHOT_COMMAND = "get_chat_snapshot";
const VIEWPORTS = [
  { width: 480, height: 720 },
  { width: 360, height: 560 },
];

const entry = (
  id: number,
  text: string,
  role: WireEntry["role"] = "user",
): WireEntry => ({ id, tsMs: 1000 + id, role, text });

const snapshot = (
  seq: number,
  entries: WireEntry[],
  extra: Partial<WireSnapshot> = {},
): WireSnapshot => ({
  seq,
  entries,
  state: "idle",
  end: null,
  recording: true,
  activeSession: null,
  ...extra,
});

const MULTI_PARAGRAPH_REPLY = [
  "Here is a **detailed** answer with several paragraphs so the bubble is tall.",
  "",
  "The first paragraph explains the context in plain words. It keeps going for a while so that the wrapped text takes up several lines inside the narrow window.",
  "",
  "- point one with `inline code`",
  "- point two with more words to wrap across the line",
  "- point three",
  "",
  "1. step one",
  "2. step two",
  "",
  "A closing paragraph ends the reply.",
].join("\n");

const FIVE_K_CODE = [
  "Here is the full log:",
  "",
  "```text",
  ...Array.from({ length: 5000 }, (_, i) => `log line ${i}`),
  "```",
].join("\n");

const LONG_TRANSCRIPT: WireEntry[] = Array.from({ length: 34 }, (_, i) => {
  const id = i + 1;
  if (id === 33) return entry(id, FIVE_K_CODE, "assistant");
  if (id % 2 === 0) return entry(id, MULTI_PARAGRAPH_REPLY, "assistant");
  return entry(id, `question number ${id}`, "user");
});

function appearance(): string {
  const seed = {
    rev: 1,
    theme: "dark",
    custom_colors: null,
    accent: "amber",
    chat_theme: "dark",
    visual_style: "vibrant",
  };
  return `ap=${encodeURIComponent(JSON.stringify(seed))}`;
}

async function installHarness(
  page: Page,
  snapshots: WireSnapshot[],
): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript((queue: WireSnapshot[]) => {
    const win = window as unknown as ChatPageLike;
    win.__pwAllInvokes = [];
    win.__pwSnapshotQueue = queue.slice();
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
  }, snapshots);
}

async function openChat(
  page: Page,
  size: { width: number; height: number },
  snapshots: WireSnapshot[],
): Promise<void> {
  await page.setViewportSize(size);
  await installHarness(page, snapshots);
  await page.goto(`${CHAT_PAGE}?${appearance()}`);
  await expect
    .poll(() =>
      page.evaluate(
        (name: string) =>
          (window as unknown as ChatPageLike).__pwAllInvokes.includes(name),
        SNAPSHOT_COMMAND,
      ),
    )
    .toBe(true);
}

const log = (page: Page): Locator => page.getByRole("log");

/** Scroll position of the document, the window root and the viewport height. */
async function documentState(page: Page): Promise<{
  docTop: number;
  docHeight: number;
  innerHeight: number;
  rootTop: number;
}> {
  return page.evaluate(() => {
    const doc = document.scrollingElement ?? document.documentElement;
    const root = document.querySelector(".chat-root");
    return {
      docTop: doc.scrollTop,
      docHeight: doc.scrollHeight,
      innerHeight: window.innerHeight,
      rootTop: root instanceof HTMLElement ? root.scrollTop : 0,
    };
  });
}

async function expectNoDocumentScroll(page: Page): Promise<void> {
  const state = await documentState(page);
  expect(state.docTop, "document scrollTop").toBe(0);
  expect(state.docHeight, "document scrollHeight").toBeLessThanOrEqual(
    state.innerHeight,
  );
  expect(state.rootTop, ".chat-root scrollTop").toBe(0);
}

async function expectChromeInside(page: Page): Promise<void> {
  const size = page.viewportSize();
  if (!size) throw new Error("no viewport");
  const header = page.locator(".chat-header");
  const composer = page.locator(".chat-composer");
  expect(await header.count(), "header elements").toBe(1);
  expect(
    await page.locator('[data-testid="chat-brand-name"]').count(),
    "brand elements",
  ).toBe(1);
  for (const target of [header, composer]) {
    const b = await target.boundingBox();
    if (!b) throw new Error("no bounding box");
    expect(b.y, "top edge inside viewport").toBeGreaterThanOrEqual(0);
    expect(b.y + b.height, "bottom edge inside viewport").toBeLessThanOrEqual(
      size.height,
    );
  }
}

test.describe("Conversation window document scroll (T2b)", () => {
  for (const size of VIEWPORTS) {
    test(`a long transcript never scrolls the document at ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await openChat(page, size, [snapshot(1, LONG_TRANSCRIPT)]);

      // The transcript is the scroller and it is pinned to the newest message.
      await expect
        .poll(() =>
          log(page).evaluate((el) => el.scrollHeight > el.clientHeight),
        )
        .toBe(true);
      await expect
        .poll(() =>
          log(page).evaluate(
            (el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 2,
          ),
        )
        .toBe(true);
      const logTop = await log(page).evaluate((el) => el.scrollTop);
      expect(logTop, "transcript scrollTop after pinning").toBeGreaterThan(0);

      await expectNoDocumentScroll(page);
      await expectChromeInside(page);
    });

    test(`a sent message and a streaming update do not scroll the document at ${size.width}x${size.height}`, async ({
      page,
    }) => {
      await openChat(page, size, [snapshot(1, LONG_TRANSCRIPT)]);
      await expect
        .poll(() =>
          log(page).evaluate((el) => el.scrollHeight > el.clientHeight),
        )
        .toBe(true);

      // A sent message: the user's line is appended while the reader is pinned at the bottom.
      await page.getByTestId("chat-input").fill("a new question to send");
      await page.getByTestId("chat-send").click();
      await emitMockEvent(
        page,
        CHAT_EVENT,
        eventPayload(2, {
          type: "entry",
          entry: entry(200, "a new question to send"),
        }),
      );
      await emitMockEvent(
        page,
        CHAT_EVENT,
        eventPayload(3, { type: "state", state: "thinking", end: null }, 7),
      );
      await expectNoDocumentScroll(page);
      await expectChromeInside(page);

      // A streaming reply: the assistant entry grows in several updates.
      await emitMockEvent(
        page,
        CHAT_EVENT,
        eventPayload(4, {
          type: "entry",
          entry: entry(201, "streaming", "assistant"),
        }),
      );
      await emitMockEvent(
        page,
        CHAT_EVENT,
        eventPayload(5, {
          type: "entry",
          entry: entry(202, MULTI_PARAGRAPH_REPLY, "assistant"),
        }),
      );
      await emitMockEvent(
        page,
        CHAT_EVENT,
        eventPayload(6, {
          type: "entry",
          entry: entry(203, "final streamed text", "assistant"),
        }),
      );
      await expect(log(page)).toContainText("final streamed text");
      await expectNoDocumentScroll(page);
      await expectChromeInside(page);
    });
  }
});
