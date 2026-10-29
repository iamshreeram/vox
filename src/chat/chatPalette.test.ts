// UI enterprise S2 (DESIGN Revision 3, A4): the two chat palettes, the accent-driven brand roles,
// the decorative face hues and their readable text variants. chat.css must read every colour from
// a variable that chatTheme.ts (or chat.css itself) defines. Vitest, node fs only.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CHAT_PALETTES,
  CHAT_VARIABLE_NAMES,
  FACE_TEXT,
  accentTextStyle,
  chatPaletteFor,
  chatPaletteVariables,
} from "./chatTheme";
import type { AvatarFace } from "./chatModel";
import { ACCENT_IDS, ACCENTS } from "../styles/accents";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];
const PALETTES = ["dark", "light"] as const;
const HEX = /^#[0-9a-f]{6}$/i;
const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "chat.css"), "utf8");

describe("chat palettes (UI S2, A4)", () => {
  it("the dark palette keeps today's exact neutrals (the default look)", () => {
    expect(CHAT_PALETTES.dark.bg.toLowerCase()).toBe("#0b0d12");
    expect(CHAT_PALETTES.dark.surface.toLowerCase()).toBe("#151922");
    expect(CHAT_PALETTES.dark.text.toLowerCase()).toBe("#e7e9ee");
    expect(CHAT_PALETTES.dark.muted.toLowerCase()).toBe("#98a1b2");
  });

  it("the light palette has its own neutrals, all hex", () => {
    for (const key of ["bg", "surface", "text", "muted"] as const) {
      expect(CHAT_PALETTES.light[key]).toMatch(HEX);
      expect(CHAT_PALETTES.light[key].toLowerCase()).not.toBe(
        CHAT_PALETTES.dark[key].toLowerCase(),
      );
    }
  });

  it("every palette and accent takes its brand roles from the accent table", () => {
    for (const palette of PALETTES) {
      for (const accent of ACCENT_IDS) {
        const full = chatPaletteFor(palette, accent);
        const roles = ACCENTS[accent][palette];
        expect(full.brand).toBe(roles.fillA);
        expect(full.brandText).toBe(roles.text);
        expect(full.onBrand).toBe(roles.onFill);
      }
    }
  });

  it("FACE_TEXT has a readable hex text colour for every face in both palettes", () => {
    for (const palette of PALETTES) {
      expect(Object.keys(FACE_TEXT[palette]).sort()).toEqual([...FACES].sort());
      for (const face of FACES) expect(FACE_TEXT[palette][face]).toMatch(HEX);
    }
  });

  it("accentTextStyle points --chat-accent-text at the palette's face text variable", () => {
    for (const face of FACES) {
      expect(accentTextStyle(face)).toEqual({
        "--chat-accent-text": `var(--chat-face-${face}-text)`,
      });
    }
    const vars = chatPaletteVariables("light", "amber");
    for (const face of FACES) {
      expect(vars[`--chat-face-${face}-text`]).toBe(FACE_TEXT.light[face]);
    }
  });

  it("chatPaletteVariables sets every palette variable as a value chat.css can read", () => {
    const vars = chatPaletteVariables("light", "blue");
    expect(vars["--chat-bg-rgb"]).toBe("247, 248, 250");
    expect(vars["--chat-color-scheme"]).toBe("light");
    expect(vars["--chat-brand"]).toBe(ACCENTS.blue.light.fillA);
    for (const name of Object.keys(vars)) {
      expect(CHAT_VARIABLE_NAMES).toContain(name);
    }
    expect(chatPaletteVariables("dark", "amber")["--chat-color-scheme"]).toBe(
      "dark",
    );
  });

  it("every --chat- variable chat.css reads is declared in chat.css or set by chatTheme", () => {
    const declared = new Set(
      [...css.matchAll(/(--chat-[a-z0-9-]+)\s*:/g)].map((m) => m[1]),
    );
    const read = [...css.matchAll(/var\((--chat-[a-z0-9-]+)/g)].map(
      (m) => m[1],
    );
    const missing = read.filter(
      (name) => !declared.has(name) && !CHAT_VARIABLE_NAMES.includes(name),
    );
    expect(missing).toEqual([]);
  });

  it("chat.css has no colour literal left: every colour is a variable", () => {
    expect(css).not.toMatch(/#[0-9a-fA-F]{3,6}\b/);
    expect(css).not.toMatch(/rgba?\(\s*\d/);
  });
});
