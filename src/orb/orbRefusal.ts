// Orb refusal state (DESIGN Revision 3, A-C2/A-C3). Pure: no DOM, no invoke, no React. The orb page
// listens to `orb-refusal` ({code}) and shows the localised sr-only status text and a short
// `concerned` face. The visual notice is the bubble window; this only feeds assistive tech.

/** The codes the orb may be refused with (A-C2). `unknown` is the fallback. */
export const REFUSAL_CODES = [
  "dictation_busy",
  "tts_off",
  "composer_off",
  "ambient",
  "experimental_off",
  "text_ending",
] as const;

export type RefusalCode = (typeof REFUSAL_CODES)[number] | "unknown";

/** How long the status text stays before it clears. */
export const REFUSAL_TEXT_MS = 4000;
/** How long the orb shows the concerned face. */
export const CONCERNED_MS = 1200;

/** The code in a `{code}` payload when it is known, otherwise `unknown`. Never throws. */
export function refusalCode(payload: unknown): RefusalCode {
  if (
    typeof payload !== "object" ||
    payload === null ||
    Array.isArray(payload)
  ) {
    return "unknown";
  }
  const code = (payload as { code?: unknown }).code;
  return typeof code === "string" &&
    (REFUSAL_CODES as readonly string[]).includes(code)
    ? (code as RefusalCode)
    : "unknown";
}

/** The i18n key of a refusal's text. Never the raw payload. */
export function refusalKey(payload: unknown): string {
  return `orb.refusal.${refusalCode(payload)}`;
}

export interface RefusalState {
  /** Increments on every refusal, so timers can tell their own refusal from a newer one. */
  seq: number;
  /** The i18n key currently shown, or null when nothing is shown. */
  key: string | null;
}

export const initialRefusal: RefusalState = { seq: 0, key: null };

export type RefusalAction =
  | { type: "refuse"; payload: unknown }
  | { type: "clear"; seq?: number };

export function refusalReducer(
  state: RefusalState,
  action: RefusalAction,
): RefusalState {
  if (action.type === "refuse") {
    return { seq: state.seq + 1, key: refusalKey(action.payload) };
  }
  // A timer clears only its own refusal; a click or a conversation start clears unconditionally.
  if (action.seq !== undefined && action.seq !== state.seq) return state;
  if (state.key === null) return state;
  return { seq: state.seq, key: null };
}

/** The part of the timer API this module needs (a fake in tests). */
export interface TimerHost {
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface RefusalTimerCallbacks {
  /** The concerned face's time is over: restore the conversation face. */
  onConcernedEnd: () => void;
  /** The status text's time is over: clear it. */
  onTextEnd: () => void;
}

/** Starts the two timers of one refusal. The returned function cancels both. */
export function startRefusalTimers(
  host: TimerHost,
  callbacks: RefusalTimerCallbacks,
): () => void {
  const concerned = host.setTimeout(callbacks.onConcernedEnd, CONCERNED_MS);
  const text = host.setTimeout(callbacks.onTextEnd, REFUSAL_TEXT_MS);
  return () => {
    host.clearTimeout(concerned);
    host.clearTimeout(text);
  };
}
