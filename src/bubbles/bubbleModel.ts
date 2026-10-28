// Pure page model for the bubble stack (DESIGN.md A7-2, A8-2, R6-4). No DOM, no timers, no
// React: the page renders what this module computes. Rust is the authority for lifetimes; the
// page only fades locally from the ttl and age that Rust sent.

export const FADE_MS = 600;

const ENTER_MS = 180;
const ENTER_PX = 8;
const DISPLAY_CAP = 240;

export type BubbleRole = "user" | "assistant";

/** One bubble as Rust sends it in a snapshot. */
export type BubbleItemWire = {
  id: number;
  role: BubbleRole;
  text: string;
  age_ms: number;
  ttl_ms: number;
};

/** Where the stack hugs its window: the top when it sits below the orb, else the bottom. */
export type BubbleAnchor = "top" | "bottom";

/** A snapshot revision as Rust sends it. `anchor` is missing in older payloads. */
export type Snapshot = {
  instance: number;
  rev: number;
  anchor?: BubbleAnchor;
  items: BubbleItemWire[];
};

type StoredBubble = {
  id: number;
  role: BubbleRole;
  text: string;
  bornAt: number;
  ttlMs: number;
};

export type PageState = {
  instance: number;
  rev: number;
  anchor: BubbleAnchor;
  items: StoredBubble[];
};

export type VisibleBubble = {
  id: number;
  role: BubbleRole;
  text: string;
  opacity: number;
};

export type EnterMotion = {
  durationMs: number;
  translatePx: number;
};

/**
 * The anchor of a snapshot. Missing or unknown values mean "bottom", so older payloads still
 * render as before.
 */
export function anchorOf(value: unknown): BubbleAnchor {
  return value === "top" ? "top" : "bottom";
}

/** A fresh page state for one window instance. Rust revisions start at 1, so 0 accepts any. */
export function createState(instance: number): PageState {
  return { instance, rev: 0, anchor: "bottom", items: [] };
}

/**
 * Applies a snapshot. Ignored when it is for another instance or is older than the last revision
 * applied. A newer revision replaces the whole list; each bubble is born at `nowMs - age_ms`, so a
 * replay keeps the original lifetime. A snapshot with the same revision only changes the anchor:
 * the bubbles and their lifetimes are kept.
 */
export function applySnapshot(
  state: PageState,
  snapshot: Snapshot,
  nowMs: number,
): PageState {
  if (snapshot.instance !== state.instance || snapshot.rev < state.rev) {
    return state;
  }
  const anchor = anchorOf(snapshot.anchor);
  if (snapshot.rev === state.rev) {
    return anchor === state.anchor ? state : { ...state, anchor };
  }
  return {
    instance: state.instance,
    rev: snapshot.rev,
    anchor,
    items: snapshot.items.map((item) => ({
      id: item.id,
      role: item.role,
      text: item.text,
      bornAt: nowMs - item.age_ms,
      ttlMs: item.ttl_ms,
    })),
  };
}

/**
 * Bubbles alive at `nowMs`, oldest first. Opacity is 1 until `ttl - FADE_MS`, then falls
 * linearly to 0 at `ttl`. A bubble is removed once its age reaches its ttl.
 */
export function visibleBubbles(
  state: PageState,
  nowMs: number,
): VisibleBubble[] {
  const visible: VisibleBubble[] = [];
  for (const item of state.items) {
    const age = nowMs - item.bornAt;
    if (age >= item.ttlMs) {
      continue;
    }
    const fadeStart = item.ttlMs - FADE_MS;
    const opacity = age <= fadeStart ? 1 : (item.ttlMs - age) / FADE_MS;
    visible.push({ id: item.id, role: item.role, text: item.text, opacity });
  }
  return visible;
}

/** Your lines sit on the right and Vox's on the left. */
export function alignFor(role: BubbleRole): "right" | "left" {
  return role === "user" ? "right" : "left";
}

/**
 * How many of the newest bubbles fit in `container`, given their heights and the gap between
 * neighbours. Oldest are dropped first. The newest is always kept when there is any bubble.
 */
export function fitBubbles(
  heights: readonly number[],
  container: number,
  gap: number,
): number {
  let used = 0;
  let count = 0;
  for (const height of [...heights].reverse()) {
    const needed = count === 0 ? height : height + gap;
    if (count > 0 && used + needed > container) {
      break;
    }
    used += needed;
    count += 1;
  }
  return count;
}

/** Enter animation: 180 ms with an 8 px slide, or an instant change under reduced motion. */
export function enterMotion(reducedMotion: boolean): EnterMotion {
  return reducedMotion
    ? { durationMs: 0, translatePx: 0 }
    : { durationMs: ENTER_MS, translatePx: ENTER_PX };
}

/**
 * Display text: whitespace collapsed and trimmed, at most 240 code points. A longer text is cut
 * at a word boundary and ends with an ellipsis. Mirrors the Rust cap_display_text rule.
 */
export function capText(text: string): string {
  const collapsed = text
    .split(/\s+/)
    .filter((part) => part.length > 0)
    .join(" ");
  const chars = Array.from(collapsed);
  if (chars.length <= DISPLAY_CAP) {
    return collapsed;
  }
  // One code point is reserved for the ellipsis.
  const budget = DISPLAY_CAP - 1;
  let cut = budget;
  const next = chars[budget];
  const atBoundary = next === undefined || /\s/.test(next);
  if (!atBoundary) {
    const space = chars.lastIndexOf(" ", budget - 1);
    if (space > 0) {
      cut = space;
    }
  }
  return `${chars.slice(0, cut).join("").trimEnd()}…`;
}
