// Typing box rules for the conversation window (DESIGN-TYPE Rev 4). Pure helpers, no React.
// MAX_CHARS mirrors the backend limit (MAX_TYPED_CHARS in src-tauri); the backend has the last word.

/** Longest message the composer accepts (characters, after trimming). */
export const MAX_CHARS = 2000;

/** The character counter appears from this length on. */
export const COUNTER_FROM = 1800;

export type PreparedSend =
  | { ok: true; text: string }
  | { ok: false; reason: "empty" | "too_long" };

/** Trims the ends (inner newlines stay) and rejects empty or over-long text. */
export function prepareSend(raw: string): PreparedSend {
  const text = raw.trim();
  if (text.length === 0) return { ok: false, reason: "empty" };
  if (text.length > MAX_CHARS) return { ok: false, reason: "too_long" };
  return { ok: true, text };
}

const ERROR_KEYS = new Map<string, string>([
  ["not_configured", "chat.sendError.not_configured"],
  ["busy", "chat.sendError.busy"],
  ["too_long", "chat.sendError.too_long"],
  ["failed", "chat.sendError.failed"],
]);

/** Maps a backend error code to its translation key; anything unknown is a generic failure. */
export function errorKey(code: string): string {
  return ERROR_KEYS.get(code) ?? "chat.sendError.failed";
}

/** Enter submits; Shift/Alt+Enter and Enter during IME composition do not. */
export function shouldSubmitOnKey(e: {
  key: string;
  shiftKey: boolean;
  altKey: boolean;
  isComposing: boolean;
  keyCode?: number;
}): boolean {
  if (e.key !== "Enter") return false;
  if (e.shiftKey || e.altKey) return false;
  if (e.isComposing || e.keyCode === 229) return false;
  return true;
}

/** The counter is shown only near the limit. */
export function counterVisible(length: number): boolean {
  return length >= COUNTER_FROM;
}
