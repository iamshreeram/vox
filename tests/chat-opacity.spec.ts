// Track T spec (DESIGN Revision 3, A-T4/A-T5/A-T6): the see-through conversation page against the
// Vite dev server with the Tauri mock. The page may only listen (no invoke of settings); the
// opacity comes from ?opacity=&rev= and from chat-opacity events. Workers must not edit this file.
import { test, expect, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";
import { CHAT_COLORS } from "../src/chat/chatTheme";

const CHAT_PAGE = "/src/chat/index.html";
const OPACITY_EVENT = "chat-opacity";
const SNAPSHOT_COMMAND = "get_chat_snapshot";

interface WireEntry {
  id: number;
  tsMs: number;
  role: "user" | "assistant" | "system";
  text: string;
}

const ENTRIES: WireEntry[] = [
  { id: 1, tsMs: 1001, role: "user", text: "hello there" },
  { id: 2, tsMs: 1002, role: "assistant", text: "hi, how can I help" },
  { id: 3, tsMs: 1003, role: "system", text: "Conversation started" },
];

/** Serves a fixed snapshot to get_chat_snapshot; everything else goes to the Tauri mock. */
async function installHarness(page: Page, entries: WireEntry[]): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript((list: WireEntry[]) => {
    const win = window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (
          name: string,
          args?: unknown,
          options?: unknown,
        ) => Promise<unknown>;
      };
      __pwInvokes: string[];
    };
    win.__pwInvokes = [];
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      win.__pwInvokes.push(name);
      if (name === "get_chat_snapshot") {
        return Promise.resolve({
          seq: 0,
          entries: list,
          state: "idle",
          end: null,
          recording: true,
        });
      }
      return original(name, args, options);
    };
  }, entries);
}

async function openPage(
  page: Page,
  query: string,
  entries: WireEntry[] = [],
): Promise<void> {
  await installHarness(page, entries);
  await page.goto(`${CHAT_PAGE}${query}`);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as unknown as { __pwInvokes: string[] }).__pwInvokes.filter(
            (n) => n === "get_chat_snapshot",
          ).length,
      ),
    )
    .toBeGreaterThanOrEqual(1);
}

async function alphaVar(page: Page): Promise<string> {
  return page.evaluate(() =>
    document.documentElement.style.getPropertyValue("--chat-alpha").trim(),
  );
}

test.describe("Conversation window: see-through opacity (Track T)", () => {
  test("?opacity=20&rev=1 sets --chat-alpha to 0.2 on first paint", async ({
    page,
  }) => {
    await openPage(page, "?opacity=20&rev=1");
    expect(await alphaVar(page)).toBe("0.2");
  });

  test("a chat-opacity event with a higher rev updates the window", async ({
    page,
  }) => {
    await openPage(page, "?opacity=20&rev=1");
    await emitMockEvent(page, OPACITY_EVENT, { rev: 2, pct: 70 });
    await expect.poll(() => alphaVar(page)).toBe("0.7");
  });

  test("a stale or equal rev is ignored", async ({ page }) => {
    await openPage(page, "?opacity=20&rev=1");
    await emitMockEvent(page, OPACITY_EVENT, { rev: 3, pct: 50 });
    await expect.poll(() => alphaVar(page)).toBe("0.5");
    await emitMockEvent(page, OPACITY_EVENT, { rev: 2, pct: 90 });
    await emitMockEvent(page, OPACITY_EVENT, { rev: 3, pct: 90 });
    // give any wrong handler time to run before asserting the value is unchanged
    await page.waitForTimeout(100);
    expect(await alphaVar(page)).toBe("0.5");
  });

  test("invalid payloads are ignored", async ({ page }) => {
    await openPage(page, "?opacity=20&rev=1");
    for (const payload of [
      { rev: 5, pct: "90" },
      { rev: 5, pct: 19 },
      { rev: 5, pct: 101 },
      { pct: 90 },
      "junk",
    ]) {
      await emitMockEvent(page, OPACITY_EVENT, payload);
    }
    await page.waitForTimeout(100);
    expect(await alphaVar(page)).toBe("0.2");
  });

  test("a listener that subscribes after a commit converges on the replayed snapshot", async ({
    page,
  }) => {
    // The window was built at rev 1 / 70; rev 2 / 20 was committed while nobody listened.
    await openPage(page, "?opacity=70&rev=1");
    await page.waitForTimeout(200);
    // the replay worker re-emits the current snapshot later (2 s cadence in Rust)
    await emitMockEvent(page, OPACITY_EVENT, { rev: 2, pct: 20 });
    await emitMockEvent(page, OPACITY_EVENT, { rev: 2, pct: 20 });
    await expect.poll(() => alphaVar(page)).toBe("0.2");
  });

  test("html and body backgrounds are computed transparent", async ({
    page,
  }) => {
    await openPage(page, "?opacity=20&rev=1");
    const backgrounds = await page.evaluate(() => ({
      html: getComputedStyle(document.documentElement).backgroundColor,
      body: getComputedStyle(document.body).backgroundColor,
    }));
    expect(backgrounds.html).toBe("rgba(0, 0, 0, 0)");
    expect(backgrounds.body).toBe("rgba(0, 0, 0, 0)");
  });

  for (const pct of [20, 70, 100]) {
    test(`the empty base pixel at ${pct} is one blend of the window colour at pct/100`, async ({
      page,
    }) => {
      await openPage(page, `?opacity=${pct}&rev=1`, []);
      // A point in the transcript padding, where no element paints a background.
      const point = await page.evaluate(() => {
        const x = 3;
        const y = Math.round(window.innerHeight * 0.6);
        const el = document.elementFromPoint(x, y);
        const background =
          el === null ? "" : getComputedStyle(el).backgroundColor;
        return { x, y, background, tag: el?.className ?? "" };
      });
      expect(point.background, `element at ${point.x},${point.y}`).toBe(
        "rgba(0, 0, 0, 0)",
      );
      const png = await page.screenshot({ omitBackground: true });
      const pixel = await page.evaluate(
        async ({ data, x, y }) => {
          const img = new Image();
          img.src = `data:image/png;base64,${data}`;
          await img.decode();
          const canvas = document.createElement("canvas");
          canvas.width = img.width;
          canvas.height = img.height;
          const ctx = canvas.getContext("2d");
          if (ctx === null) throw new Error("no 2d context");
          ctx.drawImage(img, 0, 0);
          const [r, g, b, a] = ctx.getImageData(x, y, 1, 1).data;
          return { r, g, b, a };
        },
        { data: png.toString("base64"), x: point.x, y: point.y },
      );
      const expectedAlpha = (pct / 100) * 255;
      expect(
        Math.abs(pixel.a - expectedAlpha),
        `alpha at ${pct}: ${pixel.a}`,
      ).toBeLessThanOrEqual(1.5);
      const bg = [1, 3, 5].map((i) =>
        parseInt(CHAT_COLORS.bg.slice(i, i + 2), 16),
      );
      expect(Math.abs(pixel.r - bg[0])).toBeLessThanOrEqual(3);
      expect(Math.abs(pixel.g - bg[1])).toBeLessThanOrEqual(3);
      expect(Math.abs(pixel.b - bg[2])).toBeLessThanOrEqual(3);
    });
  }

  test("every text node sits on a chip of alpha >= 0.9 at slider 20", async ({
    page,
  }) => {
    await openPage(page, "?opacity=20&rev=1", ENTRIES);
    await expect(page.locator('[data-role="user"]')).toHaveCount(1);
    const result = await page.evaluate(() => {
      const alphaOf = (color: string): number => {
        const m = color.match(/rgba?\(([^)]+)\)/);
        if (m === null) return 0;
        const parts = m[1].split(",").map((p) => p.trim());
        return parts.length === 4 ? parseFloat(parts[3]) : 1;
      };
      const failures: string[] = [];
      let checked = 0;
      const walker = document.createTreeWalker(
        document.body,
        NodeFilter.SHOW_TEXT,
      );
      let node = walker.nextNode();
      while (node !== null) {
        const text = (node.textContent ?? "").trim();
        if (text !== "") {
          checked += 1;
          let el = node.parentElement;
          let alpha = 0;
          while (el !== null) {
            alpha = alphaOf(getComputedStyle(el).backgroundColor);
            if (alpha > 0) break;
            el = el.parentElement;
          }
          if (alpha < 0.9) failures.push(`${text.slice(0, 30)} alpha=${alpha}`);
        }
        node = walker.nextNode();
      }
      return { checked, failures };
    });
    expect(result.checked).toBeGreaterThan(5);
    expect(result.failures).toEqual([]);
  });

  test("text colours are opaque", async ({ page }) => {
    await openPage(page, "?opacity=20&rev=1", ENTRIES);
    const colours = await page.evaluate(() => {
      const out: string[] = [];
      for (const el of Array.from(
        document.querySelectorAll(
          ".chat-text, .chat-brand-name, .chat-sender, .chat-empty",
        ),
      )) {
        out.push(getComputedStyle(el).color);
      }
      return out;
    });
    expect(colours.length).toBeGreaterThan(0);
    for (const colour of colours) {
      const m = colour.match(/rgba\(([^)]+)\)/);
      if (m !== null) {
        expect(parseFloat(m[1].split(",")[3])).toBe(1);
      }
    }
  });

  test("no backdrop-filter, filter blur or blur anywhere in the computed styles", async ({
    page,
  }) => {
    await openPage(page, "?opacity=20&rev=1", ENTRIES);
    const offenders = await page.evaluate(() => {
      const bad: string[] = [];
      for (const el of Array.from(document.querySelectorAll("*"))) {
        const style = getComputedStyle(el);
        const backdrop = style.getPropertyValue("backdrop-filter");
        const filter = style.getPropertyValue("filter");
        if (backdrop !== "" && backdrop !== "none")
          bad.push(`${el.tagName} backdrop=${backdrop}`);
        if (/blur/i.test(filter)) bad.push(`${el.tagName} filter=${filter}`);
      }
      return bad;
    });
    expect(offenders).toEqual([]);
  });
});
