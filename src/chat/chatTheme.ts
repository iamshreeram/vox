// Single source of truth for the conversation window palette (DESIGN-UI "Visual design").
// The window is a fixed dark theme; the per-state accent is set inline on .chat-root from
// accentStyle(face). chat.css only reads var(--chat-accent) and never repeats these values.
// This module is also imported by a Playwright spec, so it keeps no imports beyond a type.
import type { AvatarFace } from "./chatModel";

/** Accent colour per avatar face (#rrggbb). Every face is distinct. */
export const FACE_ACCENT: Record<AvatarFace, string> = {
  idle: "#D9A441",
  listening: "#22D3EE",
  thinking: "#A78BFA",
  acting: "#60A5FA",
  speaking: "#FFC53D",
  sleep: "#64748B",
  concerned: "#F87171",
};

/** Fixed dark neutrals and the brand gold (the Vox icon orb family). */
export const CHAT_COLORS = {
  bg: "#0B0D12",
  surface: "#151922",
  text: "#E7E9EE",
  muted: "#98A1B2",
  brand: "#F5B921",
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

/** Inline custom property for .chat-root: the accent of the current face. */
export function accentStyle(face: AvatarFace): { "--chat-accent": string } {
  return { "--chat-accent": FACE_ACCENT[face] };
}
