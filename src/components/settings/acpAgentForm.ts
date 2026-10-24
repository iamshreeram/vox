/**
 * Pure input rules for the ACP agent settings (DESIGN 7 and 8). No React, no Tauri imports.
 * The bounds mirror the Rust validators in settings.rs; the backend stays authoritative.
 */

export const ACP_TIMEOUT_MIN_SECS = 10;
export const ACP_TIMEOUT_MAX_SECS = 900;
export const ACP_MAX_ARGS = 8;
export const ACP_MAX_ARG_LEN = 256;
export const ACP_MAX_COMMAND_LEN = 1024;

/** Error codes; each maps to a `settings.advanced.acpAgent.errors.<code>` translation key. */
export type AcpFormError =
  | "notAbsolute"
  | "tooLong"
  | "controlChars"
  | "tooMany"
  | "argTooLong"
  | "outOfRange"
  | "notInteger";

export type AcpFormResult<T> =
  | { ok: true; value: T }
  | { ok: false; error: AcpFormError };

/** Any control character (C0 and DEL), tab excluded. */
const CONTROL_EXCEPT_TAB = /[\u0000-\u0008\u000a-\u001f\u007f]/;
/** Any control character, tab included. */
const CONTROL_ANY = /[\u0000-\u001f\u007f]/;
/** Absolute path forms Rust's Path::is_absolute accepts: POSIX, Windows drive, UNC. */
const ABSOLUTE_PATH = /^(\/|[A-Za-z]:[\\/]|\\\\)/;
const SEPARATOR = /[ \t]+/;
const INTEGER = /^-?\d+$/;

/** Blank means "unset" (null). Otherwise the trimmed value must be an absolute path. */
export function parseAcpCommand(raw: string): AcpFormResult<string | null> {
  if (raw.trim() === "") return { ok: true, value: null };
  if (CONTROL_ANY.test(raw)) return { ok: false, error: "controlChars" };
  const value = raw.trim();
  if (value.length > ACP_MAX_COMMAND_LEN) {
    return { ok: false, error: "tooLong" };
  }
  if (!ABSOLUTE_PATH.test(value)) return { ok: false, error: "notAbsolute" };
  return { ok: true, value };
}

/** Whitespace split only (spaces and tabs). No quoting and no shell parsing. */
export function splitAcpArgs(raw: string): string[] {
  return raw.split(SEPARATOR).filter((part) => part !== "");
}

/** The raw string is stored as typed; the split happens at launch. */
export function parseAcpArgs(raw: string): AcpFormResult<string> {
  if (CONTROL_EXCEPT_TAB.test(raw)) return { ok: false, error: "controlChars" };
  const parts = splitAcpArgs(raw);
  if (parts.length > ACP_MAX_ARGS) return { ok: false, error: "tooMany" };
  if (parts.some((part) => part.length > ACP_MAX_ARG_LEN)) {
    return { ok: false, error: "argTooLong" };
  }
  return { ok: true, value: raw };
}

/** A whole number of seconds within the allowed range. */
export function parseAcpTimeout(raw: string): AcpFormResult<number> {
  const trimmed = raw.trim();
  if (!INTEGER.test(trimmed)) return { ok: false, error: "notInteger" };
  const value = Number(trimmed);
  if (value < ACP_TIMEOUT_MIN_SECS || value > ACP_TIMEOUT_MAX_SECS) {
    return { ok: false, error: "outOfRange" };
  }
  return { ok: true, value };
}

/** Bounds for the approval timeout, matching the backend clamp (settings.rs). */
export const ACP_PERMISSION_TIMEOUT_MIN_SECS = 10;
export const ACP_PERMISSION_TIMEOUT_MAX_SECS = 300;

/**
 * Parses an approval timeout draft and clamps it into the allowed range.
 * Returns null when the draft is not a whole number.
 */
export function clampPermissionTimeout(raw: string): number | null {
  const trimmed = raw.trim();
  if (!INTEGER.test(trimmed)) return null;
  const value = Number(trimmed);
  return Math.min(
    ACP_PERMISSION_TIMEOUT_MAX_SECS,
    Math.max(ACP_PERMISSION_TIMEOUT_MIN_SECS, value),
  );
}
