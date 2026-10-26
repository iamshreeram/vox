// Source-contract tests for the chat window entry page, its Vite input, and the i18n keys the
// ChatWindow spec uses. Node fs only: no DOM, no network. Written first (DESIGN-CHAT Rev 6,
// A3/A8/A14 and section 4). Workers must not edit this file.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const REPO = fileURLToPath(new URL("../..", import.meta.url));
const CHAT_DIR = join(REPO, "src", "chat");
const LOCALES_DIR = join(REPO, "src", "i18n", "locales");
const LOCALE_COUNT = 26;

function read(rel: string): string {
  return readFileSync(join(REPO, rel), "utf8");
}

/** Walks a dotted key path in parsed JSON; returns undefined when any segment is missing. */
function getPath(root: unknown, dotted: string): unknown {
  let node: unknown = root;
  for (const segment of dotted.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[segment];
  }
  return node;
}

/** Chat keys every locale must carry (en is the reference; `chat.system.end.*` added below). */
const CHAT_KEYS = [
  "chat.status.listening",
  "chat.status.thinking",
  "chat.status.speaking",
  "chat.status.acting",
  "chat.status.idle",
  "chat.status.ended",
  "chat.empty",
  "chat.clear",
  "chat.clearFailed",
  "chat.jumpToLatest",
  "chat.recordingOff",
  "chat.system.started",
  "chat.system.dropped",
  "chat.system.unknown",
];

const SETTINGS_KEYS = [
  "settings.conversation.chatLog.title",
  "settings.conversation.chatLog.description",
  "settings.conversation.openChatWindow",
];

/** Session end labels, parsed from converse_runtime.rs `end_label` (the source of truth). */
function endLabels(): string[] {
  const src = read("src-tauri/src/managers/converse_runtime.rs");
  const start = src.indexOf("pub fn end_label");
  expect(start, "end_label must exist in converse_runtime.rs").toBeGreaterThan(
    -1,
  );
  const body = src.slice(start, src.indexOf("\n}", start));
  const labels = [...body.matchAll(/=>\s*"([a-z_]+)"/g)].map((m) => m[1]);
  expect(labels.length, "end_label must map 7 SessionEnd variants").toBe(7);
  return labels;
}

function allRequiredKeys(): string[] {
  return [
    ...CHAT_KEYS,
    ...endLabels().map((label) => `chat.system.end.${label}`),
    "tray.chat",
    ...SETTINGS_KEYS,
  ];
}

function localeDirs(): string[] {
  return readdirSync(LOCALES_DIR).filter((name) =>
    statSync(join(LOCALES_DIR, name)).isDirectory(),
  );
}

describe("vite.config.ts input", () => {
  it("has a chat rollup input pointing at src/chat/index.html", () => {
    const config = read("vite.config.ts");
    expect(config).toMatch(/\bchat\s*:[^\n]*src\/chat\/index\.html/);
  });

  it("keeps the existing main and overlay inputs", () => {
    const config = read("vite.config.ts");
    expect(config).toMatch(/\bmain\s*:/);
    expect(config).toMatch(/\boverlay\s*:/);
  });
});

describe("src/chat/index.html", () => {
  const html = () => read("src/chat/index.html");

  it("exists", () => {
    expect(existsSync(join(CHAT_DIR, "index.html"))).toBe(true);
  });

  it("loads the module script /src/chat/main.tsx", () => {
    const tags = [...html().matchAll(/<script\b[^>]*>/gi)].map((m) => m[0]);
    const entry = tags.find((tag) =>
      /\bsrc\s*=\s*["']\/src\/chat\/main\.tsx["']/.test(tag),
    );
    expect(
      entry,
      'a <script src="/src/chat/main.tsx"> tag must exist',
    ).toBeDefined();
    expect(entry).toMatch(/\btype\s*=\s*["']module["']/);
  });

  it("has no inline script and no inline event handler attributes", () => {
    const page = html();
    const tags = [...page.matchAll(/<script\b[^>]*>/gi)].map((m) => m[0]);
    for (const tag of tags) {
      expect(tag, `script tag without src: ${tag}`).toMatch(/\bsrc\s*=/);
    }
    expect(page).not.toMatch(/<script\b[^>]*>[^<\s][\s\S]*?<\/script>/i);
    expect(page).not.toMatch(/\son[a-z]+\s*=/i);
  });

  it("references no external http(s) URL and no protocol-relative resource", () => {
    const page = html();
    expect(page).not.toMatch(/https?:\/\//i);
    expect(page).not.toMatch(/(?:src|href)\s*=\s*["']\s*\/\//i);
  });
});

describe("src/chat/main.tsx", () => {
  const main = () => read("src/chat/main.tsx");

  it("exists", () => {
    expect(existsSync(join(CHAT_DIR, "main.tsx"))).toBe(true);
  });

  it("imports the chat-local i18n bootstrap (not @/i18n) and renders ChatWindow", () => {
    const src = main();
    expect(src).toMatch(
      /import\s+["']\.\/i18n(?:\.ts)?["']|from\s+["']\.\/i18n(?:\.ts)?["']/,
    );
    expect(src).not.toMatch(/["']@\/i18n["']/);
    expect(src).toMatch(/from\s+["']\.\/ChatWindow(?:\.tsx)?["']/);
    expect(src).toMatch(/<ChatWindow\b/);
  });

  it("does not import or call settings or any command outside ChatWindow (A8)", () => {
    const src = main();
    for (const banned of [
      "getAppSettings",
      "get_app_settings",
      "syncAppearanceFromSettings",
      "@/bindings",
      "invoke(",
      "commands.",
      "@tauri-apps/plugin-",
    ]) {
      expect(src, `main.tsx must not use ${banned}`).not.toContain(banned);
    }
  });
});

describe("i18n keys used by the chat window", () => {
  const required = allRequiredKeys();

  it("parses seven session end labels from converse_runtime.rs", () => {
    expect(endLabels().sort()).toEqual([
      "audio_unavailable",
      "cancelled",
      "idle",
      "not_configured",
      "speech_stuck",
      "turn_limit",
      "user_ended",
    ]);
  });

  it("finds exactly the 26 locale directories", () => {
    expect(localeDirs()).toHaveLength(LOCALE_COUNT);
  });

  it("has every required key in the English reference locale", () => {
    const en = JSON.parse(
      read("src/i18n/locales/en/translation.json"),
    ) as unknown;
    const missing = required.filter((key) => {
      const value = getPath(en, key);
      return typeof value !== "string" || value.trim() === "";
    });
    expect(missing, "missing or empty keys in en").toEqual([]);
  });

  it("has every required key, non-empty, in ALL 26 locale files", () => {
    const failures: string[] = [];
    for (const locale of localeDirs()) {
      const file = join(LOCALES_DIR, locale, "translation.json");
      const json = JSON.parse(readFileSync(file, "utf8")) as unknown;
      for (const key of required) {
        const value = getPath(json, key);
        if (typeof value !== "string" || value.trim() === "") {
          failures.push(`${locale}: ${key}`);
        }
      }
    }
    expect(failures, `missing chat keys (${failures.length})`).toEqual([]);
  });
});
