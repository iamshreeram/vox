// UI-REFRESH SPEC (DESIGN-UI Rev 2, written by ram-ai; workers must not edit this file).
// Contract for src/chat/chatTheme.ts:
//   export const FACE_ACCENT: Record<AvatarFace, string>   // "#rrggbb" per face
//   export const CHAT_COLORS: { bg: string; surface: string; text: string; muted: string; brand: string }
//   export function contrastRatio(a: string, b: string): number   // WCAG 2.x, "#rrggbb" inputs
//   export function accentStyle(face: AvatarFace): { "--chat-accent": string }
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CHAT_COLORS,
  FACE_ACCENT,
  accentStyle,
  contrastRatio,
} from "./chatTheme";
import type { AvatarFace } from "./chatModel";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];
const HEX = /^#[0-9a-f]{6}$/i;
const here = dirname(fileURLToPath(import.meta.url));

describe("chatTheme palette", () => {
  it("has a valid hex accent for every face and nothing else", () => {
    expect(Object.keys(FACE_ACCENT).sort()).toEqual([...FACES].sort());
    for (const face of FACES) expect(FACE_ACCENT[face]).toMatch(HEX);
  });

  it("gives every face its own accent colour", () => {
    const all = FACES.map((f) => FACE_ACCENT[f].toLowerCase());
    expect(new Set(all).size).toBe(FACES.length);
  });

  it("listening is cyan-ish and speaking is gold-ish and they clearly differ", () => {
    const hue = (hex: string): number => {
      const [r, g, b] = [1, 3, 5].map(
        (i) => parseInt(hex.slice(i, i + 2), 16) / 255,
      );
      const max = Math.max(r, g, b);
      const min = Math.min(r, g, b);
      const d = max - min;
      if (d === 0) return 0;
      let h = 0;
      if (max === r) h = ((g - b) / d) % 6;
      else if (max === g) h = (b - r) / d + 2;
      else h = (r - g) / d + 4;
      return (h * 60 + 360) % 360;
    };
    const listening = hue(FACE_ACCENT.listening);
    const speaking = hue(FACE_ACCENT.speaking);
    expect(listening).toBeGreaterThan(170);
    expect(listening).toBeLessThan(210);
    expect(speaking).toBeGreaterThan(30);
    expect(speaking).toBeLessThan(55);
    expect(
      contrastRatio(FACE_ACCENT.listening, FACE_ACCENT.speaking),
    ).toBeGreaterThan(1.1);
  });

  it("accent colours are visible (>= 3:1) against the window background", () => {
    for (const face of FACES) {
      expect(
        contrastRatio(FACE_ACCENT[face], CHAT_COLORS.bg),
        `${face} accent on bg`,
      ).toBeGreaterThanOrEqual(3);
    }
  });

  it("body and muted text meet 4.5:1 on the background and on the surface", () => {
    for (const fg of [CHAT_COLORS.text, CHAT_COLORS.muted]) {
      for (const bg of [CHAT_COLORS.bg, CHAT_COLORS.surface]) {
        expect(contrastRatio(fg, bg), `${fg} on ${bg}`).toBeGreaterThanOrEqual(
          4.5,
        );
      }
    }
  });

  it("brand gold matches the Vox icon orb family (warm, hue 35-50)", () => {
    expect(CHAT_COLORS.brand).toMatch(HEX);
    const r = parseInt(CHAT_COLORS.brand.slice(1, 3), 16);
    const g = parseInt(CHAT_COLORS.brand.slice(3, 5), 16);
    const b = parseInt(CHAT_COLORS.brand.slice(5, 7), 16);
    expect(r).toBeGreaterThan(200);
    expect(g).toBeGreaterThan(140);
    expect(b).toBeLessThan(80);
  });

  it("contrastRatio is the WCAG ratio (black on white is 21, same colour is 1)", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 1);
    expect(contrastRatio("#336699", "#336699")).toBeCloseTo(1, 5);
    expect(contrastRatio("#ffffff", "#000000")).toBeCloseTo(21, 1);
  });

  it("accentStyle sets only the --chat-accent custom property", () => {
    for (const face of FACES) {
      expect(accentStyle(face)).toEqual({ "--chat-accent": FACE_ACCENT[face] });
    }
  });
});

describe("chat.css keeps a single source of truth for accents (C6)", () => {
  const css = readFileSync(join(here, "chat.css"), "utf8");
  it("never declares --chat-accent itself (it is set inline from chatTheme)", () => {
    expect(css).not.toMatch(/--chat-accent\s*:/);
  });
  it("does not repeat any per-face accent hex", () => {
    for (const face of FACES) {
      expect(css.toLowerCase()).not.toContain(FACE_ACCENT[face].toLowerCase());
    }
  });
  it("uses the accent variable", () => {
    expect(css).toContain("var(--chat-accent");
  });
  it("honours reduced motion", () => {
    expect(css).toMatch(/prefers-reduced-motion:\s*reduce/);
  });
});
