// Static checks: node fs scan of src/chat production sources (not tests).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const CHAT_DIR = fileURLToPath(new URL(".", import.meta.url));
// DESIGN-TYPE Rev 1: the user asked to type, so exactly one command is added.
// D1b (orb-click): Escape hides the window, so chatHideWindow is the fourth allowed command.
const ALLOWED_COMMANDS = new Set([
  "getChatSnapshot",
  "clearChatLog",
  "sendChatMessage",
  "chatHideWindow",
]);

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      out.push(...sourceFiles(full));
      continue;
    }
    if (!/\.(ts|tsx)$/.test(name)) continue;
    if (/\.test\.(ts|tsx)$/.test(name)) continue;
    out.push(full);
  }
  return out;
}

const FILES = sourceFiles(CHAT_DIR);
const TEXT = FILES.map((f) => ({
  name: relative(CHAT_DIR, f),
  src: readFileSync(f, "utf8"),
}));

function offenders(pattern: RegExp): string[] {
  return TEXT.filter((f) => pattern.test(f.src)).map((f) => f.name);
}

describe("src/chat static safety scan", () => {
  it("finds the production chat sources it is supposed to scan", () => {
    const names = TEXT.map((f) => f.name).sort();
    expect(names).toEqual(
      expect.arrayContaining([
        "ChatWindow.tsx",
        "VoxAvatar.tsx",
        "chatModel.ts",
      ]),
    );
  });

  it("has no dangerouslySetInnerHTML", () => {
    expect(offenders(/dangerouslySetInnerHTML/)).toEqual([]);
  });

  it("has no innerHTML or outerHTML writes", () => {
    expect(offenders(/\binnerHTML\b|\bouterHTML\b/)).toEqual([]);
  });

  it("has no insertAdjacentHTML", () => {
    expect(offenders(/insertAdjacentHTML/)).toEqual([]);
  });

  it("has no eval( and no new Function", () => {
    expect(offenders(/\beval\s*\(/)).toEqual([]);
    expect(offenders(/new\s+Function\b/)).toEqual([]);
  });

  it("imports no @tauri-apps/plugin-* modules (static or dynamic)", () => {
    expect(offenders(/@tauri-apps\/plugin-/)).toEqual([]);
  });

  it("makes no network calls (fetch, XMLHttpRequest, WebSocket)", () => {
    expect(offenders(/\bfetch\s*\(/)).toEqual([]);
    expect(offenders(/\bXMLHttpRequest\b/)).toEqual([]);
    expect(offenders(/\bWebSocket\b/)).toEqual([]);
  });

  it("writes nothing to localStorage or sessionStorage", () => {
    expect(offenders(/(localStorage|sessionStorage)\s*\.\s*setItem/)).toEqual(
      [],
    );
    expect(offenders(/(localStorage|sessionStorage)\s*\[[^\]]*\]\s*=/)).toEqual(
      [],
    );
  });

  it("builds no href from an expression (only literal fragment hrefs are allowed)", () => {
    expect(offenders(/\bhref\s*=\s*\{/)).toEqual([]);
    expect(offenders(/\bhref\s*:/)).toEqual([]);
  });

  it("calls only the four allowed backend commands through bindings", () => {
    const called = new Set<string>();
    for (const file of TEXT) {
      for (const match of file.src.matchAll(/\bcommands\.(\w+)\s*\(/g)) {
        called.add(match[1] ?? "");
      }
    }
    for (const name of called) {
      expect(ALLOWED_COMMANDS.has(name)).toBe(true);
    }
  });
});
