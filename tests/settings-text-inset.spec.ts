import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Generic runtime guard G. Mounted tests against the
 * Vite dev server with the Tauri mock. Every sidebar page is rendered at the main window size
 * (680x570), every CollapsibleSettingsGroup is opened, and four rules are measured relative to
 * the group's outer left edge (group.getBoundingClientRect().left):
 *
 *   G1 ROW LABELS      the first text node of every [data-setting-title] row: left in [12, 14.5].
 *   G2 TEXT BLOCKS     p/h2/h3/h4/li with their own text, outside rows, controls, the group header
 *                      and boxed ancestors: left in [12, 14.5].
 *   G3 PANELS          an outermost rounded panel directly in the group: box left and right
 *                      insets in [12, 14.5].
 *   G4 COMMAND MODE    with the reply-style composer in command mode, the acknowledgement label,
 *                      the acknowledgement warning and the composer mode label start within 1px.
 *
 * Text left = left of the first non-blank character (DOM Range). The spec FAILS on any offender
 * and writes the full offender and coverage report to <os tmp dir>/vox-text-inset-report/inset-report.json on
 * every run, pass or fail. Tests never weaken a rule; exemptions only come from ALLOW below.
 */

const OUT_DIR = join(tmpdir(), "vox-text-inset-report");
const REPORT_PATH = `${OUT_DIR}/inset-report.json`;
const COLUMN_MIN = 12;
const COLUMN_MAX = 14.5;
const RULE_TEXT = {
  G1: "row label text left in [12, 14.5] px from the group's outer left edge",
  G2: "text block (p/h2/h3/h4/li, outside rows, controls, header and boxed ancestors) left in [12, 14.5] px",
  G3: "outermost rounded panel: box left and right insets both in [12, 14.5] px",
  G4: "command mode: ack label, ack warning and composer mode label left within 1 px of each other",
} as const;

type Rule = keyof typeof RULE_TEXT;
type Theme = "light" | "dark";

interface SectionRef {
  id: string;
  label: string;
}

interface AllowEntry {
  /** CSS selector matched with element.matches() against G2 candidates. */
  selector: string;
  /** One-line reason, reported to the lead. */
  reason: string;
}

/**
 * Structural exemptions. Empty on purpose: every entry needs a triaged reason and is listed in
 * the report so the lead can judge it. Nothing is exempted by measured distance.
 */
const ALLOW: AllowEntry[] = [];

interface StateDef {
  id: string;
  title: string;
  theme: Theme;
  experimental: boolean;
  /** Sidebar section ids to visit, or "all" for every sidebar page. */
  sections: "all" | string[];
  settings?: Record<string, unknown>;
  commands?: Record<string, unknown>;
  /** Commands whose invoke rejects (the frontend's failure path). */
  rejectCommands?: string[];
  /** Runs after the groups are open and before measuring. */
  beforeMeasure?: (page: Page, sectionId: string) => Promise<void>;
  /** A string or element that must be visible on some visited page, else the state did not render. */
  marker?: { description: string; key?: string; selector?: string };
  /** The base run that must also prove the per-page coverage counts. */
  coverage?: boolean;
  /** How this state reaches the condition, for the report. */
  reach: string;
}

interface Offender {
  state: string;
  rule: Rule | "MEASURE";
  page: string;
  theme: Theme;
  viewport: string;
  group: string;
  selector: string;
  text: string;
  measured: string;
}

interface RunRecord {
  state: string;
  page: string;
  theme: Theme;
  viewport: string;
  status: "measured" | "NAV_FAILED";
  reason: string | null;
  groupsSeen: number;
  checked: Record<Rule, number>;
  allowedExempt: number;
  offenderCount: number;
}

interface BrowserOffender {
  rule: string;
  group: string;
  selector: string;
  text: string;
  measured: string;
}

interface BrowserMeasure {
  checked: { G1: number; G2: number; G3: number };
  offenders: BrowserOffender[];
  allowedExempt: number;
  allowHits: Record<string, number>;
  groupsSeen: number;
}

const VIEWPORT = { label: "680x570", width: 680, height: 570 } as const;

const NOT_INSTALLED = {
  supported: true,
  model: "not_installed",
  downloaded: 0,
  total: 0,
  health: "ready",
  failure: null,
};

const BASE_COMMANDS = {
  get_available_models: [
    {
      id: "model-a",
      name: "Audit Model Alpha",
      description: "A model used by the audit.",
      filename: "a.bin",
      source: "Local",
      size_mb: 10,
      is_downloaded: true,
      is_downloading: false,
      partial_size: 0,
      is_directory: false,
      engine_type: "TranscribeCpp",
      accuracy_score: 0.5,
      speed_score: 0.5,
      supports_translation: true,
      is_recommended: false,
      supported_languages: ["en", "de"],
      supports_language_selection: true,
      is_custom: false,
      supports_streaming: false,
      supports_language_detection: true,
    },
  ],
  get_current_model: "model-a",
  get_orb_support: { supported: true, block: null },
  phonon_voice_status: NOT_INSTALLED,
  natural_voice_status: NOT_INSTALLED,
};

function customPersonas(count: number): Record<string, unknown> {
  return {
    list_personas: Array.from({ length: count }, (_, i) => ({
      persona: {
        id: `audit-custom-${i}`,
        name: `Audit persona ${i}`,
        style: "Answer briefly.",
        voice: null,
      },
      builtin: false,
      active: i === 0,
    })),
  };
}

const BASE_SETTINGS = { vault_path: "" };

const STATES: StateDef[] = [
  {
    id: "base-light-680",
    title: "every sidebar page, experimental on, light",
    theme: "light",
    experimental: true,
    sections: "all",
    coverage: true,
    reach:
      "default settings with experimental_enabled, post_process_enabled and debug_mode on; vault_path empty (vault RAG cannot-enable note)",
  },
  {
    id: "models-dark",
    title: "Models page, dark",
    theme: "dark",
    experimental: true,
    sections: ["models"],
    reach: "dark colour scheme, Models only",
  },
  {
    id: "models-experimental-off",
    title: "Models page, experimental off (new-user default)",
    theme: "light",
    experimental: false,
    sections: ["models"],
    reach: "experimental_enabled false",
  },
  {
    id: "latency-panel-enabled",
    marker: {
      description: "latency placeholder",
      key: "converse.voice.latency.placeholder",
    },
    title: "latency panel placeholder",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { converse_latency_panel: true },
    reach: "setting converse_latency_panel = true",
  },
  {
    id: "smart-turn-aux-not-ready",
    marker: {
      description: "smart turn fallback note",
      key: "converse.voice.smartTurn.fallbackNote",
    },
    title: "smart turn on, aux model not installed (fallback note)",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { converse_smart_turn: true },
    commands: {
      aux_model_status: {
        kind: "smart_turn",
        state: "not_installed",
        size_bytes: 0,
      },
    },
    reach:
      "converse_smart_turn = true and aux_model_status(smart_turn) = not_installed",
  },
  {
    id: "speaker-filter-not-enrolled",
    marker: {
      description: "speaker filter inactive note",
      key: "converse.voice.speaker.inactiveNote",
    },
    title: "speaker filter on, not enrolled (inactive note)",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { speaker_filter_enabled: true },
    commands: {
      speaker_status: {
        enrolled: false,
        enrollment_active: false,
        model_ready: false,
      },
    },
    reach:
      "speaker_filter_enabled = true and speaker_status not enrolled, model not ready",
  },
  {
    id: "orb-support-rejects",
    marker: {
      description: "floating orb reason paragraph",
      selector: "[data-testid=floating-orb-reason]",
    },
    title: "floating orb support command rejects (reason paragraph)",
    theme: "light",
    experimental: true,
    sections: "all",
    rejectCommands: ["get_orb_support"],
    reach:
      "get_orb_support invoke rejects (init script wraps __TAURI_INTERNALS__.invoke)",
  },
  {
    id: "reply-style-model",
    marker: {
      description: "reply style model note",
      key: "settings.advanced.tts.composer.latencyModel",
    },
    title: "reply style mode = model",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { tts_composer_mode: "model" },
    reach: "tts_composer_mode = model",
  },
  {
    id: "reply-style-command",
    marker: {
      description: "reply style command acknowledgement warning",
      key: "settings.advanced.tts.composer.ackWarning",
    },
    title:
      "reply style mode = command, path set, ack not given, send utterance on",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: {
      tts_composer_mode: "command",
      tts_composer_command_path: "/opt/audit/composer",
      tts_composer_command_ack: false,
      tts_composer_send_user_utterance: true,
    },
    reach:
      "tts_composer_mode = command with a non-empty command path; G4 measured here",
  },
  {
    id: "reply-style-command-acknowledged",
    marker: {
      description: "reply style command acknowledgement warning",
      key: "settings.advanced.tts.composer.ackWarning",
    },
    title: "reply style mode = command, acknowledged",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: {
      tts_composer_mode: "command",
      tts_composer_command_path: "/opt/audit/composer",
      tts_composer_command_ack: true,
      tts_composer_send_user_utterance: true,
    },
    reach:
      "tts_composer_mode = command, path set, tts_composer_command_ack = true; G4 measured here",
  },
  {
    id: "agent-bridge-enabled",
    marker: {
      description: "agent bridge advanced section",
      key: "settings.advanced.agentBridge.advancedLabel",
    },
    title: "agent bridge enabled",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { agent_bridge_enabled: true },
    reach: "agent_bridge_enabled = true",
  },
  {
    id: "persona-editor-open",
    marker: {
      description: "persona editor Save button",
      key: "converse.voice.persona.save",
    },
    title: "persona editor opened (new persona)",
    theme: "light",
    experimental: true,
    sections: "all",
    beforeMeasure: async (page, sectionId) => {
      if (sectionId !== "general") return;
      await page
        .getByRole("button", { name: label("converse.voice.persona.new") })
        .first()
        .click({ timeout: 8000 });
    },
    reach:
      "no custom personas; the New persona button is clicked on General so the editor renders",
  },
  {
    id: "persona-limit-reached",
    marker: {
      description: "persona limit note",
      key: "converse.voice.persona.error.limit_reached",
    },
    title: "custom persona limit reached (New button replaced by limit note)",
    theme: "light",
    experimental: true,
    sections: "all",
    commands: customPersonas(12),
    reach:
      "list_personas returns 12 custom personas (PERSONA_LIMITS.maxCustom = 12); the limit paragraph replaces the New button",
  },
  {
    id: "natural-tts-failed",
    marker: {
      description: "natural voice failure detail",
      key: "settings.advanced.tts.natural.failure.load",
    },
    title: "natural TTS engine selected, status failed",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { tts_engine: "natural" },
    commands: {
      natural_voice_status: {
        supported: true,
        model: "ready",
        downloaded: 0,
        total: 0,
        health: "failed",
        failure: "load",
      },
    },
    reach:
      "tts_engine = natural and natural_voice_status health = failed (failure = load)",
  },
  {
    id: "vault-no-path",
    marker: {
      description: "vault RAG needs a vault path note",
      key: "settings.experimental.vaultRag.needsVaultPath",
    },
    title: "vault RAG on, no vault path (cannot-enable note)",
    theme: "light",
    experimental: true,
    sections: "all",
    settings: { vault_path: "", vault_rag_enabled: true },
    reach: "vault_path empty and vault_rag_enabled = true",
  },
];

const TRANSLATION = JSON.parse(
  readFileSync(
    new URL("../src/i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

function textAt(root: Record<string, unknown>, dottedKey: string): string {
  let node: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (typeof node !== "object" || node === null || !(part in node)) {
      throw new Error(`missing translation key ${dottedKey}`);
    }
    node = (node as Record<string, unknown>)[part];
  }
  if (typeof node !== "string") {
    throw new Error(`translation key ${dottedKey} is not a string`);
  }
  return node;
}

const label = (key: string): string => textAt(TRANSLATION, key);

/** Sidebar sections from src/components/Sidebar.tsx (SECTIONS_CONFIG), labels from en. */
function sidebarSections(): SectionRef[] {
  const src = readFileSync(
    new URL("../src/components/Sidebar.tsx", import.meta.url),
    "utf8",
  );
  const en = TRANSLATION as { sidebar?: Record<string, string> };
  const re = /(\w+):\s*\{\s*labelKey:\s*"sidebar\.(\w+)"/g;
  const sections: SectionRef[] = [];
  for (const m of src.matchAll(re)) {
    const text = en.sidebar?.[m[2]];
    if (typeof text !== "string") {
      throw new Error(`no English label for sidebar.${m[2]}`);
    }
    sections.push({ id: m[1], label: text });
  }
  if (sections.length === 0) {
    throw new Error(
      "no sidebar sections parsed from src/components/Sidebar.tsx",
    );
  }
  return sections;
}

const SECTIONS = sidebarSections();

const navItem = (page: Page, text: string): Locator =>
  page.locator("p", {
    hasText: new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
  });

const sectionLocator = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);

/**
 * Browser-side guard measurement. Passed to page.evaluate, so it must be self-contained:
 * every helper is defined inside this body and nothing outside is referenced.
 */
function measureGuard(arg: { allow: AllowEntry[] }): BrowserMeasure {
  const ROW = "[data-setting-title]";
  const CONTROL =
    "button,input,select,textarea,[role=switch],[role=combobox],[role=listbox],[role=slider],[role=progressbar],[role=tab],[role=dialog],[role=tooltip]";
  const TEXT = "p,h2,h3,h4,li";
  const LO = 12;
  const HI = 14.5;
  const r1 = (n: number): number => Math.round(n * 10) / 10;

  const offenders: BrowserOffender[] = [];
  const allowHits: Record<string, number> = {};
  const checked = { G1: 0, G2: 0, G3: 0 };
  let allowedExempt = 0;

  const isVisible = (el: Element): boolean => {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    const cs = getComputedStyle(el);
    return cs.display !== "none" && cs.visibility !== "hidden";
  };

  const isTransparent = (color: string): boolean => {
    const m = /rgba?\(([^)]*)\)/.exec(color);
    if (!m) return color === "" || color === "transparent";
    const parts = m[1].split(/[\s,/]+/).filter((p) => p !== "");
    return parts.length === 4 && parseFloat(parts[3]) === 0;
  };

  // Border on at least three sides. A single divider line (divide-y) is not a box.
  const borderSides = (cs: CSSStyleDeclaration): number =>
    ["top", "right", "bottom", "left"].filter(
      (side) =>
        parseFloat(cs.getPropertyValue(`border-${side}-width`)) > 0 &&
        cs.getPropertyValue(`border-${side}-style`) !== "none",
    ).length;

  // "Boxed": a non-transparent background or a real border box.
  const isBoxed = (el: Element): boolean => {
    const cs = getComputedStyle(el);
    return !isTransparent(cs.backgroundColor) || borderSides(cs) >= 3;
  };

  // "Panel": boxed AND rounded corners (rule G3).
  const isPanel = (el: Element): boolean => {
    if (!isBoxed(el)) return false;
    const cs = getComputedStyle(el);
    return ["top-left", "top-right", "bottom-right", "bottom-left"].some(
      (corner) =>
        parseFloat(cs.getPropertyValue(`border-${corner}-radius`)) > 0,
    );
  };

  // First non-blank text node below el. With `own`, only text whose nearest TEXT ancestor is el.
  const firstText = (el: Element, own: boolean): Text | null => {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const t = n as Text;
      if (!t.textContent || t.textContent.trim() === "") continue;
      const parent = t.parentElement;
      if (!parent || parent.closest(".sr-only")) continue;
      if (own && parent.closest(TEXT) !== el) continue;
      return t;
    }
    return null;
  };

  const textLeft = (t: Text): number => {
    const text = t.textContent ?? "";
    const at = Math.max(0, text.search(/\S/));
    const range = document.createRange();
    range.setStart(t, at);
    range.setEnd(t, at + 1);
    return range.getBoundingClientRect().left;
  };

  const snippet = (t: Text | null): string =>
    (t?.textContent ?? "").replace(/\s+/g, " ").trim().slice(0, 40);

  const shortSel = (el: Element): string => {
    const cls = (el.getAttribute("class") ?? "")
      .split(/\s+/)
      .filter((c) => c !== "" && !c.includes(":") && !c.includes("["))
      .slice(0, 3)
      .join(".");
    const title = el.getAttribute("data-setting-title");
    const attr = title ? `[data-setting-title="${title.slice(0, 40)}"]` : "";
    return `${el.tagName.toLowerCase()}${cls ? `.${cls}` : ""}${attr}`.slice(
      0,
      160,
    );
  };

  const groups = Array.from(
    document.querySelectorAll<HTMLElement>("[data-group-id]"),
  ).filter(isVisible);
  let groupsSeen = 0;

  for (const group of groups) {
    const groupId = group.getAttribute("data-group-id") ?? "?";
    const header = group.querySelector(":scope > button");
    const bodyId = header?.getAttribute("aria-controls");
    const body = bodyId ? document.getElementById(bodyId) : null;
    if (!body || !isVisible(body)) {
      offenders.push({
        rule: "MEASURE",
        group: groupId,
        selector: shortSel(group),
        text: "",
        measured: "group body not open or not found",
      });
      continue;
    }
    groupsSeen++;
    const gRect = group.getBoundingClientRect();
    const gl = gRect.left;
    const gr = gRect.right;
    const owned = (el: Element): boolean =>
      el.closest("[data-group-id]") === group && body.contains(el);

    // A row that lives inside a rounded, boxed panel (for example a voice-engine card) is
    // indented by that panel on purpose; the panel itself is checked by G3. This is a
    // structural exemption, never a distance-based one: a bare padded wrapper has no
    // border or background, so it is not a panel and its rows are still measured.
    const insidePanel = (el: Element): boolean => {
      for (let a = el.parentElement; a && a !== body; a = a.parentElement) {
        if (isPanel(a)) return true;
      }
      return false;
    };

    // G1: row labels.
    for (const row of Array.from(body.querySelectorAll(ROW))) {
      if (!owned(row) || !isVisible(row)) continue;
      if (insidePanel(row)) continue;
      checked.G1++;
      const t = firstText(row, false);
      if (!t) {
        offenders.push({
          rule: "G1",
          group: groupId,
          selector: shortSel(row),
          text: "",
          measured: "no text node",
        });
        continue;
      }
      const inset = r1(textLeft(t) - gl);
      if (inset < LO || inset > HI) {
        offenders.push({
          rule: "G1",
          group: groupId,
          selector: shortSel(row),
          text: snippet(t),
          measured: `left=${inset}px`,
        });
      }
    }

    // G2: text blocks.
    for (const el of Array.from(body.querySelectorAll(TEXT))) {
      if (!owned(el) || !isVisible(el)) continue;
      const t = firstText(el, true);
      if (!t) continue;
      const row = el.closest(ROW);
      if (row && body.contains(row)) continue; // (a) inside a row
      const control = el.closest(CONTROL);
      if (control && body.contains(control)) continue; // (b) inside a control
      if (header && header.contains(el)) continue; // (c) group header
      let boxed = false; // (d) boxed ancestor strictly between el and body
      for (let a = el.parentElement; a && a !== body; a = a.parentElement) {
        if (isBoxed(a)) {
          boxed = true;
          break;
        }
      }
      if (boxed) continue;
      const allow = arg.allow.find((entry) => el.matches(entry.selector));
      if (allow) {
        allowedExempt++;
        allowHits[allow.selector] = (allowHits[allow.selector] ?? 0) + 1;
        continue;
      }
      checked.G2++;
      const inset = r1(textLeft(t) - gl);
      if (inset < LO || inset > HI) {
        offenders.push({
          rule: "G2",
          group: groupId,
          selector: shortSel(el),
          text: snippet(t),
          measured: `left=${inset}px`,
        });
      }
    }

    // G3: outermost rounded panels directly in the group.
    for (const el of Array.from(body.querySelectorAll("*"))) {
      // A control (button, input) is not a panel, even when it has a border and radius.
      if (!owned(el) || !isVisible(el) || el.matches(CONTROL) || !isPanel(el))
        continue;
      let nested = false;
      for (let a = el.parentElement; a && a !== body; a = a.parentElement) {
        if (isPanel(a) || a.matches(CONTROL) || a.matches(ROW)) {
          nested = true;
          break;
        }
      }
      if (nested) continue;
      checked.G3++;
      const cr = el.getBoundingClientRect();
      const insetL = r1(cr.left - gl);
      const insetR = r1(gr - cr.right);
      const bad = (v: number): boolean => v < LO || v > HI;
      if (bad(insetL) || bad(insetR)) {
        offenders.push({
          rule: "G3",
          group: groupId,
          selector: shortSel(el),
          text: snippet(firstText(el, false)),
          measured: `boxL=${insetL}px boxR=${insetR}px`,
        });
      }
    }
  }

  return {
    checked,
    offenders,
    allowedExempt,
    allowHits,
    groupsSeen,
  };
}

/** G4 helper: left of the first text node that matches, in page coordinates. */
function findTextLeft(arg: {
  needle: string;
  prefix: boolean;
  role: string | null;
}): number | null {
  const norm = (s: string): string => s.replace(/\s+/g, " ").trim();
  const candidates = Array.from(
    document.querySelectorAll(arg.role ? `[role="${arg.role}"]` : "body *"),
  );
  for (const el of candidates) {
    const rect = el.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) continue;
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    let first: Text | null = null;
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const t = n as Text;
      if (t.textContent && t.textContent.trim() !== "") {
        first = t;
        break;
      }
    }
    if (!first || !first.textContent) continue;
    const matches = arg.prefix
      ? norm(el.textContent ?? "").startsWith(arg.needle)
      : norm(first.textContent) === arg.needle;
    if (!matches) continue;
    const at = Math.max(0, first.textContent.search(/\S/));
    const range = document.createRange();
    range.setStart(first, at);
    range.setEnd(first, at + 1);
    return range.getBoundingClientRect().left;
  }
  return null;
}

/** Test-side: makes selected invokes reject, so the frontend's failure path renders. */
function rejectCommandsInPage(names: string[]): void {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: {
        invoke: (
          cmd: string,
          args?: unknown,
          options?: unknown,
        ) => Promise<unknown>;
      };
    }
  ).__TAURI_INTERNALS__;
  const original = internals.invoke;
  internals.invoke = (cmd, args, options) =>
    names.includes(cmd)
      ? Promise.reject(new Error(`audit: ${cmd} rejected`))
      : original(cmd, args, options);
}

/*
 * Results are persisted per state. A failing test makes Playwright restart the worker, which
 * resets module-level state, so the report is rebuilt from these files on every write.
 */
interface StateResult {
  state: string;
  records: RunRecord[];
  offenders: Offender[];
  navFailures: Array<{ state: string; page: string; reason: string }>;
  allowHits: Record<string, number>;
  marker: { description: string; seenOn: string | null } | null;
}

const STATE_DIR = `${OUT_DIR}/inset-states`;

function persistState(result: StateResult): void {
  mkdirSync(STATE_DIR, { recursive: true });
  writeFileSync(
    `${STATE_DIR}/${result.state}.json`,
    JSON.stringify(result, null, 2),
  );
}

function loadStates(): Array<StateResult | null> {
  return STATES.map((s) => {
    const file = `${STATE_DIR}/${s.id}.json`;
    return existsSync(file)
      ? (JSON.parse(readFileSync(file, "utf8")) as StateResult)
      : null;
  });
}

async function settle(page: Page): Promise<void> {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
}

async function openSection(
  page: Page,
  section: SectionRef,
): Promise<string | null> {
  try {
    await navItem(page, section.label).first().click({ timeout: 5000 });
    await expect(sectionLocator(page, section.id)).toBeVisible({
      timeout: 8000,
    });
  } catch (error) {
    return `section did not open: ${(error as Error).message.split("\n")[0]}`;
  }
  await settle(page);
  return null;
}

/** Opens every collapsed group (two passes, so nested collapsed groups open too). */
async function openAllGroups(page: Page): Promise<void> {
  for (let pass = 0; pass < 2; pass++) {
    const opened = await page.evaluate(() => {
      const closed = Array.from(
        document.querySelectorAll<HTMLButtonElement>(
          '[data-group-id] > button[aria-expanded="false"]',
        ),
      );
      for (const button of closed) button.click();
      return closed.length;
    });
    if (opened === 0) break;
    await settle(page);
  }
}

function writeReport(): void {
  mkdirSync(OUT_DIR, { recursive: true });
  const loaded = loadStates().filter((s): s is StateResult => s !== null);
  const records = loaded.flatMap((s) => s.records);
  const offenders = loaded.flatMap((s) => s.offenders);
  const navFailures = loaded.flatMap((s) => s.navFailures);
  const allowHits: Record<string, number> = {};
  for (const s of loaded) {
    for (const [selector, hits] of Object.entries(s.allowHits)) {
      allowHits[selector] = (allowHits[selector] ?? 0) + hits;
    }
  }
  const pages = Array.from(new Set(records.map((r) => r.page)));
  const rules = Object.keys(RULE_TEXT) as Rule[];
  const byRulePage: Record<string, Record<string, number>> = {};
  const checkedByRulePage: Record<string, Record<string, number>> = {};
  for (const rule of rules) {
    byRulePage[rule] = {};
    checkedByRulePage[rule] = {};
    for (const p of pages) {
      byRulePage[rule][p] = offenders.filter(
        (o) => o.rule === rule && o.page === p,
      ).length;
      checkedByRulePage[rule][p] = records
        .filter((r) => r.page === p)
        .reduce((sum, r) => sum + r.checked[rule], 0);
    }
  }
  const distinct = new Set(
    offenders.map(
      (o) => `${o.rule}|${o.page}|${o.group}|${o.selector}|${o.text}`,
    ),
  );
  const states = STATES.map((s) => ({
    id: s.id,
    title: s.title,
    theme: s.theme,
    experimental: s.experimental,
    sections: s.sections,
    settings: s.settings ?? {},
    commands: Object.keys(s.commands ?? {}),
    rejectCommands: s.rejectCommands ?? [],
    reach: s.reach,
    offenders: offenders.filter((o) => o.state === s.id).length,
    ran: loaded.some((l) => l.state === s.id),
    marker: loaded.find((l) => l.state === s.id)?.marker ?? null,
  }));
  writeFileSync(
    REPORT_PATH,
    JSON.stringify(
      {
        generatedAt: new Date().toISOString(),
        design: "settings text inset guard, rules G1-G4",
        viewport: VIEWPORT.label,
        columnRange: [COLUMN_MIN, COLUMN_MAX],
        rules: RULE_TEXT,
        interpretation: [
          "G2 panel exemption: an ancestor between the element and the group body that has a background (alpha > 0) or a border on at least 3 sides. A single divider line (divide-y) is not a panel.",
          "G2 text: first non-blank text node whose nearest text-block ancestor is the element (p/h2/h3/h4/li).",
          "G3 nesting: an ancestor panel, control or [data-setting-title] row makes the panel nested (not outermost), so it is not checked.",
          "G4 is measured only in states with tts_composer_mode = command, on the General page.",
        ],
        allowList: ALLOW,
        allowHits,
        states,
        notRenderable: [] as string[],
        notRenderableNote:
          "Every state in STATES was rendered through the mock. A state with ran=false was not executed in this run.",
        summary: {
          offenderRecords: offenders.length,
          distinctOffenders: distinct.size,
          offendersByRulePage: byRulePage,
          checkedByRulePage,
        },
        navFailures,
        records,
        offenders,
      },
      null,
      2,
    ),
  );
}

/** Visible marker of a conditional state. The key's text is cut before any {{param}}. */
async function markerVisible(
  page: Page,
  marker: { key?: string; selector?: string },
): Promise<boolean> {
  const locator = marker.selector
    ? page.locator(marker.selector)
    : page.getByText(
        label(marker.key ?? "")
          .split("{{")[0]
          .trim(),
      );
  return (await locator.count()) > 0 && (await locator.first().isVisible());
}

async function runState(
  page: Page,
  state: StateDef,
  acc: StateResult,
): Promise<void> {
  acc.marker = state.marker
    ? { description: state.marker.description, seenOn: null }
    : null;
  await page.setViewportSize({
    width: VIEWPORT.width,
    height: VIEWPORT.height,
  });
  await page.emulateMedia({ colorScheme: state.theme });
  await installTauriMock(page, {
    settings: {
      experimental_enabled: state.experimental,
      post_process_enabled: state.experimental,
      debug_mode: state.experimental,
      ...BASE_SETTINGS,
      ...state.settings,
    },
    commands: { ...BASE_COMMANDS, ...state.commands },
  });
  if (state.rejectCommands && state.rejectCommands.length > 0) {
    await page.addInitScript(rejectCommandsInPage, state.rejectCommands);
  }
  await page.goto("/");
  await expect(navItem(page, "General").first()).toBeVisible({
    timeout: 30000,
  });

  const sections =
    state.sections === "all"
      ? SECTIONS
      : SECTIONS.filter((s) => (state.sections as string[]).includes(s.id));
  const commandMode = state.settings?.tts_composer_mode === "command";

  for (const section of sections) {
    const reason = await openSection(page, section);
    if (reason) {
      acc.navFailures.push({ state: state.id, page: section.id, reason });
      acc.records.push({
        state: state.id,
        page: section.id,
        theme: state.theme,
        viewport: VIEWPORT.label,
        status: "NAV_FAILED",
        reason,
        groupsSeen: 0,
        checked: { G1: 0, G2: 0, G3: 0, G4: 0 },
        allowedExempt: 0,
        offenderCount: 0,
      });
      continue;
    }

    await openAllGroups(page);
    if (state.beforeMeasure) await state.beforeMeasure(page, section.id);
    if (state.marker && acc.marker && acc.marker.seenOn === null) {
      if (await markerVisible(page, state.marker)) {
        acc.marker.seenOn = section.id;
      }
    }

    const measured = await page.evaluate(measureGuard, { allow: ALLOW });
    const pageOffenders: Offender[] = measured.offenders.map((o) => ({
      state: state.id,
      rule: o.rule as Rule | "MEASURE",
      page: section.id,
      theme: state.theme,
      viewport: VIEWPORT.label,
      group: o.group,
      selector: o.selector,
      text: o.text,
      measured: o.measured,
    }));
    for (const [selector, hits] of Object.entries(measured.allowHits)) {
      acc.allowHits[selector] = (acc.allowHits[selector] ?? 0) + hits;
    }
    const checked: Record<Rule, number> = {
      G1: measured.checked.G1,
      G2: measured.checked.G2,
      G3: measured.checked.G3,
      G4: 0,
    };

    if (section.id === "general" && commandMode) {
      const ackLabel = label("settings.advanced.tts.composer.ackLabel");
      const ackWarning = label("settings.advanced.tts.composer.ackWarning");
      const modeLabel = label("settings.advanced.tts.composer.modeLabel");
      const ackX = await page.evaluate(findTextLeft, {
        needle: ackLabel,
        prefix: false,
        role: null,
      });
      const warnX = await page.evaluate(findTextLeft, {
        needle: ackWarning.slice(0, 30),
        prefix: true,
        role: "note",
      });
      const modeX = await page.evaluate(findTextLeft, {
        needle: modeLabel,
        prefix: false,
        role: null,
      });
      const g4Base = {
        state: state.id,
        rule: "G4" as const,
        page: section.id,
        theme: state.theme,
        viewport: VIEWPORT.label,
        group: "composer",
      };
      if (ackX === null || warnX === null || modeX === null) {
        pageOffenders.push({
          ...g4Base,
          selector: "composer",
          text: "",
          measured: `not found: ack=${ackX} warn=${warnX} mode=${modeX}`,
        });
      } else {
        checked.G4 = 1;
        const ack = Math.round(ackX * 10) / 10;
        const warn = Math.round(warnX * 10) / 10;
        const mode = Math.round(modeX * 10) / 10;
        if (Math.abs(ack - warn) > 1 || Math.abs(ack - mode) > 1) {
          pageOffenders.push({
            ...g4Base,
            selector: "composer ack label / warning / mode label",
            text: ackLabel.slice(0, 40),
            measured: `ack=${ack}px warn=${warn}px mode=${mode}px`,
          });
        }
      }
    }

    acc.offenders.push(...pageOffenders);
    acc.records.push({
      state: state.id,
      page: section.id,
      theme: state.theme,
      viewport: VIEWPORT.label,
      status: "measured",
      reason: null,
      groupsSeen: measured.groupsSeen,
      checked,
      allowedExempt: measured.allowedExempt,
      offenderCount: pageOffenders.length,
    });
  }

  // A conditional state whose marker never showed on any visited page did not render.
  if (state.marker && acc.marker && acc.marker.seenOn === null) {
    acc.offenders.push({
      state: state.id,
      rule: "MEASURE",
      page: "-",
      theme: state.theme,
      viewport: VIEWPORT.label,
      group: "-",
      selector: "conditional state marker",
      text: "",
      measured: `state not rendered: ${state.marker.description}`,
    });
  }
}

function formatOffenders(list: Offender[], limit: number): string {
  return list
    .slice(0, limit)
    .map(
      (o) =>
        `${o.rule} page=${o.page} group=${o.group} ${o.selector} "${o.text}" ${o.measured}`,
    )
    .join("\n");
}

test.describe("settings text inset guard G (measuring, fails on offenders)", () => {
  // Each case opens every sidebar page and every group, which takes longer than the default.
  test.describe.configure({ timeout: 180_000 });

  test.afterAll(() => {
    writeReport();
  });

  for (const state of STATES) {
    test(`${state.id}: ${state.title}`, async ({ page }) => {
      const acc: StateResult = {
        state: state.id,
        records: [],
        offenders: [],
        navFailures: [],
        allowHits: {},
        marker: null,
      };
      try {
        await runState(page, state, acc);
      } finally {
        persistState(acc);
        writeReport();
      }

      if (state.coverage) {
        for (const p of ["models", "general", "agents"]) {
          const rows = acc.records.filter(
            (r) => r.page === p && r.status === "measured",
          );
          const g1 = rows.reduce((s, r) => s + r.checked.G1, 0);
          const g2 = rows.reduce((s, r) => s + r.checked.G2, 0);
          expect(g1, `G1 coverage on ${p} must be > 0`).toBeGreaterThan(0);
          expect(g2, `G2 coverage on ${p} must be > 0`).toBeGreaterThan(0);
        }
      }

      const navText = acc.navFailures
        .map((n) => `${n.page}: ${n.reason}`)
        .join("\n");
      expect(navText, "sidebar pages that could not be opened").toBe("");

      const mine = acc.offenders;
      const summary = `${mine.length} offender(s); first ${Math.min(40, mine.length)}:\n${formatOffenders(mine, 40)}`;
      expect(mine.length, summary).toBe(0);
    });
  }
});
