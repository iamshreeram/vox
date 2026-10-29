// Opacity of the conversation window page (Track T, DESIGN Revision 3). Pure rules: the page
// reads its first value from the URL (?opacity=&rev=) and then applies `chat-opacity` events, but
// only when the event's rev is strictly higher than the one it has already applied. The page never
// invokes a command: it only listens.

export const CHAT_OPACITY_MIN = 20;
export const CHAT_OPACITY_MAX = 100;
export const CHAT_OPACITY_DEFAULT = 70;

/** A committed value: `rev` grows on every change; `pct` is the opacity in percent. */
export interface OpacityState {
  rev: number;
  pct: number;
}

function isOpacityPct(value: number): boolean {
  return (
    Number.isInteger(value) &&
    value >= CHAT_OPACITY_MIN &&
    value <= CHAT_OPACITY_MAX
  );
}

/** An opacity from the URL or the init value: an integer 20..100, anything else reads as 70. */
export function parseOpacityPct(raw: string | null | undefined): number {
  if (typeof raw !== "string" || !/^-?\d+$/.test(raw.trim())) {
    return CHAT_OPACITY_DEFAULT;
  }
  const value = Number(raw.trim());
  return isOpacityPct(value) ? value : CHAT_OPACITY_DEFAULT;
}

/** The first state: the query's opacity and rev. A missing or invalid rev is 0 (any event wins). */
export function initialOpacity(search: string): OpacityState {
  const params = new URLSearchParams(search);
  const rawRev = params.get("rev");
  const rev = rawRev !== null && /^\d+$/.test(rawRev) ? Number(rawRev) : 0;
  return { rev, pct: parseOpacityPct(params.get("opacity")) };
}

/** A `chat-opacity` payload, or null when it is not `{ rev: positive integer, pct: 20..100 }`. */
export function parseOpacityEvent(payload: unknown): OpacityState | null {
  if (typeof payload !== "object" || payload === null) return null;
  const { rev, pct } = payload as { rev?: unknown; pct?: unknown };
  if (typeof rev !== "number" || !Number.isSafeInteger(rev) || rev < 1) {
    return null;
  }
  if (typeof pct !== "number" || !isOpacityPct(pct)) return null;
  return { rev, pct };
}

/** Applies an event only when its rev is strictly higher; otherwise returns `state` unchanged. */
export function applyOpacityEvent(
  state: OpacityState,
  payload: unknown,
): OpacityState {
  const next = parseOpacityEvent(payload);
  if (next === null || next.rev <= state.rev) return state;
  return next;
}

/** The CSS alpha of the window layer for a percent: 70 -> "0.7". */
export function alphaForPct(pct: number): string {
  return String(pct / 100);
}
