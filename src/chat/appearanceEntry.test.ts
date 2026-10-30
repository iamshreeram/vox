// UI enterprise S2 (A1/A2): the conversation page seeds its appearance from the URL and follows
// `appearance` events through the shared consumer. It keeps the listen-only rule of entry.test.ts
// and no longer reads the localStorage cache (a cache must never outrank the rev guard).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const main = readFileSync(join(here, "main.tsx"), "utf8");

describe("src/chat/main.tsx appearance wiring (UI S2)", () => {
  it("seeds from the URL and listens for appearance through the consumer", () => {
    expect(main).toMatch(/from\s+["']\.\.\/lib\/appearance\/consumer["']/);
    expect(main).toMatch(/parseSeed\(window\.location\.search\)/);
    expect(main).toMatch(/listen\(\s*APPEARANCE_EVENT/);
    expect(main).toMatch(/consumer\.accept\(/);
  });

  it("imports the palette and never reads localStorage or the legacy theme events", () => {
    expect(main).toMatch(/from\s+["']\.\/chatTheme["']/);
    expect(main).not.toContain("localStorage");
    expect(main).not.toContain("THEME_STORAGE_KEY");
    expect(main).not.toContain("theme-changed");
    expect(main).not.toContain("applyTheme(");
  });

  it("keeps the opacity logic with its own rev guard", () => {
    expect(main).toContain("applyOpacityEvent");
    expect(main).toContain('"chat-opacity"');
  });

  it("keeps every chat stylesheet import explicit", () => {
    expect(main).toMatch(/import\s+["']\.\/chat\.css["']/);
  });
});
