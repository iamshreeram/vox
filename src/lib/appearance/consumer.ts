// Shared appearance consumer for the listen-only pages: the conversation window, the orb, the
// bubbles and (through theme.ts) the main windows (UI enterprise S2, DESIGN Revision 3 A1/A2).
// The page seeds its first paint from the `ap` query parameter the Rust side writes, then applies
// `appearance` events. An event applies only when its rev is strictly higher than the one applied.
// The consumer never invokes a command and never reads a settings store.
import type { AppearanceSnapshot } from "@/bindings";
import {
  applyResolvedAppearance,
  resolveAppearance,
  type AppearanceRoot,
  type ResolvedAppearance,
} from "./palette";
import { ACCENT_IDS } from "../../styles/accents";

/** The Tauri event that carries a committed snapshot to the consumer windows. */
export const APPEARANCE_EVENT = "appearance";
/** The URL query parameter that carries the first-paint seed (percent-encoded JSON). */
export const SEED_PARAM = "ap";
/** A seed longer than this (after decoding) is refused before it is parsed. */
export const MAX_SEED_LENGTH = 4096;

const DARK_QUERY = "(prefers-color-scheme: dark)";
const THEMES = ["system", "light", "dark", "aurora", "custom"] as const;
const CHAT_THEMES = ["dark", "follow_app", "light"] as const;
const VISUAL_STYLES = ["vibrant", "restrained"] as const;
const COLOR_FIELDS = [
  "text",
  "background",
  "accent",
  "accent_secondary",
  "warning",
  "error",
] as const;
const HEX6 = /^#[0-9a-fA-F]{6}$/;

const oneOf = <T extends string>(
  value: unknown,
  options: readonly T[],
): T | null =>
  typeof value === "string" && (options as readonly string[]).includes(value)
    ? (value as T)
    : null;

/**
 * The first-paint seed from a page URL (`window.location.search`). Returns null when the parameter
 * is missing, oversized, not JSON, or not a valid snapshot. Never throws.
 */
export function parseSeed(search: string): AppearanceSnapshot | null {
  const raw = new URLSearchParams(search).get(SEED_PARAM);
  if (raw === null || raw.length === 0 || raw.length > MAX_SEED_LENGTH) {
    return null;
  }
  let payload: unknown;
  try {
    payload = JSON.parse(raw);
  } catch {
    return null;
  }
  return parseSnapshot(payload);
}

/**
 * A strict snapshot from an untrusted payload (an event or a seed). Unknown enum values, a bad
 * revision, a malformed custom palette or a custom palette on a non-custom theme all reject the
 * whole payload. Extra keys are ignored (forward compatibility).
 */
export function parseSnapshot(payload: unknown): AppearanceSnapshot | null {
  if (
    typeof payload !== "object" ||
    payload === null ||
    Array.isArray(payload)
  ) {
    return null;
  }
  const p = payload as Record<string, unknown>;
  const rev = p.rev;
  if (typeof rev !== "number" || !Number.isSafeInteger(rev) || rev < 1) {
    return null;
  }
  const theme = oneOf(p.theme, THEMES);
  const accent = oneOf(p.accent, ACCENT_IDS);
  const chatTheme = oneOf(p.chat_theme, CHAT_THEMES);
  const visualStyle = oneOf(p.visual_style, VISUAL_STYLES);
  if (
    theme === null ||
    accent === null ||
    chatTheme === null ||
    visualStyle === null
  ) {
    return null;
  }

  let customColors: AppearanceSnapshot["custom_colors"] = null;
  if (theme === "custom") {
    const colors = p.custom_colors;
    if (
      typeof colors !== "object" ||
      colors === null ||
      Array.isArray(colors)
    ) {
      return null;
    }
    const record = colors as Record<string, unknown>;
    for (const field of COLOR_FIELDS) {
      const value = record[field];
      if (typeof value !== "string" || !HEX6.test(value)) return null;
    }
    customColors = {
      text: record.text as string,
      background: record.background as string,
      accent: record.accent as string,
      accent_secondary: record.accent_secondary as string,
      warning: record.warning as string,
      error: record.error as string,
    };
  } else if (p.custom_colors !== null) {
    return null;
  }

  // The orb accent is optional and normalised: a missing or unknown value reads as null (follow
  // the app accent), so an older or garbled snapshot never rejects the whole appearance.
  const orbAccent = oneOf(p.orb_accent, ACCENT_IDS);

  return {
    rev,
    theme,
    custom_colors: customColors,
    accent,
    chat_theme: chatTheme,
    visual_style: visualStyle,
    orb_accent: orbAccent,
  };
}

/** True only when the incoming revision is strictly higher than the applied one. */
export function shouldApply(appliedRev: number, incomingRev: number): boolean {
  return incomingRev > appliedRev;
}

/** The subset of `matchMedia` the consumer needs (a fake in tests). */
export type MatchMediaLike = (query: string) => {
  readonly matches: boolean;
  addEventListener?: (type: "change", listener: () => void) => void;
};

export interface AppearanceConsumer {
  /** Applies a payload when it is a valid, strictly newer snapshot. Returns whether it applied. */
  accept(payload: unknown): boolean;
  /** The revision applied so far (0 before the first snapshot). */
  appliedRev(): number;
  /** The snapshot applied last, or null. */
  snapshot(): AppearanceSnapshot | null;
}

export interface AppearanceConsumerOptions {
  root: AppearanceRoot;
  matchMedia: MatchMediaLike;
  /** Called after every apply, including a live OS colour-scheme change that alters the result. */
  onApplied?: (
    resolved: ResolvedAppearance,
    snapshot: AppearanceSnapshot,
  ) => void;
}

/**
 * One consumer per page. It holds the applied revision, applies valid newer snapshots to the root,
 * and re-resolves live when the OS colour scheme changes while the result depends on it (system
 * theme, or a chat palette that follows the app).
 */
export function createAppearanceConsumer(
  options: AppearanceConsumerOptions,
): AppearanceConsumer {
  let applied = 0;
  let current: AppearanceSnapshot | null = null;

  const prefersDark = (): boolean => options.matchMedia(DARK_QUERY).matches;

  const apply = (): void => {
    if (current === null) return;
    const resolved = resolveAppearance(current, prefersDark());
    applyResolvedAppearance(options.root, resolved);
    options.onApplied?.(resolved, current);
  };

  options.matchMedia(DARK_QUERY).addEventListener?.("change", () => {
    if (current === null) return;
    if (current.theme === "system" || current.chat_theme === "follow_app") {
      apply();
    }
  });

  return {
    accept(payload: unknown): boolean {
      const next = parseSnapshot(payload);
      if (next === null || !shouldApply(applied, next.rev)) return false;
      applied = next.rev;
      current = next;
      apply();
      return true;
    },
    appliedRev: () => applied,
    snapshot: () => current,
  };
}
