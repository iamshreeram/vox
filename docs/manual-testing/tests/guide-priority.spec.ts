import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { expect, test, type Page } from "@playwright/test";

const here = path.dirname(fileURLToPath(import.meta.url));
const URL = pathToFileURL(
  path.join(here, "..", "experimental-features.html"),
).href;
const UIKEY = "vox-test-ui-v1";
const PRIKEY = "vox-test-ui-pri-v1";
const VIEWKEY = "vox-test-view-v1";
const KEY = "vox-test-steps-v2";

type Pri = { P0: string[]; P1: string[]; P2: string[]; X: string[] };
declare global {
  interface Window {
    PRI: Pri;
    ALL: { id: string; pri: string; title: string }[];
    PATCH: Record<string, Record<string, unknown>>;
    setResult: (id: string, v: string) => void;
    markdown: () => string;
    jsonData: () => string;
  }
}

async function open(page: Page, seed?: Record<string, string>) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push("pageerror: " + e.message));
  page.on("console", (m) => {
    if (m.type() === "error" || /PATCH|unknown test/i.test(m.text()))
      errors.push("console: " + m.text());
  });
  if (seed) {
    await page.addInitScript((s) => {
      // Seed once per tab so a later reload keeps what the page itself saved.
      if (sessionStorage.getItem("seeded")) return;
      sessionStorage.setItem("seeded", "1");
      for (const k of Object.keys(s)) localStorage.setItem(k, s[k]);
    }, seed);
  }
  await page.goto(URL);
  await page.waitForSelector(".test", { state: "attached" });
  return errors;
}

// "Next" first walks through the steps of the current test, then moves to the next test.
async function nextUntilActive(page: Page, id: string) {
  for (let i = 0; i < 30; i++) {
    const cls = await page.locator(`#t-${id}`).getAttribute("class");
    if (cls && cls.includes("active")) return;
    await page.click("#gNext");
  }
}

const isOpen = (page: Page, id: string) =>
  page.evaluate(
    (i) => (document.getElementById(i) as HTMLDetailsElement | null)?.open,
    id,
  );

test("U1 default is the priority view: groups in order, only P0 open, P0 order matches PRI", async ({
  page,
}) => {
  const errors = await open(page);
  expect(await page.locator("#view").inputValue()).toBe("priority");
  const ids = await page.$$eval("details.sec[data-view=priority]", (els) =>
    els.map((e) => e.id),
  );
  expect(ids).toEqual(["sec-pri-P0", "sec-pri-P1", "sec-pri-P2", "sec-pri-X"]);
  expect(await isOpen(page, "sec-pri-P0")).toBe(true);
  expect(await isOpen(page, "sec-pri-P1")).toBe(false);
  const p0 = await page.evaluate(() => window.PRI.P0);
  const shown = await page.$$eval("#sec-pri-P0 .test", (els) =>
    els.map((e) => e.id.replace(/^t-/, "")),
  );
  expect(shown).toEqual(p0);
  expect(p0.length).toBeGreaterThanOrEqual(15);
  expect(errors).toEqual([]);
});

test("U2 every test appears once in both views; the view choice persists; a corrupt value falls back", async ({
  page,
}) => {
  await open(page);
  const total = await page.evaluate(() => window.ALL.length);
  const count = async () => {
    const ids = await page.$$eval(".test", (els) => els.map((e) => e.id));
    expect(new Set(ids).size).toBe(ids.length);
    return ids.length;
  };
  expect(await count()).toBe(total);
  await page.selectOption("#view", "section");
  expect(await count()).toBe(total);
  await page.reload();
  await page.waitForSelector(".test", { state: "attached" });
  expect(await page.locator("#view").inputValue()).toBe("section");
  await page.evaluate((k) => localStorage.setItem(k, "bogus"), VIEWKEY);
  await page.reload();
  await page.waitForSelector(".test", { state: "attached" });
  expect(await page.locator("#view").inputValue()).toBe("priority");
});

test("U3 badges: priority on every card, developer-build flag, not-built label, From line", async ({
  page,
}) => {
  await open(page);
  const x = await page.evaluate(() => window.PRI.X[0]);
  const cardsWithoutBadge = await page.$$eval(
    ".test",
    (els) => els.filter((e) => !e.querySelector(".badge.pri")).length,
  );
  expect(cardsWithoutBadge).toBe(0);
  await expect(page.locator("#t-cmd-ack-reset .badge.pri")).toHaveText("P0");
  await expect(page.locator("#t-stt-unusable-speech .badge.pri")).toHaveText(
    "P1",
  );
  await expect(page.locator("#t-stt-unusable-speech")).toContainText(
    "developer build only",
  );
  await expect(page.locator(`#t-${x} .badge.pri`)).toContainText(
    "Not built yet",
  );
  await expect(page.locator("#t-cmd-ack-reset p.from")).toContainText("From:");
});

test("U4 UAT panel: only Pass counts, skip/blocked/not-run do not; verdict flips only when every P0 passes", async ({
  page,
}) => {
  await open(page);
  const p0 = await page.evaluate(() => window.PRI.P0);
  const n = p0.length;
  await expect(page.locator("#uat-verdict")).toHaveText(
    `NOT READY: ${n} of ${n} P0 tests not passed`,
  );
  await page.evaluate((id) => window.setResult(id, "pass"), p0[0]);
  await expect(page.locator("#uat-line")).toContainText(`1 of ${n} passed`);
  await page.evaluate((id) => window.setResult(id, "skip"), p0[1]);
  await page.evaluate((id) => window.setResult(id, "blocked"), p0[2]);
  await page.evaluate((id) => window.setResult(id, "fail"), p0[3]);
  await expect(page.locator("#uat-line")).toContainText(`1 of ${n} passed`);
  await expect(page.locator("#uat-verdict")).toHaveText(
    `NOT READY: ${n - 1} of ${n} P0 tests not passed`,
  );
  await expect(page.locator("#cnt-pri-P0")).toContainText("need attention");
  await page.evaluate((ids) => {
    for (const id of ids) window.setResult(id, "pass");
  }, p0);
  await expect(page.locator("#uat-verdict")).toHaveText(
    `ALL ${n} P0 TESTS PASSED`,
  );
});

test("U4b open state is stored per view; all-closed persists; seeded section state is not lost", async ({
  page,
}) => {
  await open(page, { [UIKEY]: JSON.stringify({ "sec-setup": 1 }) });
  expect(await isOpen(page, "sec-pri-P0")).toBe(true);
  await page.click("#sec-pri-P0 > summary");
  await page.click("#sec-pri-P2 > summary");
  expect(await isOpen(page, "sec-pri-P0")).toBe(false);
  expect(await isOpen(page, "sec-pri-P2")).toBe(true);
  await page.reload();
  await page.waitForSelector(".test", { state: "attached" });
  expect(await isOpen(page, "sec-pri-P0")).toBe(false);
  expect(await isOpen(page, "sec-pri-P2")).toBe(true);
  await page.selectOption("#view", "section");
  expect(await isOpen(page, "sec-setup")).toBe(true);
  await page.selectOption("#view", "priority");
  expect(await isOpen(page, "sec-pri-P2")).toBe(true);
  await page.click("#sec-pri-P2 > summary");
  await page.reload();
  await page.waitForSelector(".test", { state: "attached" });
  for (const g of ["P0", "P1", "P2", "X"])
    expect(await isOpen(page, `sec-pri-${g}`)).toBe(false);
  expect(await page.evaluate((k) => localStorage.getItem(k), PRIKEY)).not.toBe(
    null,
  );
});

test("U5 priority filter hides lower groups and empty groups; it combines with the status filter", async ({
  page,
}) => {
  await open(page);
  await page.selectOption("#prifilter", "p0");
  await expect(page.locator("#sec-pri-P1")).toHaveClass(/hide/);
  await expect(page.locator("#sec-pri-P2")).toHaveClass(/hide/);
  await expect(page.locator("#sec-pri-X")).toHaveClass(/hide/);
  await expect(page.locator("#sec-pri-P0")).not.toHaveClass(/hide/);
  await page.selectOption("#prifilter", "p01");
  await expect(page.locator("#sec-pri-P1")).not.toHaveClass(/hide/);
  await expect(page.locator("#sec-pri-P2")).toHaveClass(/hide/);
  await page.selectOption("#prifilter", "all");
  await page.selectOption("#filter", "fail");
  for (const g of ["P0", "P1", "P2", "X"])
    await expect(page.locator(`#sec-pri-${g}`)).toHaveClass(/hide/);
});

test("U7 guide follows the on-screen order (priority order, not section order)", async ({
  page,
}) => {
  await open(page);
  const { p0, p1 } = await page.evaluate(() => ({
    p0: window.PRI.P0,
    p1: window.PRI.P1,
  }));
  await page.click(`[data-guide="${p0[0]}"]`);
  await expect(page.locator(`#t-${p0[0]}`)).toHaveClass(/active/);
  await nextUntilActive(page, p0[1]);
  await expect(page.locator(`#t-${p0[1]}`)).toHaveClass(/active/);
  // Start at the last P0: the next test is the first P1, not the next one in section order.
  await page.click("#gStop");
  await page.click(`[data-guide="${p0[p0.length - 1]}"]`);
  await nextUntilActive(page, p1[0]);
  await expect(page.locator(`#t-${p1[0]}`)).toHaveClass(/active/);
  // A non-first card: the result lands on that card.
  await page.click("#gPass");
  await expect(page.locator(`#b-${p1[0]}`)).toHaveText("pass");
  // Switching view mid-guide keeps the current test.
  await page.selectOption("#view", "section");
  await expect(page.locator(`#t-${p1[0]}`)).toHaveClass(/active/);
});

test("U8 reports carry priority and the UAT verdict; old keys stay", async ({
  page,
}) => {
  await open(page);
  const p0 = await page.evaluate(() => window.PRI.P0);
  await page.evaluate((id) => window.setResult(id, "pass"), p0[0]);
  const md = await page.evaluate(() => window.markdown());
  expect(md).toContain("## UAT summary (P0)");
  expect(md).toContain(
    `NOT READY: ${p0.length - 1} of ${p0.length} P0 tests not passed`,
  );
  expect(md).toContain(`[P0]`);
  expect(md).toContain(p0[1]);
  const j = JSON.parse(await page.evaluate(() => window.jsonData())) as {
    results: { id: string; priority?: string }[];
    summaryByPriority: Record<string, { total: number }>;
    uatVerdict: string;
  };
  expect(j.uatVerdict).toContain("NOT READY");
  expect(j.summaryByPriority.P0.total).toBe(p0.length);
  expect(j.results.find((r) => r.id === p0[0])?.priority).toBe("P0");
  expect(Object.keys(j)).toEqual(expect.arrayContaining(["results"]));
});

test("U9 legacy saved answers still show; an unknown saved id stays in the orphan list", async ({
  page,
}) => {
  const state = {
    tests: {
      "setup-run": { r: "pass" },
      "gone-id": { r: "fail", n: "kept text" },
    },
    meta: {},
    secNotes: {},
    overall: "",
  };
  await open(page, { [KEY]: JSON.stringify(state) });
  await expect(page.locator("#b-setup-run")).toHaveText("pass");
  await expect(page.locator("#app")).toContainText("gone-id");
  await expect(page.locator("#app")).toContainText("kept text");
});

test("U10 corrections reach the page: every PATCH title is rendered on its card", async ({
  page,
}) => {
  const errors = await open(page);
  const bad = await page.evaluate(() => {
    const strip = (h: string) =>
      h
        .replace(/<[^>]+>/g, "")
        .replace(/&amp;/g, "&")
        .trim();
    const out: string[] = [];
    for (const id of Object.keys(window.PATCH)) {
      const p = window.PATCH[id];
      const card = document.getElementById("t-" + id);
      if (!card) {
        out.push("missing card " + id);
        continue;
      }
      if (typeof p.title === "string") {
        const h = card.querySelector("h3")?.textContent || "";
        if (!h.includes(strip(p.title).slice(0, 30))) out.push("title " + id);
      }
    }
    return out;
  });
  expect(bad).toEqual([]);
  expect(errors).toEqual([]);
});

test("U11 where-to-click: a duplicated label uses the per-test override; no home paths in the page", async ({
  page,
}) => {
  await open(page);
  const withWhere = await page.evaluate(() =>
    window.ALL.filter((t) =>
      Array.isArray((t as unknown as { where?: unknown[] }).where),
    ).map((t) => ({
      id: t.id,
      w: (t as unknown as { where: string[] }).where,
    })),
  );
  expect(withWhere.length).toBeGreaterThan(0);
  for (const { id, w } of withWhere.slice(0, 40)) {
    const text = await page.locator(`#t-${id} .where`).first().textContent();
    expect(text, `where text for ${id}`).toContain(w[0]);
  }
  const body = await page.locator("body").textContent();
  expect(body).not.toMatch(/\/Users\/|\/home\//);
});
