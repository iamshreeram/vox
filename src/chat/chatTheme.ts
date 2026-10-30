// Single source of truth for the conversation window palette (UI enterprise S2, DESIGN Revision 3
// A4). Two palettes, dark (today's exact look, the default) and light. Every colour chat.css paints
// with comes from a variable that chatPaletteVariables() sets on the document, so the hex values
// live only in this module. The brand roles (fill, text, label on fill) come from the canonical
// accent table in src/styles/accents.ts, for the palette's bucket.
//
// FACE_ACCENT keeps the per-face hue as a DECORATIVE stroke or dot. Text that sits on a chip uses
// FACE_TEXT, a readable variant per palette (>= 4.5:1 on every chip; chatOpacityContrast.test.ts).
// This module keeps no imports beyond types and the accent table.
import type { AvatarFace } from "./chatModel";
import { ACCENTS, type AccentId } from "../styles/accents";

export type ChatPaletteName = "dark" | "light";

/** Accent colour per avatar face (#rrggbb). Decorative: never used as body text. */
export const FACE_ACCENT: Record<AvatarFace, string> = {
  idle: "#D9A441",
  listening: "#22D3EE",
  thinking: "#A78BFA",
  acting: "#60A5FA",
  speaking: "#FFC53D",
  sleep: "#94A3B8",
  concerned: "#F87171",
};

/** Readable text colour per face and palette: each one clears 4.5:1 on every chip (A4). */
export const FACE_TEXT: Record<ChatPaletteName, Record<AvatarFace, string>> = {
  dark: {
    idle: "#D9A441",
    listening: "#22D3EE",
    thinking: "#C4B5FD",
    acting: "#93C5FD",
    speaking: "#FFC53D",
    sleep: "#94A3B8",
    concerned: "#FCA5A5",
  },
  light: {
    idle: "#6B4A00",
    listening: "#0B5566",
    thinking: "#6D28D9",
    acting: "#1E40AF",
    speaking: "#6E4800",
    sleep: "#475569",
    concerned: "#991B1B",
  },
};

/** The dark identity neutrals and gold (the Vox icon orb family). Kept as the dark palette's values. */
export const CHAT_COLORS = {
  bg: "#0B0D12",
  surface: "#151922",
  text: "#E7E9EE",
  muted: "#98A1B2",
  brand: "#F5B921",
};

/** The neutrals, chip colours and text colours of one palette (no accent). */
export interface ChatNeutrals {
  bg: string;
  surface: string;
  text: string;
  muted: string;
  userBubble: string;
  userText: string;
  userSender: string;
  systemChip: string;
  errorChip: string;
  errorText: string;
  errorBorder: string;
  sendDisabled: string;
  scrollbar: string;
  border: string;
  shadow: string;
  colorScheme: ChatPaletteName;
}

/** A full palette: the neutrals plus the accent roles of the chosen accent. */
export interface ChatPalette extends ChatNeutrals {
  brand: string;
  brandText: string;
  onBrand: string;
}

export const CHAT_PALETTES: Record<ChatPaletteName, ChatNeutrals> = {
  dark: {
    bg: "#0b0d12",
    surface: "#151922",
    text: "#e7e9ee",
    muted: "#98a1b2",
    userBubble: "#141d38",
    userText: "#eef1ff",
    userSender: "#c9d3f5",
    systemChip: "#11151d",
    errorChip: "#1f1417",
    errorText: "#ff9e94",
    errorBorder: "#5a2a2a",
    sendDisabled: "#2a3140",
    scrollbar: "#2a3140",
    border: "rgba(255, 255, 255, 0.08)",
    shadow: "0 4px 14px rgba(0, 0, 0, 0.45)",
    colorScheme: "dark",
  },
  light: {
    bg: "#f7f8fa",
    surface: "#ffffff",
    text: "#111827",
    muted: "#4b5563",
    userBubble: "#e8f0ff",
    userText: "#0b1b3f",
    userSender: "#1e3a8a",
    systemChip: "#e9edf3",
    errorChip: "#fdecec",
    errorText: "#991b1b",
    errorBorder: "#f0b3b3",
    sendDisabled: "#d1d5db",
    scrollbar: "#cbd2dc",
    border: "rgba(17, 24, 39, 0.12)",
    shadow: "0 4px 14px rgba(15, 23, 42, 0.18)",
    colorScheme: "light",
  },
};

/** "r, g, b" triplet of a "#rrggbb" colour, for `rgba(var(--x), alpha)` in chat.css. */
export function rgbTriplet(hex: string): string {
  const channel = (i: number): number => parseInt(hex.slice(i, i + 2), 16);
  return `${channel(1)}, ${channel(3)}, ${channel(5)}`;
}

/** The rgb triplets main.tsx sets as --chat-bg-rgb and --chat-surface-rgb (kept for compatibility). */
export const CHAT_RGB = {
  bg: rgbTriplet(CHAT_COLORS.bg),
  surface: rgbTriplet(CHAT_COLORS.surface),
};

function channelLuminance(channel: number): number {
  const s = channel / 255;
  return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
}

function relativeLuminance(hex: string): number {
  const r = channelLuminance(parseInt(hex.slice(1, 3), 16));
  const g = channelLuminance(parseInt(hex.slice(3, 5), 16));
  const b = channelLuminance(parseInt(hex.slice(5, 7), 16));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG 2.x contrast ratio of two "#rrggbb" colours (1 to 21). */
export function contrastRatio(a: string, b: string): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** A palette with the brand roles of `accent` (A3 table, the palette's bucket). */
export function chatPaletteFor(
  name: ChatPaletteName,
  accent: AccentId,
): ChatPalette {
  const roles = ACCENTS[accent][name];
  return {
    ...CHAT_PALETTES[name],
    brand: roles.fillA,
    brandText: roles.text,
    onBrand: roles.onFill,
  };
}

/**
 * Every variable the chat window paints from, for the document root. main.tsx sets these; chat.css
 * only reads them. Face text is keyed per face so ChatWindow never needs to know the palette.
 */
export function chatPaletteVariables(
  name: ChatPaletteName,
  accent: AccentId,
): Record<string, string> {
  const p = chatPaletteFor(name, accent);
  const faces = Object.fromEntries(
    (Object.keys(FACE_TEXT[name]) as AvatarFace[]).map((face) => [
      `--chat-face-${face}-text`,
      FACE_TEXT[name][face],
    ]),
  );
  return {
    "--chat-bg-rgb": rgbTriplet(p.bg),
    "--chat-surface-rgb": rgbTriplet(p.surface),
    "--chat-user-rgb": rgbTriplet(p.userBubble),
    "--chat-system-rgb": rgbTriplet(p.systemChip),
    "--chat-error-rgb": rgbTriplet(p.errorChip),
    "--chat-text": p.text,
    "--chat-muted": p.muted,
    "--chat-brand": p.brand,
    "--chat-brand-text": p.brandText,
    "--chat-on-brand": p.onBrand,
    "--chat-user-text": p.userText,
    "--chat-user-sender": p.userSender,
    "--chat-error-text": p.errorText,
    "--chat-error-border": p.errorBorder,
    "--chat-disabled-bg": p.sendDisabled,
    "--chat-scroll": p.scrollbar,
    "--chat-border": p.border,
    "--chat-shadow": p.shadow,
    "--chat-color-scheme": p.colorScheme,
    ...faces,
  };
}

/** Every custom property the chat window reads from its root (set by main.tsx or ChatWindow). */
export const CHAT_VARIABLE_NAMES: string[] = [
  ...Object.keys(chatPaletteVariables("dark", "amber")),
  "--chat-accent",
  "--chat-accent-text",
  "--chat-alpha",
];

/** Inline custom property for .chat-root: the accent of the current face (decorative). */
export function accentStyle(face: AvatarFace): { "--chat-accent": string } {
  return { "--chat-accent": FACE_ACCENT[face] };
}

/** Inline custom property for .chat-root: the readable text colour of the current face. */
export function accentTextStyle(face: AvatarFace): {
  "--chat-accent-text": string;
} {
  return { "--chat-accent-text": `var(--chat-face-${face}-text)` };
}
