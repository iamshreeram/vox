import { test, expect, type Page } from "@playwright/test";
import { appendFileSync, mkdirSync } from "node:fs";
import {
  emitMockEvent,
  getMockAppearance,
  getMockEmits,
  installTauriMock,
} from "./fixtures/tauriMock";
import { CHAT_VARIABLE_NAMES } from "../src/chat/chatTheme";
import * as Q from "./helpers/uiQuality";

/*
 * UI quality gate (UI enterprise S5, DESIGN Revision 3 D5 / A6 / A6c). Locked: this spec is the
 * objective measure of the audit-fix loop. Failures on real pages are REAL DEFECTS: every
 * assertion is strict, nothing is skipped and there is no waiver list. Findings are written to
 * /tmp/voxui-r1/ (screenshots and findings.jsonl); FINDINGS-R1.md is compiled from them.
 *
 * Essential motion exceptions: none (see ESSENTIAL_MOTION in helpers/uiQuality.ts, empty).
 */

const OUT = "/tmp/voxui-r1";
mkdirSync(OUT, { recursive: true });

const SIDEBAR_ITEM = "p.text-sm.font-medium.truncate[title]";
const ALL_GATES = {
  post_process_enabled: true,
  experimental_enabled: true,
  debug_mode: true,
};
const CHAT_PAGE = "/src/chat/index.html";
const ORB_PAGE = "/src/orb/index.html";
const BUBBLES_PAGE = "/src/bubbles/index.html?instance=7";
const OVERLAY_PAGE = "/src/overlay/index.html";

/** Settings matrix (A6 / D5): themes x style with accent amber; blue for Light and Dark only. */
const SETTINGS_COMBOS = [
  { theme: "light", style: "vibrant", accent: "amber" },
  { theme: "light", style: "restrained", accent: "amber" },
  { theme: "dark", style: "vibrant", accent: "amber" },
  { theme: "dark", style: "restrained", accent: "amber" },
  { theme: "aurora", style: "vibrant", accent: "amber" },
  { theme: "aurora", style: "restrained", accent: "amber" },
  { theme: "light", style: "vibrant", accent: "blue" },
  { theme: "dark", style: "vibrant", accent: "blue" },
] as const;

/** Chat matrix: chat_theme x opacity. follow_app is evaluated with both main-window buckets. */
const CHAT_COMBOS = [
  { chatTheme: "dark", theme: "dark" },
  { chatTheme: "light", theme: "light" },
  { chatTheme: "follow_app", theme: "light" },
  { chatTheme: "follow_app", theme: "dark" },
] as const;
const CHAT_OPACITIES = [20, 100] as const;

const CONVERSATION = [
  {
    id: 1,
    tsMs: 1700000001000,
    role: "user",
    text: "What's on my calendar today?",
  },
  {
    id: 2,
    tsMs: 1700000002000,
    role: "assistant",
    text: "You have three meetings: standup at 9, design review at 11, and a 1:1 at 3.",
  },
  {
    id: 3,
    tsMs: 1700000003000,
    role: "user",
    text: "Summarize the review notes.",
  },
  {
    id: 4,
    tsMs: 1700000004000,
    role: "assistant",
    text: "The review covered the settings redesign. Open items: sidebar contrast on light backgrounds.",
  },
  {
    id: 5,
    tsMs: 1700000005000,
    role: "system",
    text: "Error: the assistant could not reach the model. Check the Brain settings and try again.",
  },
];

type Violation = {
  kind:
    | "contrast"
    | "font"
    | "focus"
    | "keyboard"
    | "name"
    | "size"
    | "overflow"
    | "motion"
    | "blur"
    | "console"
    | "convergence";
  surface: string;
  variant: string;
  selector: string;
  measured: string;
  required: string;
};

const KINDS: Violation["kind"][] = [
  "contrast",
  "font",
  "focus",
  "keyboard",
  "name",
  "size",
  "overflow",
  "motion",
  "blur",
  "console",
  "convergence",
];

/* ------------------------------------------------------------------ in-page collectors */
// Every function below runs inside the page through page.evaluate: each is self-contained.

interface TextRaw {
  text: string;
  selector: string;
  color: string;
  fontSizePx: number;
  fontWeight: number;
  opacity: number;
  layers: Q.LayerCase[];
  filters: string[];
}

interface InteractiveRaw {
  qid: string;
  tag: string;
  role: string | null;
  enabled: boolean;
  visible: boolean;
  selector: string;
  parts: Q.NameParts;
  width: number;
  height: number;
}

interface PageData {
  texts: TextRaw[];
  interactive: InteractiveRaw[];
  blur: Array<{ selector: string; backdropFilter: string; filter: string }>;
  motion: Q.MotionSample[];
  overflow: { docWidth: number; viewport: number; offenders: string[] };
  appearance: {
    theme: string | null;
    accent: string | null;
    style: string | null;
  };
}

function collectPage(): PageData {
  const describeEl = (el: Element): string => {
    const cls =
      typeof el.className === "string"
        ? el.className.split(/\s+/).filter(Boolean).slice(0, 2).join(".")
        : "";
    return (
      el.tagName.toLowerCase() +
      (el.id ? "#" + el.id : "") +
      (cls ? "." + cls : "")
    );
  };
  const chainOf = (el: Element): Element[] => {
    const chain: Element[] = [];
    for (let n: Element | null = el; n; n = n.parentElement) chain.unshift(n);
    return chain;
  };
  const layersOf = (el: Element) => {
    let cum = 1;
    const layers: Q.LayerCase[] = [];
    const filters: string[] = [];
    for (const n of chainOf(el)) {
      const s = getComputedStyle(n);
      cum *= parseFloat(s.opacity);
      if (s.filter !== "none") filters.push(s.filter);
      layers.push({
        background: s.backgroundColor,
        backgroundImage: s.backgroundImage,
        opacity: cum,
        mixBlendMode: s.mixBlendMode,
        maskImage: s.maskImage,
        backdropFilter: s.backdropFilter,
        filter: s.filter,
      });
    }
    return { layers, filters, opacity: cum };
  };
  const visibleEl = (el: Element): boolean => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return (
      r.width > 0 &&
      r.height > 0 &&
      s.visibility !== "hidden" &&
      s.display !== "none"
    );
  };

  const texts: TextRaw[] = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node: Node | null;
  while ((node = walker.nextNode())) {
    const raw = (node.textContent ?? "").replace(/\s+/g, " ").trim();
    const el = node.parentElement;
    if (
      !raw ||
      !el ||
      el.closest("script,style,noscript,template") ||
      !visibleEl(el)
    )
      continue;
    // Visually hidden text (sr-only: 1px boxes) is not rendered content; its name is covered by the name check.
    const box = el.getBoundingClientRect();
    if (box.width <= 1 || box.height <= 1) continue;
    const s = getComputedStyle(el);
    const { layers, filters, opacity } = layersOf(el);
    texts.push({
      text: raw.slice(0, 60),
      selector: describeEl(el),
      color: s.color,
      fontSizePx: parseFloat(s.fontSize),
      fontWeight: parseFloat(s.fontWeight),
      opacity,
      layers,
      filters,
    });
  }

  const interactiveEls = Array.from(
    document.querySelectorAll<HTMLElement>(
      "button, a[href], input:not([type=hidden]), select, textarea, summary, [role=button], [role=switch], [role=radio], [role=slider], [role=tab], [role=checkbox]",
    ),
  );
  const interactive: InteractiveRaw[] = interactiveEls.map((el, i) => {
    el.setAttribute("data-qa-id", "q" + i);
    const r = el.getBoundingClientRect();
    const labelledBy = (el.getAttribute("aria-labelledby") ?? "")
      .split(/\s+/)
      .filter(Boolean)
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ");
    const labelText =
      el instanceof HTMLInputElement ||
      el instanceof HTMLSelectElement ||
      el instanceof HTMLTextAreaElement
        ? Array.from(el.labels ?? [])
            .map((l) => l.textContent ?? "")
            .join(" ")
        : "";
    const isInput = el instanceof HTMLInputElement;
    const visibleText = isInput ? "" : (el.textContent ?? "");
    const alt = el.querySelector("img[alt]")?.getAttribute("alt") ?? undefined;
    return {
      qid: "q" + i,
      tag: el.tagName.toLowerCase(),
      role: el.getAttribute("role"),
      enabled:
        !(el as HTMLButtonElement).disabled &&
        el.getAttribute("aria-disabled") !== "true" &&
        !el.closest("[inert]"),
      visible: visibleEl(el),
      selector: describeEl(el),
      parts: {
        labelledBy,
        ariaLabel: el.getAttribute("aria-label") ?? undefined,
        labelText,
        visibleText,
        alt,
        title: el.getAttribute("title") ?? undefined,
      },
      width: r.width,
      height: r.height,
    };
  });

  const blur: PageData["blur"] = [];
  const motion: Q.MotionSample[] = [];
  const all = Array.from(document.querySelectorAll("*"));
  for (const el of all) {
    const s = getComputedStyle(el);
    if (s.backdropFilter !== "none" || /blur\(/i.test(s.filter)) {
      blur.push({
        selector: describeEl(el),
        backdropFilter: s.backdropFilter,
        filter: s.filter,
      });
    }
    const animated = s.animationName !== "none";
    const transitioned = /[1-9]/.test(s.transitionDuration);
    if (animated || transitioned) {
      motion.push({
        selector: describeEl(el),
        animationName: s.animationName,
        animationDuration: s.animationDuration,
        transitionProperty: s.transitionProperty,
        transitionDuration: s.transitionDuration,
      });
    }
  }

  const root = document.documentElement;
  const viewport = root.clientWidth;
  const offenders: string[] = [];
  for (const el of all) {
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.right <= viewport + 1 || !visibleEl(el)) continue;
    let clipped = false;
    for (
      let n: Element | null = el.parentElement;
      n && n !== document.body;
      n = n.parentElement
    ) {
      const ox = getComputedStyle(n).overflowX;
      if (ox !== "visible") {
        clipped = true;
        break;
      }
    }
    if (!clipped && offenders.length < 5) offenders.push(describeEl(el));
  }

  return {
    texts,
    interactive,
    blur,
    motion,
    overflow: { docWidth: root.scrollWidth, viewport, offenders },
    appearance: {
      theme: root.dataset.theme ?? null,
      accent: root.dataset.accent ?? null,
      style: root.dataset.style ?? null,
    },
  };
}

interface FocusObs {
  qid: string | null;
  tag: string;
  role: string | null;
  focusVisible: boolean;
  focused: Q.FocusStyle;
  base: Q.FocusStyle;
  layers: Q.LayerCase[];
  groupRole: string | null;
  orientation: string | null;
  groupMembers: string[];
}

/** Focuses the first enabled, visible interactive element in DOM order. Returns its qid or null. */
function focusFirstEnabled(): string | null {
  const candidates = Array.from(
    document.querySelectorAll<HTMLElement>("[data-qa-id]"),
  );
  for (const el of candidates) {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    const enabled =
      !(el as HTMLButtonElement).disabled &&
      el.getAttribute("aria-disabled") !== "true";
    if (enabled && r.width > 0 && r.height > 0 && s.visibility !== "hidden") {
      el.focus();
      return el.getAttribute("data-qa-id");
    }
  }
  return null;
}

/** The focused element's focus styles (focused and base), its backgrounds and its roving group. */
function observeFocus(): FocusObs {
  const el = document.activeElement as HTMLElement | null;
  if (!el || el === document.body) {
    return {
      qid: null,
      tag: "body",
      role: null,
      focusVisible: false,
      focused: {
        outlineStyle: "",
        outlineWidth: "",
        outlineColor: "",
        boxShadow: "",
      },
      base: {
        outlineStyle: "",
        outlineWidth: "",
        outlineColor: "",
        boxShadow: "",
      },
      layers: [],
      groupRole: null,
      orientation: null,
      groupMembers: [],
    };
  }
  const style = (node: Element): Q.FocusStyle => {
    const s = getComputedStyle(node);
    return {
      outlineStyle: s.outlineStyle,
      outlineWidth: s.outlineWidth,
      outlineColor: s.outlineColor,
      boxShadow: s.boxShadow,
    };
  };
  const focused = style(el);
  const focusVisible = el.matches(":focus-visible");
  el.blur();
  const base = style(el);
  el.focus();

  let cum = 1;
  const layers: Q.LayerCase[] = [];
  const chain: Element[] = [];
  for (let n: Element | null = el; n; n = n.parentElement) chain.unshift(n);
  for (const n of chain) {
    const s = getComputedStyle(n);
    cum *= parseFloat(s.opacity);
    layers.push({
      background: s.backgroundColor,
      backgroundImage: s.backgroundImage,
      opacity: cum,
      mixBlendMode: s.mixBlendMode,
      maskImage: s.maskImage,
      backdropFilter: s.backdropFilter,
      filter: s.filter,
    });
  }

  let groupRole: string | null = null;
  let orientation: string | null = null;
  let members: Element[] = [];
  const group = el.closest("[role=radiogroup],[role=tablist]");
  if (group) {
    groupRole = group.getAttribute("role");
    orientation = group.getAttribute("aria-orientation");
    members = Array.from(group.querySelectorAll("[role=radio],[role=tab]"));
  } else if (el instanceof HTMLInputElement && el.type === "radio" && el.name) {
    groupRole = "radiogroup";
    members = Array.from(
      document.querySelectorAll<HTMLInputElement>(
        `input[type=radio][name="${el.name}"]`,
      ),
    );
  }
  return {
    qid: el.getAttribute("data-qa-id"),
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute("role"),
    focusVisible,
    focused,
    base,
    layers,
    groupRole,
    orientation,
    groupMembers: members
      .map((m) => m.getAttribute("data-qa-id") ?? "")
      .filter(Boolean),
  };
}

/* ------------------------------------------------------------------ harness helpers */

function findingsSink(
  surface: string,
  variant: string,
  violations: Violation[],
  screenshot: string | null,
): void {
  for (const v of violations) {
    appendFileSync(
      `${OUT}/findings.jsonl`,
      `${JSON.stringify({ ...v, screenshot })}\n`,
    );
  }
}

const slug = (s: string): string =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

/** Walks the Tab order from the first enabled element; arrow keys cover roving groups. */
async function keyboardWalk(
  page: Page,
): Promise<{ visited: string[]; obs: Map<string, FocusObs>; steps: number }> {
  await page.keyboard.press("Shift");
  const start = await page.evaluate(focusFirstEnabled);
  let current = (await page.evaluate(observeFocus)) as FocusObs;
  const visited: string[] = [];
  const obs = new Map<string, FocusObs>();
  const record = (o: FocusObs) => {
    if (o.qid && !visited.includes(o.qid)) {
      visited.push(o.qid);
      obs.set(o.qid, o);
    }
  };
  let steps = 0;
  if (start === null) return { visited, obs, steps };
  record(current);
  const startQid = current.qid;
  while (steps < Q.MAX_TAB_STEPS) {
    await page.keyboard.press("Tab");
    steps += 1;
    current = (await page.evaluate(observeFocus)) as FocusObs;
    if (current.tag === "body") break;
    if (current.qid !== null && current.qid === startQid) break;
    record(current);
    const isRoving =
      current.role === "radio" ||
      current.role === "tab" ||
      current.tag === "input";
    const key = Q.rovingArrowKey(current.groupRole ?? "", current.orientation);
    if (isRoving && key !== null && current.groupMembers.length > 1) {
      while (
        steps < Q.MAX_TAB_STEPS &&
        current.groupMembers.some((m) => !visited.includes(m))
      ) {
        await page.keyboard.press(key);
        steps += 1;
        const next = (await page.evaluate(observeFocus)) as FocusObs;
        if (next.qid === null || next.qid === current.qid) break;
        record(next);
        current = next;
      }
    }
  }
  return { visited, obs, steps };
}

/** Full audit of one page state. Returns violations; soft-asserts every kind; writes findings. */
/**
 * Every colour string the text audit of one page needs: text colours and every chain element's
 * background and gradient stops. Resolved once in the page (Q.resolveColourMap), so no colour is
 * ever parsed by regex. Focus colours are collected from the keyboard walk (see focusColourStrings).
 */
function textColourStrings(data: PageData): string[] {
  const out: string[] = [];
  for (const t of data.texts) {
    out.push(...Q.candidateColours(t.layers, [t.color]));
  }
  return out;
}

/** Colour strings for the focus-ring checks of the observed keyboard-walk elements. */
function focusColourStrings(observed: Iterable<FocusObs>): string[] {
  const out: string[] = [];
  for (const o of observed) {
    out.push(...Q.candidateColours(o.layers as Q.LayerCase[]));
    const ring = Q.focusRingText(o.focused, o.base);
    if (ring !== null) out.push(ring);
  }
  return out;
}

async function auditVisit(
  page: Page,
  collector: Q.ConsoleErrorCollector,
  surface: string,
  variant: string,
  opts: { walk: boolean },
): Promise<void> {
  const data = (await page.evaluate(collectPage)) as PageData;
  const v: Violation[] = [];
  const add = (
    kind: Violation["kind"],
    selector: string,
    measured: string,
    required: string,
  ) => v.push({ kind, surface, variant, selector, measured, required });

  const resolve = await Q.resolveColourMap(page, textColourStrings(data));
  const seen = new Set<string>();
  for (const t of data.texts) {
    const key = JSON.stringify([
      t.color,
      t.fontSizePx,
      t.fontWeight,
      t.opacity,
      t.layers,
      t.filters,
    ]);
    if (seen.has(key)) continue;
    seen.add(key);
    const verdict = Q.evaluateTextContrast(t as Q.TextCase, resolve);
    if (!verdict.pass) {
      add(
        "contrast",
        `${t.selector} "${t.text}"`,
        verdict.reason ?? verdict.ratio.toFixed(2),
        `>= ${verdict.required}:1 (${verdict.backdrop})`,
      );
    }
    if (!Q.fontSizeOk(t.fontSizePx)) {
      add(
        "font",
        `${t.selector} "${t.text}"`,
        `${t.fontSizePx}px`,
        `>= ${Q.MIN_FONT_PX}px`,
      );
    }
  }

  for (const el of data.interactive) {
    if (!el.visible || !el.enabled) continue;
    const isNamed =
      ["button", "input", "select", "textarea"].includes(el.tag) ||
      ["button", "switch", "radio", "slider", "tab", "checkbox"].includes(
        el.role ?? "",
      );
    if (isNamed) {
      const name = Q.checkAccessibleName(el.parts);
      if (!name.pass)
        add("name", el.selector, "no accessible name", "non-empty name");
    }
    if (!Q.targetSizeOk(el.width, el.height)) {
      add(
        "size",
        el.selector,
        `${el.width.toFixed(0)}x${el.height.toFixed(0)}px`,
        `>= ${Q.MIN_TARGET_PX}x${Q.MIN_TARGET_PX}px`,
      );
    }
  }

  const overflow = Q.overflowPx(data.overflow.docWidth, data.overflow.viewport);
  if (overflow > 0) add("overflow", "document", `${overflow}px`, "0px");
  for (const o of data.overflow.offenders)
    add("overflow", o, "extends past the viewport", "inside viewport");

  for (const m of data.motion) {
    const reason = Q.motionViolation(m);
    if (reason !== null)
      add(
        "motion",
        m.selector,
        reason,
        `<= ${Q.MAX_REDUCED_MOTION_SECONDS}s under reduced motion`,
      );
  }
  for (const b of data.blur) {
    const reason = Q.blurViolation(b.backdropFilter, b.filter);
    if (reason !== null)
      add("blur", b.selector, reason, "no blur or backdrop-filter");
  }

  if (opts.walk) {
    const enabledIds = data.interactive
      .filter((e) => e.visible && e.enabled)
      .map((e) => e.qid);
    const walk = await keyboardWalk(page);
    const focusResolve = await Q.resolveColourMap(
      page,
      focusColourStrings(walk.obs.values()),
    );
    const reach = Q.analyzeTabWalk(enabledIds, walk.visited, walk.steps);
    for (const id of reach.unreached) {
      const el = data.interactive.find((e) => e.qid === id);
      add(
        "keyboard",
        el?.selector ?? id,
        "not reached by Tab or arrow keys",
        "reachable",
      );
    }
    if (reach.capped)
      add(
        "keyboard",
        "walk",
        `step cap ${Q.MAX_TAB_STEPS} reached`,
        `<= ${Q.MAX_TAB_STEPS} steps`,
      );
    for (const o of walk.obs.values()) {
      if (o.tag === "body") continue;
      if (!o.focusVisible)
        add(
          "focus",
          o.tag,
          "focus not :focus-visible on keyboard focus",
          ":focus-visible",
        );
      {
        const ring = Q.checkFocusRing(
          o.focused,
          o.base,
          o.layers as Q.LayerCase[],
          focusResolve,
        );
        if (!ring.pass)
          add(
            "focus",
            o.tag + (o.qid ? ` [${o.qid}]` : ""),
            ring.reason ?? "no ring",
            `ring >= ${Q.MIN_FOCUS_RING_RATIO}:1`,
          );
      }
    }
  }

  for (const message of collector.drain())
    add("console", "console", message.slice(0, 160), "no console errors");

  const counts = Object.fromEntries(
    KINDS.map((k) => [k, v.filter((x) => x.kind === k).length]),
  ) as Record<string, number>;
  appendFileSync(
    `${OUT}/visits.jsonl`,
    `${JSON.stringify({ surface, variant, texts: data.texts.length, interactive: data.interactive.length, counts })}\n`,
  );

  let shot: string | null = null;
  if (v.length > 0) {
    shot = `${OUT}/${slug(surface + "-" + variant)}.png`;
    await page.screenshot({ path: shot, fullPage: false, timeout: 20_000 });
  }
  findingsSink(surface, variant, v, shot);
  for (const kind of KINDS) {
    const list = v.filter((x) => x.kind === kind);
    const first = list[0];
    const detail = first
      ? ` first: ${first.selector} = ${first.measured} (need ${first.required})`
      : "";
    expect
      .soft(list.length, `${surface} [${variant}] ${kind}${detail}`)
      .toBe(0);
  }
}

/** Keeps the sidebar visits independent: a walk that changed the appearance reloads the page. */
async function appearanceOf(page: Page): Promise<string> {
  return JSON.stringify(
    await page.evaluate(() => ({
      t: document.documentElement.dataset.theme,
      a: document.documentElement.dataset.accent,
      s: document.documentElement.dataset.style,
    })),
  );
}

function seedUrl(path: string, snapshot: unknown, extra = ""): string {
  return `${path}${path.includes("?") ? "&" : "?"}${extra}ap=${encodeURIComponent(JSON.stringify(snapshot))}`;
}

function snapshotFor(
  theme: string,
  chatTheme = "dark",
  accent = "amber",
  style = "vibrant",
  rev = 1,
): Record<string, unknown> {
  return {
    rev,
    theme,
    custom_colors: null,
    accent,
    chat_theme: chatTheme,
    visual_style: style,
  };
}

async function observeConsumer(page: Page): Promise<Q.ObservedAppearance> {
  return page.evaluate(
    (names: string[]) => {
      const root = document.documentElement;
      const chatVariables: Record<string, string> = {};
      for (const n of names) chatVariables[n] = root.style.getPropertyValue(n);
      return {
        dataTheme: root.dataset.theme ?? null,
        dataAccent: root.dataset.accent ?? null,
        dataStyle: root.dataset.style ?? null,
        chatVariables,
      };
    },
    [...CHAT_VARIABLE_NAMES],
  );
}

/** Convergence of one consumer page against the committed snapshot. Empty array = converged. */
function consumerMismatches(
  label: string,
  snapshot: Parameters<typeof Q.expectedAppearance>[0],
  observed: Q.ObservedAppearance,
): string[] {
  const expected = Q.expectedAppearance(snapshot, false);
  if (label !== "chat") expected.chatVariables = {};
  return Q.convergenceMismatches(expected, observed);
}

/* ------------------------------------------------------------------ matrix: settings */

test.describe("UI quality gate: settings (every page, 1100x760)", () => {
  // Declared budget: 8 settings combos + 1 narrow pass + chat + consumers + convergence < 6 minutes.
  test.setTimeout(150_000);

  for (const combo of SETTINGS_COMBOS) {
    const variant = `${combo.theme}/${combo.style}/${combo.accent}`;
    test(`settings ${variant}`, async ({ page }) => {
      const collector = new Q.ConsoleErrorCollector();
      collector.attach(page);
      await page.emulateMedia({
        reducedMotion: "reduce",
        colorScheme: "light",
      });
      await page.setViewportSize({ width: 1100, height: 760 });
      await installTauriMock(page, {
        settings: {
          ...ALL_GATES,
          theme: combo.theme,
          visual_style: combo.style,
          accent: combo.accent,
        },
      });
      await page.goto("/");
      const items = page.locator(SIDEBAR_ITEM);
      await items.first().waitFor({ timeout: 30_000 });
      const titles = await items.evaluateAll((els) =>
        els.map((el) => el.getAttribute("title") ?? ""),
      );
      for (let i = 0; i < titles.length; i += 1) {
        if (i > 0) await items.nth(i).click({ timeout: 10_000 });
        await page.waitForTimeout(250);
        const before = await appearanceOf(page);
        await auditVisit(
          page,
          collector,
          "settings",
          `${variant}/${titles[i]}`,
          { walk: true },
        );
        if ((await appearanceOf(page)) !== before) {
          // The walk moved a roving selection (theme, accent or style): restore the page for the next visit.
          await page.goto("/");
          await items.first().waitFor({ timeout: 30_000 });
          await items.nth(i).click({ timeout: 10_000 });
        }
      }
    });
  }

  test("settings at 760x600: no horizontal overflow on any page", async ({
    page,
  }) => {
    test.setTimeout(90_000);
    const collector = new Q.ConsoleErrorCollector();
    collector.attach(page);
    await page.emulateMedia({ reducedMotion: "reduce", colorScheme: "light" });
    await page.setViewportSize({ width: 760, height: 600 });
    await installTauriMock(page, {
      settings: {
        ...ALL_GATES,
        theme: "light",
        visual_style: "vibrant",
        accent: "amber",
      },
    });
    await page.goto("/");
    const items = page.locator(SIDEBAR_ITEM);
    await items.first().waitFor({ timeout: 30_000 });
    const count = await items.count();
    for (let i = 0; i < count; i += 1) {
      await items.nth(i).click({ timeout: 10_000 });
      await page.waitForTimeout(250);
      const title = (await items.nth(i).getAttribute("title")) ?? String(i);
      await auditVisit(
        page,
        collector,
        "settings-760x600",
        `light/vibrant/amber/${title}`,
        { walk: false },
      );
    }
  });
});

/* ------------------------------------------------------------------ matrix: chat */

test.describe("UI quality gate: chat window (480x720, overflow 360x420)", () => {
  test.setTimeout(120_000);
  for (const combo of CHAT_COMBOS) {
    for (const opacity of CHAT_OPACITIES) {
      const variant = `${combo.chatTheme}/theme-${combo.theme}/opacity-${opacity}`;
      test(`chat ${variant}`, async ({ page }) => {
        const collector = new Q.ConsoleErrorCollector();
        collector.attach(page);
        await page.emulateMedia({
          reducedMotion: "reduce",
          colorScheme: "light",
        });
        const snapshot = snapshotFor(combo.theme, combo.chatTheme);
        const states: Array<{
          name: string;
          entries: unknown[];
          state: string;
        }> = [
          { name: "populated", entries: CONVERSATION, state: "idle" },
          { name: "empty", entries: [], state: "idle" },
        ];
        for (const s of states) {
          await page.setViewportSize({ width: 480, height: 720 });
          await installTauriMock(page, {
            label: "chat",
            commands: {
              get_chat_snapshot: {
                seq: 1,
                entries: s.entries,
                state: s.state,
                end: null,
                recording: true,
              },
            },
          });
          await page.goto(
            seedUrl(CHAT_PAGE, snapshot, `opacity=${opacity}&rev=1&`),
          );
          await page.waitForTimeout(400);
          await auditVisit(page, collector, "chat", `${variant}/${s.name}`, {
            walk: true,
          });
        }
        // Narrow: populated conversation at 360x420 (A6: no horizontal overflow).
        await page.setViewportSize({ width: 360, height: 420 });
        await page.waitForTimeout(300);
        await auditVisit(
          page,
          collector,
          "chat-360x420",
          `${variant}/populated`,
          { walk: false },
        );
      });
    }
  }
});

/* ------------------------------------------------------------------ matrix: orb, bubbles, overlay */

test.describe("UI quality gate: orb, bubbles and overlay (light and dark)", () => {
  test.setTimeout(120_000);
  for (const theme of ["light", "dark"] as const) {
    test(`orb states ${theme}`, async ({ page }) => {
      const collector = new Q.ConsoleErrorCollector();
      collector.attach(page);
      await page.emulateMedia({
        reducedMotion: "reduce",
        colorScheme: "light",
      });
      await page.setViewportSize({ width: 300, height: 300 });
      await installTauriMock(page, {
        label: "orb",
        commands: { orb_get_state: { face: "idle" } },
      });
      await page.goto(seedUrl(ORB_PAGE, snapshotFor(theme)));
      await page.getByTestId("orb").waitFor({ timeout: 20_000 });
      await auditVisit(page, collector, "orb", `${theme}/idle`, { walk: true });
      let seq = 1;
      for (const state of [
        "listening",
        "thinking",
        "acting",
        "speaking",
        "concerned",
      ]) {
        await emitMockEvent(page, "converse-chat", {
          seq: seq++,
          kind: { type: "state", state, end: null },
        });
        await page.waitForTimeout(600);
        await auditVisit(page, collector, "orb", `${theme}/${state}`, {
          walk: true,
        });
      }
    });

    test(`bubbles ${theme}`, async ({ page }) => {
      const collector = new Q.ConsoleErrorCollector();
      collector.attach(page);
      await page.emulateMedia({
        reducedMotion: "reduce",
        colorScheme: "light",
      });
      await page.setViewportSize({ width: 360, height: 520 });
      await installTauriMock(page, { label: "orb-bubbles" });
      await page.goto(seedUrl(BUBBLES_PAGE, snapshotFor(theme)));
      await page.waitForTimeout(600);
      await auditVisit(page, collector, "bubbles", `${theme}/empty`, {
        walk: true,
      });
      await emitMockEvent(page, "bubbles-state", {
        instance: 7,
        rev: 1,
        items: [
          {
            id: 1,
            role: "user",
            text: "Remind me about the design review.",
            age_ms: 0,
            ttl_ms: 600000,
          },
          {
            id: 2,
            role: "assistant",
            text: "Done. It's set for 11:00 today.",
            age_ms: 0,
            ttl_ms: 600000,
          },
        ],
      });
      await page.waitForTimeout(600);
      await auditVisit(page, collector, "bubbles", `${theme}/populated`, {
        walk: true,
      });
    });

    test(`overlay states ${theme}`, async ({ page }) => {
      const collector = new Q.ConsoleErrorCollector();
      collector.attach(page);
      await page.emulateMedia({
        reducedMotion: "reduce",
        colorScheme: "light",
      });
      await page.setViewportSize({ width: 420, height: 140 });
      await installTauriMock(page, {
        settings: { theme, visual_style: "vibrant", accent: "amber" },
      });
      await page.goto(OVERLAY_PAGE);
      await page.waitForTimeout(800);
      for (const state of [
        "recording",
        "transcribing",
        "processing",
        "speaking",
      ]) {
        await emitMockEvent(page, "show-overlay", state);
        await page.waitForTimeout(500);
        await auditVisit(page, collector, "overlay", `${theme}/${state}`, {
          walk: true,
        });
      }
    });
  }
});

/* ------------------------------------------------------------------ appearance convergence */

test.describe("appearance convergence (events only, consumer pages)", () => {
  test.setTimeout(120_000);

  test("consumers converge after commits, a dropped delivery, a replay and a stale reorder", async ({
    browser,
  }) => {
    const context = await browser.newContext({ colorScheme: "light" });
    const main = await context.newPage();
    await installTauriMock(main, {
      label: "main",
      settings: {
        theme: "light",
        accent: "amber",
        chat_theme: "dark",
        visual_style: "vibrant",
      },
    });
    await main.goto("/");
    await main.locator(SIDEBAR_ITEM).first().waitFor({ timeout: 30_000 });
    const seed = await getMockAppearance(main);

    const chat = await context.newPage();
    await installTauriMock(chat, {
      label: "chat",
      commands: {
        get_chat_snapshot: {
          seq: 1,
          entries: [],
          state: "idle",
          end: null,
          recording: true,
        },
      },
    });
    await chat.goto(seedUrl(CHAT_PAGE, seed, "opacity=100&rev=1&"));
    const orb = await context.newPage();
    await installTauriMock(orb, {
      label: "orb",
      commands: { orb_get_state: { face: "idle" } },
    });
    await orb.goto(seedUrl(ORB_PAGE, seed));
    const bubbles = await context.newPage();
    await installTauriMock(bubbles, { label: "orb-bubbles" });
    await bubbles.goto(seedUrl(BUBBLES_PAGE, seed));
    const consumers: Record<string, Page> = {
      chat,
      orb,
      "orb-bubbles": bubbles,
    };

    const commit = async (command: string, args: Record<string, unknown>) => {
      await main.evaluate(
        ([c, a]) =>
          (
            window as unknown as {
              __TAURI_INTERNALS__: {
                invoke: (c: string, a: unknown) => Promise<unknown>;
              };
            }
          ).__TAURI_INTERNALS__.invoke(c, a),
        [command, args],
      );
    };
    const cursor = { next: 0 };
    /** Forwards every committed emit (from the cursor) to the labels in `only`. Returns the delivered count. */
    const forward = async (only: string[]): Promise<void> => {
      const emits = await getMockEmits(main);
      for (const e of emits.slice(cursor.next)) {
        if (only.includes(e.label))
          await emitMockEvent(consumers[e.label], e.event, e.payload);
      }
      cursor.next = emits.length;
    };
    const converged = async (label: string) => {
      const snapshot = await getMockAppearance(main);
      await expect
        .poll(
          async () =>
            consumerMismatches(
              label,
              snapshot,
              await observeConsumer(consumers[label]),
            ),
          { timeout: 10_000 },
        )
        .toEqual([]);
    };
    const all = ["chat", "orb", "orb-bubbles"];

    // 1. A commit reaches every consumer through the event.
    await commit("change_accent_setting", { accent: "blue" });
    await forward(all);
    for (const label of all) await converged(label);

    // 2. A dropped delivery: orb and bubbles miss rev 3; the chat still converges.
    await commit("change_visual_style_setting", { visualStyle: "restrained" });
    await forward(["chat"]);
    await converged("chat");
    expect(
      consumerMismatches(
        "orb",
        await getMockAppearance(main),
        await observeConsumer(orb),
      ).length,
    ).toBeGreaterThan(0);
    // 3. The replay (second emit of the committed snapshot) converges them.
    const latest = (await getMockEmits(main)).filter((e) => e.label !== "chat");
    for (const e of latest.slice(-2))
      await emitMockEvent(consumers[e.label], e.event, e.payload);
    for (const label of all) await converged(label);

    // 4. A stale reorder: rev 2 arrives after rev 4 and must not win.
    const oldest = (await getMockEmits(main))[0];
    await emitMockEvent(chat, oldest.event, oldest.payload);
    await converged("chat");
    await context.close();
  });
});

/* ------------------------------------------------------------------ negative tests (must FAIL the gate) */

test.describe("negative fixtures: the gate must detect each injected defect", () => {
  test.setTimeout(60_000);

  test("NEG low-contrast gradient text is flagged", async ({ page }) => {
    await page.setContent(
      '<body style="margin:0"><p id="t" style="color:#767676;background:linear-gradient(#000,#fff);font-size:16px;padding:20px">Gradient text</p></body>',
    );
    const data = (await page.evaluate(collectPage)) as PageData;
    const text = data.texts.find((t) => t.text === "Gradient text");
    expect(text, "fixture text found").toBeDefined();
    const resolve = await Q.resolveColourMap(page, textColourStrings(data));
    const verdict = Q.evaluateTextContrast(text as Q.TextCase, resolve);
    expect(
      verdict.pass,
      "the gate must reject #767676 on a black-to-white gradient",
    ).toBe(false);
  });

  test("NEG transparent focus ring is flagged", async ({ page }) => {
    await page.setContent(
      '<style>button:focus-visible{outline:2px solid transparent;outline-offset:2px}</style><body style="background:#fff"><button>Save</button></body>',
    );
    await page.keyboard.press("Shift");
    await page.keyboard.press("Tab");
    const obs = (await page.evaluate(observeFocus)) as FocusObs;
    expect(obs.tag, "the button must take focus").toBe("button");
    const ringText = Q.focusRingText(obs.focused, obs.base);
    const resolve = await Q.resolveColourMap(
      page,
      Q.candidateColours(
        obs.layers as Q.LayerCase[],
        ringText ? [ringText] : [],
      ),
    );
    const ring = Q.checkFocusRing(
      obs.focused,
      obs.base,
      obs.layers as Q.LayerCase[],
      resolve,
    );
    expect(ring.pass, "the gate must reject a transparent ring").toBe(false);
  });

  test("NEG unreachable enabled button is reported by the Tab walk", async ({
    page,
  }) => {
    await page.setContent(
      '<body><button>First</button><button tabindex="-1">Hidden from Tab</button><button>Last</button></body>',
    );
    const data = (await page.evaluate(collectPage)) as PageData;
    const enabledIds = data.interactive
      .filter((e) => e.visible && e.enabled)
      .map((e) => e.qid);
    const walk = await keyboardWalk(page);
    const reach = Q.analyzeTabWalk(enabledIds, walk.visited, walk.steps);
    expect(
      reach.unreached.length,
      "the gate must report the tabindex=-1 button",
    ).toBeGreaterThan(0);
  });

  test("NEG dropped appearance event is detected by the convergence check", async ({
    browser,
  }) => {
    const context = await browser.newContext();
    const main = await context.newPage();
    await installTauriMock(main, { label: "main" });
    await main.goto("/");
    const before = await getMockAppearance(main);
    await main.evaluate(() =>
      (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (c: string, a: unknown) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__.invoke("change_accent_setting", { accent: "rose" }),
    );
    const committed = await getMockAppearance(main);
    expect(committed.rev).toBeGreaterThan(before.rev);
    const orb = await context.newPage();
    await installTauriMock(orb, { label: "orb" });
    await orb.goto(seedUrl(ORB_PAGE, before));
    // The commit is never delivered to orb (dropped): the check must report it.
    const mismatches = consumerMismatches(
      "orb",
      committed as never,
      await observeConsumer(orb),
    );
    expect(
      mismatches.length,
      "the gate must report the dropped delivery",
    ).toBeGreaterThan(0);
    await context.close();
  });
});
