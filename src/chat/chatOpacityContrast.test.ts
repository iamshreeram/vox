// Track T spec (DESIGN Revision 3, A-T5): table-driven contrast with the production palette and
// real alpha blending. A text chip is the palette surface at alpha max(--chat-alpha, 0.9); the
// window content behind it is white or black in the worst cases. Every text/chip pair must reach
// WCAG AA 4.5:1 for normal text. Workers must not edit this file.
import { describe, expect, it } from "vitest";
import { CHAT_COLORS, FACE_ACCENT, contrastRatio } from "./chatTheme";
import type { AvatarFace } from "./chatModel";

/** The chip floor: text chips are never more transparent than this (chat.css --chat-chip-alpha). */
const CHIP_ALPHA_FLOOR = 0.9;
/** Text colours chat.css sets by name (not in CHAT_COLORS): error text and the user bubble. */
const ERROR_TEXT = "#ff9e94";
const USER_BUBBLE_BG = "#141d38";
const USER_TEXT = "#eef1ff";
const USER_SENDER = "#c9d3f5";

const FACES: AvatarFace[] = [
  "idle",
  "listening",
  "thinking",
  "acting",
  "speaking",
  "sleep",
  "concerned",
];

/** The colour of an opaque-ish chip (`fg` at `alpha`) over an opaque `backdrop`, as #rrggbb. */
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
    name: "timestamp on bubble chip",
    text: CHAT_COLORS.muted,
    chip: CHAT_COLORS.surface,
  },
  {
    name: "error text on error chip",
    text: ERROR_TEXT,
    chip: CHAT_COLORS.surface,
  },
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

describe("every text/chip pair reaches 4.5:1 at the 0.9 chip floor", () => {
  for (const pair of PAIRS) {
    for (const [label, backdrop] of Object.entries(BACKDROPS)) {
      it(`${pair.name}, over ${label}`, () => {
        const effective = blend(pair.chip, CHIP_ALPHA_FLOOR, backdrop);
        const ratio = contrastRatio(pair.text, effective);
        expect(
          ratio,
          `${pair.name} ${pair.text} on ${effective} (${label})`,
        ).toBeGreaterThanOrEqual(4.5);
      });
    }
  }
});

describe("the chat stylesheet uses the same error and user colours", () => {
  it("names the error text and the user bubble colours the table tests", async () => {
    const { readFileSync } = await import("node:fs");
    const { dirname, join } = await import("node:path");
    const { fileURLToPath } = await import("node:url");
    const here = dirname(fileURLToPath(import.meta.url));
    const css = readFileSync(join(here, "chat.css"), "utf8").toLowerCase();
    expect(css).toContain(ERROR_TEXT);
    // the user bubble is written as its rgb triplet so it can sit on the chip alpha
    expect(css).toContain("rgba(20, 29, 56, var(--chat-chip-alpha))");
    expect(css).toContain(USER_TEXT);
    expect(css).toContain(USER_SENDER);
  });
});
