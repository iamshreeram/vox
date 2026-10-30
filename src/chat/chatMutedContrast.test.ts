import { describe, expect, it } from "vitest";
import { CHAT_PALETTES, type ChatPaletteName } from "./chatTheme";
import { contrastRatio } from "../styles/wcag";

// UI enterprise S4b (U15c): the composer hint, the timestamps, the character count and the disabled
// Send label are muted text. Each must reach 4.5:1 over every solid surface it can sit on, in both
// palettes. The chip floor is the palette bg/surface under translucent chips (chatOpacityContrast).
const SURFACE_KEYS = [
  "bg",
  "surface",
  "userBubble",
  "systemChip",
  "errorChip",
  "sendDisabled",
] as const;

describe("chat muted text (composer hint, timestamps, count, disabled Send)", () => {
  for (const name of Object.keys(CHAT_PALETTES) as ChatPaletteName[]) {
    it(`${name}: muted text is at least 4.5:1 on every surface`, () => {
      const palette = CHAT_PALETTES[name];
      for (const key of SURFACE_KEYS) {
        const ratio = contrastRatio(palette.muted, palette[key]);
        expect(
          ratio,
          `${name} muted on ${key} (${palette[key]})`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});
