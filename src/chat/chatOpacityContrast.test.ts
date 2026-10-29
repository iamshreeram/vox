// Track T spec (DESIGN Revision 3, A-T5): table-driven contrast with the production palette and
// real alpha blending. A translucent text chip is its colour at alpha max(--chat-alpha, 0.9);
// opaque controls (the brand buttons, the disabled Send) use alpha 1. The window content behind a
// chip is white or black, the worst cases. Every text/chip pair must reach WCAG AA 4.5:1 for normal
// text. Each chip colour below is the value chat.css uses (see the CSS checks at the end).
// Workers must not edit this file.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CHAT_COLORS,
  CHAT_PALETTES,
  FACE_ACCENT,
  FACE_TEXT,
  chatPaletteFor,
  contrastRatio,
  type ChatPaletteName,
} from "./chatTheme";
import type { AvatarFace } from "./chatModel";
import { ACCENT_IDS, type AccentId } from "../styles/accents";

const here = dirname(fileURLToPath(import.meta.url));

/** The chip floor: translucent text chips are never more transparent than this. */
const CHIP_ALPHA_FLOOR = 0.9;
/** chat.css colours that are not in CHAT_COLORS (checked against chat.css below). */
const ERROR_TEXT = "#ff9e94";
const ERROR_CHIP = "#1f1417";
const USER_BUBBLE_BG = "#141d38";
const USER_TEXT = "#eef1ff";
const USER_SENDER = "#c9d3f5";
const SYSTEM_CHIP = "#11151d";
const DISABLED_SEND = "#2a3140";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];

/** The colour of `fg` at `alpha` over an opaque `backdrop`, as #rrggbb. */
function blend(fg: string, alpha: number, backdrop: string): string {
  const channel = (hex: string, i: number): number =>
    parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
  const out = [0, 1, 2].map((i) =>
    Math.round(channel(fg, i) * alpha + channel(backdrop, i) * (1 - alpha)),
  );
  return (
    "#" + out.map((v) => v.toString(16).padStart(2, "0")).join("")
  ).toLowerCase();
}

const BACKDROPS = { white: "#ffffff", black: "#000000" } as const;

interface Pair {
  name: string;
  text: string;
  chip: string;
  /** 0.9 for translucent chips (default); 1 for opaque controls. */
  alpha?: number;
}

const PAIRS: Pair[] = [
  {
    name: "body text on surface chip",
    text: CHAT_COLORS.text,
    chip: CHAT_COLORS.surface,
  },
  {
    name: "muted text on surface chip",
    text: CHAT_COLORS.muted,
    chip: CHAT_COLORS.surface,
  },
  {
    name: "placeholder on input chip",
    text: CHAT_COLORS.muted,
    chip: CHAT_COLORS.surface,
  },
  {
    name: "timestamp on assistant chip",
    text: CHAT_COLORS.muted,
    chip: CHAT_COLORS.surface,
  },
  { name: "error text on error chip", text: ERROR_TEXT, chip: ERROR_CHIP },
  {
    name: "brand sender on assistant chip",
    text: CHAT_COLORS.brand,
    chip: CHAT_COLORS.surface,
  },
  {
    name: "user text on user bubble chip",
    text: USER_TEXT,
    chip: USER_BUBBLE_BG,
  },
  {
    name: "user sender on user bubble chip",
    text: USER_SENDER,
    chip: USER_BUBBLE_BG,
  },
  {
    name: "timestamp on user bubble chip",
    text: CHAT_COLORS.muted,
    chip: USER_BUBBLE_BG,
  },
  {
    name: "muted text on user bubble chip",
    text: CHAT_COLORS.muted,
    chip: USER_BUBBLE_BG,
  },
  {
    name: "body text on background chip",
    text: CHAT_COLORS.text,
    chip: CHAT_COLORS.bg,
  },
  {
    name: "muted text on background chip (footer, count)",
    text: CHAT_COLORS.muted,
    chip: CHAT_COLORS.bg,
  },
  {
    name: "muted text on system chip",
    text: CHAT_COLORS.muted,
    chip: SYSTEM_CHIP,
  },
  {
    name: "jump label on brand (opaque)",
    text: CHAT_COLORS.bg,
    chip: CHAT_COLORS.brand,
    alpha: 1,
  },
  {
    name: "send label on brand (opaque)",
    text: CHAT_COLORS.bg,
    chip: CHAT_COLORS.brand,
    alpha: 1,
  },
  {
    name: "muted text on disabled send (opaque)",
    text: CHAT_COLORS.muted,
    chip: DISABLED_SEND,
    alpha: 1,
  },
  ...FACES.map((face) => ({
    name: `${face} accent text on surface chip`,
    text: FACE_ACCENT[face],
    chip: CHAT_COLORS.surface,
  })),
];

describe("chip blending (A-T5)", () => {
  it("blends a chip over white and black with the real alpha formula", () => {
    expect(blend("#000000", 0.5, "#ffffff")).toBe("#808080");
    expect(blend("#ffffff", 1, "#000000")).toBe("#ffffff");
    expect(blend("#151922", 1, "#ffffff")).toBe("#151922");
  });
});

describe("every text/chip pair reaches 4.5:1 (translucent chips at the 0.9 floor)", () => {
  for (const pair of PAIRS) {
    for (const [label, backdrop] of Object.entries(BACKDROPS)) {
      it(`${pair.name}, over ${label}`, () => {
        const effective = blend(
          pair.chip,
          pair.alpha ?? CHIP_ALPHA_FLOOR,
          backdrop,
        );
        const ratio = contrastRatio(pair.text, effective);
        expect(
          ratio,
          `${pair.name} ${pair.text} on ${effective} (${label})`,
        ).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});

// UI enterprise S2 (A4): the same table runs for BOTH palettes, every accent and every face and
// chip, over white AND black, using the production tokens. The dark-palette literals above are
// pinned to the production values below, so the table can never drift from the code.
describe("the dark literals above are the production dark palette", () => {
  it("pins each chip and text literal to CHAT_PALETTES.dark", () => {
    const dark = CHAT_PALETTES.dark;
    expect(ERROR_TEXT.toLowerCase()).toBe(dark.errorText.toLowerCase());
    expect(ERROR_CHIP.toLowerCase()).toBe(dark.errorChip.toLowerCase());
    expect(USER_BUBBLE_BG.toLowerCase()).toBe(dark.userBubble.toLowerCase());
    expect(USER_TEXT.toLowerCase()).toBe(dark.userText.toLowerCase());
    expect(USER_SENDER.toLowerCase()).toBe(dark.userSender.toLowerCase());
    expect(SYSTEM_CHIP.toLowerCase()).toBe(dark.systemChip.toLowerCase());
    expect(DISABLED_SEND.toLowerCase()).toBe(dark.sendDisabled.toLowerCase());
  });

  it("chat.css reads the chip, text and disabled colours from variables", () => {
    const source = readFileSync(join(here, "chat.css"), "utf8");
    expect(source).toContain(
      "rgba(var(--chat-error-rgb), var(--chat-chip-alpha))",
    );
    expect(source).toContain(
      "rgba(var(--chat-user-rgb), var(--chat-chip-alpha))",
    );
    expect(source).toContain(
      "rgba(var(--chat-system-rgb), var(--chat-chip-alpha))",
    );
    expect(source).toContain("var(--chat-error-text)");
    expect(source).toContain("var(--chat-user-text)");
    expect(source).toContain("var(--chat-user-sender)");
    expect(source).toContain("var(--chat-disabled-bg)");
  });
});

/** Every text/chip pair of one palette and accent, with its label, text, chip and chip alpha. */
function palettePairs(palette: ChatPaletteName, accent: AccentId) {
  const p = chatPaletteFor(palette, accent);
  const translucent = [
    ["surface", p.surface],
    ["background", p.bg],
    ["user bubble", p.userBubble],
    ["system", p.systemChip],
    ["error", p.errorChip],
  ] as const;
  const pairs: Array<{
    name: string;
    text: string;
    chip: string;
    alpha: number;
  }> = [];
  for (const [chipName, chip] of translucent) {
    pairs.push({
      name: `body text on ${chipName}`,
      text: p.text,
      chip,
      alpha: CHIP_ALPHA_FLOOR,
    });
    pairs.push({
      name: `muted text on ${chipName}`,
      text: p.muted,
      chip,
      alpha: CHIP_ALPHA_FLOOR,
    });
    for (const face of FACES) {
      pairs.push({
        name: `${face} face text on ${chipName}`,
        text: FACE_TEXT[palette][face],
        chip,
        alpha: CHIP_ALPHA_FLOOR,
      });
    }
  }
  pairs.push({
    name: "accent text on surface",
    text: p.brandText,
    chip: p.surface,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "accent text on background",
    text: p.brandText,
    chip: p.bg,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "accent text on system",
    text: p.brandText,
    chip: p.systemChip,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "user text on user bubble",
    text: p.userText,
    chip: p.userBubble,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "user sender on user bubble",
    text: p.userSender,
    chip: p.userBubble,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "error text on error chip",
    text: p.errorText,
    chip: p.errorChip,
    alpha: CHIP_ALPHA_FLOOR,
  });
  pairs.push({
    name: "label on brand (opaque)",
    text: p.onBrand,
    chip: p.brand,
    alpha: 1,
  });
  pairs.push({
    name: "muted on disabled send (opaque)",
    text: p.muted,
    chip: p.sendDisabled,
    alpha: 1,
  });
  return pairs;
}

describe("every palette x accent x face x chip reaches 4.5:1 over white and black (UI S2)", () => {
  for (const palette of ["dark", "light"] as const) {
    for (const accent of ACCENT_IDS) {
      it(`${palette} palette, ${accent} accent`, () => {
        const failures: string[] = [];
        for (const pair of palettePairs(palette, accent)) {
          for (const [label, backdrop] of Object.entries(BACKDROPS)) {
            const effective = blend(pair.chip, pair.alpha, backdrop);
            const ratio = contrastRatio(pair.text, effective);
            if (ratio < 4.5) {
              failures.push(
                `${pair.name} over ${label}: ${ratio.toFixed(2)}:1`,
              );
            }
          }
        }
        expect(failures).toEqual([]);
      });
    }
  }
});

describe("the chat stylesheet reads the production palette (no literal chip colours)", () => {
  it("names the error, user, system and disabled-send colours as variables", () => {
    const source = readFileSync(join(here, "chat.css"), "utf8").toLowerCase();
    expect(source).not.toContain(ERROR_TEXT);
    expect(source).not.toContain(DISABLED_SEND);
    expect(source).not.toContain(USER_TEXT);
    expect(source).not.toContain(USER_SENDER);
    expect(source).toContain("var(--chat-error-text)");
  });
});
