import type { AutoUpdateStatus } from "../../bindings";

/** localStorage key holding the timestamp (unix seconds) of the dismissed failure. */
export const DISMISS_KEY = "vox.autoUpdate.dismissedFailureTs";

/** Reason codes the backend may send; anything else uses the generic text. */
const KNOWN_REASON_CODES = new Set([
  "disabled",
  "not_source",
  "no_update",
  "blocked",
  "backoff",
  "unverified",
  "verify_failed",
  "active_run",
  "busy",
  "not_idle",
  "verifying",
  "interrupted",
  "build_failed",
  "install_failed",
  "spawn_failed",
  "invalid_pending",
]);

const KEY_PREFIX = "settings.debug.autoUpdate";

/** Stable code to i18n key. The reason text never comes from the backend. */
export function reasonKey(code: string | null): string {
  const known =
    code !== null && KNOWN_REASON_CODES.has(code) ? code : "unknown";
  return `${KEY_PREFIX}.reasons.${known}`;
}

export interface AutoUpdateFailureNotice {
  /** i18n key of the reason text (translate it, then pass it as `reason`). */
  reasonKey: string;
  /** Unix seconds of the failed attempt. */
  ts: number;
  /** Short commit of the target, or null when the backend did not record one. */
  targetShort: string | null;
}

/**
 * The footer notice for the last automatic update, or null for none.
 * Shown only for a finished, failed, enabled attempt that the user has not
 * dismissed. Pure: the caller reads the dismissed timestamp and translates.
 */
export function autoUpdateFailureNotice(
  status: AutoUpdateStatus | null,
  dismissedTs: number | null,
): AutoUpdateFailureNotice | null {
  if (
    status === null ||
    !status.enabled ||
    status.phase !== "idle" ||
    status.last_result !== "failed" ||
    status.ts === null ||
    status.ts === dismissedTs
  ) {
    return null;
  }
  return {
    reasonKey: reasonKey(status.reason_code),
    ts: status.ts,
    targetShort: status.target_short,
  };
}

/** Parses the stored dismissed timestamp. Only plain digit strings are accepted. */
export function parseDismissed(raw: string | null): number | null {
  if (raw === null || !/^\d+$/.test(raw)) return null;
  const value = Number(raw);
  return Number.isSafeInteger(value) ? value : null;
}
