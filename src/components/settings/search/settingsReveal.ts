// DOM reveal for settings search: wait for the destination section, open its
// card, find the exact row, scroll to it and highlight it briefly.
// Every DOM and deps call is guarded: nothing here ever throws.
import { openGroup } from "../../ui/collapsibleState";
import type { SearchSection } from "./types";

export interface RevealRequest {
  section: SearchSection;
  groupId?: string;
  labelText: string;
  occurrence?: number;
}

/** The subset of DOM element API the reveal uses (fakeable in tests). */
export interface RevealElement {
  getAttribute(name: string): string | null;
  querySelector(sel: string): RevealElement | null;
  querySelectorAll(sel: string): ArrayLike<RevealElement>;
  getClientRects(): { length: number };
  scrollIntoView(opts: {
    block: "center" | "nearest";
    behavior: "auto" | "smooth";
  }): void;
  classList: { add(c: string): void; remove(c: string): void };
}

export interface RevealDeps {
  doc: { querySelector(sel: string): RevealElement | null };
  raf(cb: (t: number) => void): number;
  caf(id: number): void;
  now(): number;
  setTimeout(fn: () => void, ms: number): number;
  clearTimeout(id: number): void;
  reducedMotion(): boolean;
  openGroup(id: string): void;
}

export const REVEAL_TIMEOUT_MS = 1000;
export const HIGHLIGHT_MS = 1800;
export const HIGHLIGHT_CLASS = "settings-search-highlight";

const ROOT_SELECTOR = "[data-settings-section]";
const SECTION_ATTR = "data-settings-section";
const ROW_SELECTOR = "[data-setting-title]";
const GROUP_ID_PATTERN = /^[A-Za-z0-9_-]+$/;

/** Only called with a groupId that already matches GROUP_ID_PATTERN. */
const groupScopeSel = (groupId: string): string =>
  `[data-group-id="${groupId}"]`;

interface ActiveReveal {
  readonly deps: RevealDeps;
  readonly request: RevealRequest;
  startedAt: number;
  /** True once the destination section root has been observed. */
  arrived: boolean;
  rafId: number | null;
  highlighted: RevealElement | null;
  highlightTimer: number | null;
}

/** The single reveal the module currently owns (null when idle). */
let active: ActiveReveal | null = null;

const attempt = (fn: () => void): void => {
  try {
    fn();
  } catch {
    // DOM and deps failures must not escape the reveal.
  }
};

const defaultDeps = (): RevealDeps => ({
  doc: { querySelector: (sel) => document.querySelector(sel) },
  raf: (cb) => window.requestAnimationFrame(cb),
  caf: (id) => window.cancelAnimationFrame(id),
  now: () => performance.now(),
  setTimeout: (fn, ms) => window.setTimeout(fn, ms),
  clearTimeout: (id) => window.clearTimeout(id),
  reducedMotion: () =>
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches,
  openGroup: (id) => openGroup(id),
});

/** Cancels this reveal's frame and timers and clears its highlight. */
const stop = (reveal: ActiveReveal): void => {
  const { deps } = reveal;
  if (reveal.rafId !== null) {
    const rafId = reveal.rafId;
    reveal.rafId = null;
    attempt(() => deps.caf(rafId));
  }
  if (reveal.highlightTimer !== null) {
    const timer = reveal.highlightTimer;
    reveal.highlightTimer = null;
    attempt(() => deps.clearTimeout(timer));
  }
  if (reveal.highlighted !== null) {
    const row = reveal.highlighted;
    reveal.highlighted = null;
    attempt(() => row.classList.remove(HIGHLIGHT_CLASS));
  }
  if (active === reveal) active = null;
};

/** Same-title rows inside the scope; picks the occurrence-th one that is rendered. */
const findRow = (
  scope: RevealElement | null,
  labelText: string,
  occurrence: number,
): RevealElement | null => {
  if (scope === null) return null;
  // Compare attribute values by iteration: no selector is built from text.
  const sameTitle = Array.from(scope.querySelectorAll(ROW_SELECTOR)).filter(
    (el) => el.getAttribute("data-setting-title") === labelText,
  );
  const row = sameTitle[occurrence];
  if (row === undefined) return null;
  return row.getClientRects().length > 0 ? row : null;
};

const scrollBehavior = (reveal: ActiveReveal): "auto" | "smooth" =>
  reveal.deps.reducedMotion() ? "auto" : "smooth";

const foundRow = (reveal: ActiveReveal, row: RevealElement): void => {
  const { deps } = reveal;
  const behavior = scrollBehavior(reveal);
  attempt(() => row.scrollIntoView({ block: "center", behavior }));
  row.classList.add(HIGHLIGHT_CLASS);
  reveal.highlighted = row;
  reveal.rafId = null;
  reveal.highlightTimer = deps.setTimeout(() => {
    reveal.highlightTimer = null;
    stop(reveal);
  }, HIGHLIGHT_MS);
};

const timedOut = (
  reveal: ActiveReveal,
  root: RevealElement | null,
  groupId: string | undefined,
): void => {
  // Only scroll when the destination actually arrived; never move the outgoing page.
  if (reveal.arrived && root !== null) {
    const group =
      groupId === undefined ? null : root.querySelector(groupScopeSel(groupId));
    const target = group ?? root;
    const behavior = scrollBehavior(reveal);
    attempt(() => target.scrollIntoView({ block: "nearest", behavior }));
  }
  stop(reveal);
};

/** One animation frame of the reveal state machine (WAITING -> ARRIVED -> found/timeout). */
const runFrame = (reveal: ActiveReveal): void => {
  reveal.rafId = null;
  if (active !== reveal) return;
  const { deps, request } = reveal;
  const groupId =
    request.groupId !== undefined && GROUP_ID_PATTERN.test(request.groupId)
      ? request.groupId
      : undefined;

  const root = deps.doc.querySelector(ROOT_SELECTOR);
  if (
    !reveal.arrived &&
    root !== null &&
    root.getAttribute(SECTION_ATTR) === request.section
  ) {
    reveal.arrived = true;
  }

  if (reveal.arrived) {
    // Navigated away after arriving: this reveal is over.
    if (root === null || root.getAttribute(SECTION_ATTR) !== request.section) {
      stop(reveal);
      return;
    }
    if (groupId !== undefined) deps.openGroup(groupId);
    const scope =
      groupId === undefined ? root : root.querySelector(groupScopeSel(groupId));
    const row = findRow(scope, request.labelText, request.occurrence ?? 0);
    if (row !== null) {
      foundRow(reveal, row);
      return;
    }
  }

  if (deps.now() - reveal.startedAt >= REVEAL_TIMEOUT_MS) {
    timedOut(reveal, root, groupId);
    return;
  }
  reveal.rafId = deps.raf(() => runFrame(reveal));
};

/**
 * Starts revealing a setting. Returns the cancel function for THIS reveal;
 * starting a new reveal cancels the previous one. Never throws.
 */
export function revealSetting(
  request: RevealRequest,
  deps: RevealDeps = defaultDeps(),
): () => void {
  const reveal: ActiveReveal = {
    deps,
    request,
    startedAt: 0,
    arrived: false,
    rafId: null,
    highlighted: null,
    highlightTimer: null,
  };
  if (active !== null) stop(active);
  active = reveal;
  try {
    reveal.startedAt = deps.now();
    reveal.rafId = deps.raf(() => runFrame(reveal));
  } catch {
    stop(reveal);
  }
  return () => stop(reveal);
}

/** Cancels the active reveal (if any), including its highlight. Never throws. */
export function cancelReveal(): void {
  if (active !== null) stop(active);
}
