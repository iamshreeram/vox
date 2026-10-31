import { test, expect, type Locator, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { installTauriMock } from "./fixtures/tauriMock";
import { CHAT_PALETTES } from "../src/chat/chatTheme";
import { contrastRatio } from "../src/styles/wcag";

/*
 * Conversation window polish round 2 (UI enterprise T2, DESIGN B6/B7): compact avatar, transcript
 * height, fade mask, pill colours from the orb, Stop look, empty state, safe markdown, copy button,
 * message surfaces. Runs against the Vite dev server with the Tauri mock. The page may call only
 * get_chat_snapshot and clear_chat_log (plus the event plugin); the harness serves snapshots.
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

const CHAT_PAGE = "/src/chat/index.html";
const SNAPSHOT_COMMAND = "get_chat_snapshot";
const SHOTS = "/tmp/voxux/t2-shots";

const entry = (id: number, text: string, role: WireEntry["role"] = "user") => ({
  id,
  tsMs: 1000 + id,
  role,
  text,
});

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

const MARKDOWN_REPLY = [
  "Here is the **summary** you asked for.",
  "",
  "- first point with `inline code`",
  "- second point",
  "",
  "1. step one",
  "2. step two",
  "",
  "```js",
  "const answer = 42;",
  "```",
].join("\n");

const LONG_TOKEN = "Z".repeat(400);

const TRANSCRIPT = [
  entry(1, "hello there"),
  entry(2, MARKDOWN_REPLY, "assistant"),
  entry(3, "thanks"),
];

function appearance(chatTheme: "dark" | "light"): string {
  const seed = {
    rev: 1,
    theme: chatTheme,
    custom_colors: null,
    accent: "amber",
    chat_theme: chatTheme,
    visual_style: "vibrant",
  };
  return `ap=${encodeURIComponent(JSON.stringify(seed))}`;
}

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

async function openChat(
  page: Page,
  config: Config,
  options: { size?: { width: number; height: number }; query?: string } = {},
): Promise<void> {
  if (options.size) await page.setViewportSize(options.size);
  await installHarness(page, config);
  const query = options.query ?? appearance("dark");
  await page.goto(`${CHAT_PAGE}?${query}`);
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
const assistantText = (page: Page): Locator =>
  page.locator('[data-role="assistant"] [data-testid="chat-text"]');
const pill = (page: Page): Locator => page.locator(".chat-pill");
const stopButton = (page: Page): Locator => page.getByTestId("chat-stop");
const copyButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Copy message", exact: true });

async function box(locator: Locator): Promise<{
  x: number;
  y: number;
  width: number;
  height: number;
}> {
  const b = await locator.boundingBox();
  if (!b) throw new Error("no bounding box");
  return b;
}

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}

/** Parses "rgb(r, g, b)" or "rgba(r, g, b, a)" to [r, g, b]. */
function parseRgb(value: string): [number, number, number] {
  const parts = (value.match(/[\d.]+/g) ?? []).map(Number);
  return [parts[0] ?? 0, parts[1] ?? 0, parts[2] ?? 0];
}

function sameRgb(
  a: [number, number, number],
  b: [number, number, number],
): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

test.describe("Conversation window polish round 2 (T2): layout", () => {
  test("the transcript keeps at least 55% of the height at 480x720 and 40% at 360x560", async ({
    page,
  }) => {
    await openChat(
      page,
      { snapshots: [snapshot(1, TRANSCRIPT)] },
      {
        size: { width: 480, height: 720 },
      },
    );
    const big = await box(log(page));
    expect(
      big.height / 720,
      "transcript share at 480x720",
    ).toBeGreaterThanOrEqual(0.55);
    await page.setViewportSize({ width: 360, height: 560 });
    await expect
      .poll(async () => (await box(log(page))).height / 560)
      .toBeGreaterThanOrEqual(0.4);
  });

  test("the avatar is about 96 px at 480x720 and collapses to about 64 px at 360x560", async ({
    page,
  }) => {
    await openChat(
      page,
      { snapshots: [snapshot(1, TRANSCRIPT)] },
      {
        size: { width: 480, height: 720 },
      },
    );
    const avatar = page.locator(".chat-root svg.vox-avatar");
    const tall = await box(avatar);
    expect(tall.width).toBeGreaterThanOrEqual(88);
    expect(tall.width).toBeLessThanOrEqual(104);
    await page.setViewportSize({ width: 360, height: 560 });
    await expect
      .poll(async () => (await box(avatar)).width)
      .toBeLessThanOrEqual(72);
    const short = await box(avatar);
    expect(short.width).toBeGreaterThanOrEqual(56);
  });

  test("the transcript has a soft top fade (gradient overlay, no mask) and chat.css has no backdrop-filter or blur", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    // A mask on .chat-log would sit on every reply's text chain (the UI-quality gate cannot measure
    // contrast through a mask), so the fade is a gradient overlay on .chat-body::before.
    const fade = await page
      .locator(".chat-body")
      .evaluate((el) => getComputedStyle(el, "::before").backgroundImage);
    expect(fade).toContain("linear-gradient");
    const mask = await log(page).evaluate(
      (el) =>
        getComputedStyle(el).getPropertyValue("mask-image") ||
        getComputedStyle(el).getPropertyValue("-webkit-mask-image"),
    );
    expect(mask === "" || mask === "none").toBe(true);
    const css = [
      readFileSync(new URL("../src/chat/chat.css", import.meta.url), "utf8"),
      readFileSync(new URL("../src/chat/avatar.css", import.meta.url), "utf8"),
    ].join("\n");
    expect(css).not.toMatch(/backdrop-filter|filter:\s*blur|blur\(/);
  });

  test("the window has no horizontal overflow at 360 px with a long unbroken reply", async ({
    page,
  }) => {
    await openChat(
      page,
      {
        snapshots: [
          snapshot(1, [
            entry(1, "hi"),
            entry(
              2,
              `${LONG_TOKEN}\n\n\`\`\`\n${"Q".repeat(300)}\n\`\`\``,
              "assistant",
            ),
          ]),
        ],
      },
      { size: { width: 360, height: 600 } },
    );
    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);
  });
});

test.describe("Conversation window polish round 2 (T2): status and Stop", () => {
  const FACES: Array<{ state: string; face: string }> = [
    { state: "listening", face: "listening" },
    { state: "thinking", face: "thinking" },
    { state: "speaking", face: "speaking" },
    { state: "idle", face: "idle" },
  ];

  for (const { state, face } of FACES) {
    test(`the status pill colour follows the orb colour for the ${face} face`, async ({
      page,
    }) => {
      await openChat(page, {
        snapshots: [snapshot(1, TRANSCRIPT, { state, activeSession: 7 })],
      });
      await expect(page.locator(".chat-root")).toHaveAttribute(
        "data-face",
        face,
      );
      const orbRing = await page
        .locator(".chat-root svg.vox-avatar")
        .evaluate((el) =>
          getComputedStyle(el).getPropertyValue("--chat-accent").trim(),
        );
      const borderRgb = parseRgb(
        await pill(page).evaluate((el) => getComputedStyle(el).borderTopColor),
      );
      const dotRgb = parseRgb(
        await page
          .locator(".chat-pill-dot")
          .evaluate((el) => getComputedStyle(el).backgroundColor),
      );
      expect(sameRgb(borderRgb, hexToRgb(orbRing))).toBe(true);
      expect(sameRgb(dotRgb, hexToRgb(orbRing))).toBe(true);
    });
  }

  for (const chatTheme of ["dark", "light"] as const) {
    test(`the status pill text reaches 4.5:1 on the ${chatTheme} conversation theme`, async ({
      page,
    }) => {
      await openChat(
        page,
        { snapshots: [snapshot(1, TRANSCRIPT)] },
        {
          query: appearance(chatTheme),
        },
      );
      const palette = CHAT_PALETTES[chatTheme];
      const text = await pill(page).evaluate(
        (el) => getComputedStyle(el).color,
      );
      const [r, g, b] = parseRgb(text);
      const hex = `#${[r, g, b]
        .map((v) => Math.round(v).toString(16).padStart(2, "0"))
        .join("")}`;
      expect(contrastRatio(hex, palette.surface)).toBeGreaterThanOrEqual(4.5);
    });
  }

  test("Stop is visually distinct from the pill and keeps a 36 px target", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(1, TRANSCRIPT, { state: "acting", activeSession: 7 }),
      ],
    });
    const stop = stopButton(page);
    await expect(stop).toBeVisible();
    const pillBorder = await pill(page).evaluate(
      (el) => getComputedStyle(el).borderTopColor,
    );
    const stopBorder = await stop.evaluate(
      (el) => getComputedStyle(el).borderTopColor,
    );
    expect(stopBorder).not.toBe(pillBorder);
    const pillRadius = await pill(page).evaluate(
      (el) => getComputedStyle(el).borderTopLeftRadius,
    );
    const stopRadius = await stop.evaluate(
      (el) => getComputedStyle(el).borderTopLeftRadius,
    );
    expect(stopRadius).not.toBe(pillRadius);
    const b = await box(stop);
    expect(b.height).toBeGreaterThanOrEqual(36);
    const danger = hexToRgb(CHAT_PALETTES.dark.errorBorder);
    expect(sameRgb(parseRgb(stopBorder), danger)).toBe(true);
  });
});

test.describe("Conversation window polish round 2 (T2): empty state", () => {
  test("the empty state is an inline block, not a bubble, with a hint line", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(0, [])] });
    const empty = page.locator(".chat-empty");
    await expect(empty).toBeVisible();
    const inBubble = await empty.evaluate(
      (el) => el.closest(".chat-bubble") !== null,
    );
    expect(inBubble).toBe(false);
    // Not pill-shaped: a calm panel with a modest corner radius (the old pill was 999px).
    const radius = await empty.evaluate((el) =>
      parseFloat(getComputedStyle(el).borderTopLeftRadius),
    );
    expect(radius).toBeLessThanOrEqual(16);
    await expect(page.locator(".chat-empty-hint")).toBeVisible();
  });
});

test.describe("Conversation window polish round 2 (T2): safe markdown", () => {
  test("a markdown reply renders bold, lists, a code block and inline code", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [
        snapshot(1, [
          entry(1, "explain", "user"),
          entry(2, MARKDOWN_REPLY, "assistant"),
        ]),
      ],
    });
    const reply = assistantText(page);
    await expect(reply.locator("strong")).toHaveText("summary");
    await expect(reply.locator("ul li")).toHaveCount(2);
    await expect(reply.locator("ol li")).toHaveCount(2);
    await expect(reply.locator("pre.chat-code-block code")).toContainText(
      "const answer = 42;",
    );
    await expect(reply.locator("code.chat-code-inline")).toHaveText(
      "inline code",
    );
    const text = (await reply.textContent()) ?? "";
    expect(text).not.toContain("**");
    expect(text).not.toContain("```");
  });

  test("raw HTML and an img onerror stay text: no img, no anchor, no script effect", async ({
    page,
  }) => {
    const payload =
      '<img src=x onerror="window.__pwXss=1"> <a href="https://evil.test">link</a>';
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, payload, "assistant")])],
    });
    await expect(assistantText(page)).toContainText(
      '<img src=x onerror="window.__pwXss=1">',
    );
    expect(await log(page).locator("img").count()).toBe(0);
    expect(await log(page).locator("a").count()).toBe(0);
    expect(
      await page.evaluate(
        () => (window as unknown as { __pwXss?: number }).__pwXss ?? null,
      ),
    ).toBeNull();
  });

  test("the transcript stays responsive with a 5k-line reply", async ({
    page,
  }) => {
    const body = Array.from({ length: 5000 }, (_, i) => `line ${i} **x**`).join(
      "\n",
    );
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, body, "assistant")])],
    });
    await expect(assistantText(page).locator("strong").first()).toBeVisible();
  });
});

test.describe("Conversation window polish round 2 (T2): copy", () => {
  // Renamed from "copy writes the raw reply": the copied text is now the cleaned reply
  // (stripBidiAndInvisible); this fixture has no bidi characters, so the expectation is unchanged.
  test("copy writes the cleaned reply, shows Copied for 1.5 s and never throws", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      (window as unknown as { __pwCopied: string[] }).__pwCopied = [];
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: async (value: string) => {
            (window as unknown as { __pwCopied: string[] }).__pwCopied.push(
              value,
            );
          },
        },
      });
    });
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => pageErrors.push(err.message));
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    await assistantText(page).hover();
    await copyButton(page).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Copied" }),
    ).toHaveText("Copied");
    const copied = await page.evaluate(
      () => (window as unknown as { __pwCopied: string[] }).__pwCopied,
    );
    expect(copied).toEqual([MARKDOWN_REPLY]);
    await expect(
      page.getByRole("status").filter({ hasText: "Copied" }),
    ).toHaveCount(0, { timeout: 4000 });
    expect(pageErrors).toEqual([]);
  });

  test("a rejected clipboard shows Could not copy inline and the transcript survives", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: () => Promise.reject(new Error("NotAllowedError")),
        },
      });
    });
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => pageErrors.push(err.message));
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    await assistantText(page).hover();
    await copyButton(page).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Could not copy" }),
    ).toHaveText("Could not copy");
    await expect(assistantText(page)).toContainText("summary");
    expect(pageErrors).toEqual([]);
  });

  test("a missing clipboard shows Could not copy and does not break the window", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: undefined,
      });
    });
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    await assistantText(page).hover();
    await copyButton(page).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Could not copy" }),
    ).toHaveText("Could not copy");
    await expect(log(page)).toBeVisible();
  });

  test("Tab reaches the Copy button from the header by keyboard", async ({
    page,
  }) => {
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    await page.locator(".chat-hide").focus();
    let reached = false;
    for (let i = 0; i < 30 && !reached; i += 1) {
      await page.keyboard.press("Tab");
      reached = await copyButton(page).evaluate(
        (el) => el === document.activeElement,
      );
    }
    expect(reached).toBe(true);
  });
});

test.describe("Conversation window polish round 2 (T2): safety and announcements", () => {
  const BIDI_REPLY = "pay\u202Eevil\u202C now \u2066iso\u2069 zero\u200Bwidth";
  const CLEAN_REPLY = "payevil now iso zerowidth";

  test("a reply with RLO, LRI and ZWSP renders and copies without them", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      (window as unknown as { __pwCopied: string[] }).__pwCopied = [];
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: async (value: string) => {
            (window as unknown as { __pwCopied: string[] }).__pwCopied.push(
              value,
            );
          },
        },
      });
    });
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, BIDI_REPLY, "assistant")])],
    });
    await expect(assistantText(page)).toHaveText(CLEAN_REPLY);
    await assistantText(page).hover();
    await copyButton(page).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Copied" }),
    ).toHaveText("Copied");
    const copied = await page.evaluate(
      () => (window as unknown as { __pwCopied: string[] }).__pwCopied,
    );
    expect(copied).toEqual([CLEAN_REPLY]);
  });

  test("a typed user message is plain text: 2*3*4 is not italicised", async ({
    page,
  }) => {
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, "2*3*4 and **x**", "user")])],
    });
    const userText = page.locator(
      '[data-role="user"] [data-testid="chat-text"]',
    );
    await expect(userText).toHaveText("2*3*4 and **x**");
    expect(await userText.locator("em, strong").count()).toBe(0);
  });

  test("fifty assistant replies carry no per-message live region; one shared region announces Copied", async ({
    page,
  }) => {
    const entries = Array.from({ length: 50 }, (_, i) =>
      entry(i + 1, `reply ${i}`, "assistant"),
    );
    await openChat(page, { snapshots: [snapshot(1, entries)] });
    await expect(copyButton(page)).toHaveCount(50);
    const perMessageLive = await log(page)
      .locator("[aria-live], [role=status], [role=alert]")
      .count();
    expect(perMessageLive).toBe(0);
    expect(await page.getByTestId("chat-copy-announce").count()).toBe(1);
  });

  test("Copied and Could not copy show next to the button for 1.5 s and are announced", async ({
    page,
  }) => {
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: async () => undefined },
      });
    });
    await openChat(page, { snapshots: [snapshot(1, TRANSCRIPT)] });
    await assistantText(page).hover();
    await copyButton(page).click();
    const note = page
      .locator(".chat-bubble.chat-assistant")
      .first()
      .getByTestId("chat-copy-note");
    await expect(note).toHaveText("Copied");
    await expect(note).toBeVisible();
    await expect(page.getByTestId("chat-copy-announce")).toHaveText("Copied");
    await expect(note).toHaveText("", { timeout: 4000 });
    await expect(page.getByTestId("chat-copy-announce")).toHaveText("");
  });

  test("a single-line reply renders inline bold, italic and code and invents no list or fence", async ({
    page,
  }) => {
    const line = "Run `npm test` for **all** tests, then *check* it.";
    await openChat(page, {
      snapshots: [snapshot(1, [entry(1, line, "assistant")])],
    });
    const reply = assistantText(page);
    await expect(reply.locator("code.chat-code-inline")).toHaveText("npm test");
    await expect(reply.locator("strong")).toHaveText("all");
    await expect(reply.locator("em")).toHaveText("check");
    expect(await reply.locator("ul, ol, li, pre").count()).toBe(0);
  });
});

test.describe("Conversation window polish round 2 (T2): screenshots", () => {
  test("saves the round 2 screenshots", async ({ page }) => {
    await openChat(
      page,
      { snapshots: [snapshot(1, TRANSCRIPT)] },
      {
        size: { width: 360, height: 600 },
      },
    );
    await page.screenshot({ path: `${SHOTS}/t2-markdown-360x600-dark.png` });
    // A fresh load, not a live resize: a headless resize without a reload can capture a stale
    // composited layer.
    await openChat(
      page,
      { snapshots: [snapshot(1, TRANSCRIPT)] },
      {
        size: { width: 480, height: 720 },
      },
    );
    await page.screenshot({ path: `${SHOTS}/t2-markdown-480x720-dark.png` });
    await openChat(
      page,
      { snapshots: [snapshot(1, TRANSCRIPT)] },
      {
        size: { width: 480, height: 720 },
        query: appearance("light"),
      },
    );
    await page.screenshot({ path: `${SHOTS}/t2-markdown-480x720-light.png` });
    await openChat(
      page,
      {
        snapshots: [snapshot(1, [], { state: "acting", activeSession: 7 })],
      },
      { size: { width: 480, height: 720 } },
    );
    await page.screenshot({ path: `${SHOTS}/t2-acting-stop-480x720.png` });
    await openChat(
      page,
      { snapshots: [snapshot(0, [])] },
      {
        size: { width: 480, height: 720 },
      },
    );
    await page.screenshot({ path: `${SHOTS}/t2-empty-480x720.png` });
  });
});
