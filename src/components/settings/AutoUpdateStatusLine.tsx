import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands, type AutoUpdateStatus } from "../../bindings";

/** Event the backend emits with the current AutoUpdateStatus. */
const AUTO_UPDATE_EVENT = "auto-update-state-changed";

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

export interface StatusLine {
  key: string;
  options?: Record<string, string>;
  /** Key of the reason text to translate and pass in as `{{reason}}`. */
  reasonKey?: string;
}

/**
 * Picks the line to show, or null for none. Pure: the caller translates it.
 * `date` is the already-formatted time of the last attempt, or null.
 */
export function statusLine(
  status: AutoUpdateStatus | null,
  date: string | null,
): StatusLine | null {
  if (status === null) return null;
  switch (status.phase) {
    case "building":
      return { key: `${KEY_PREFIX}.status.building` };
    case "waiting":
      return { key: `${KEY_PREFIX}.status.waiting` };
    case "installing":
      return { key: `${KEY_PREFIX}.status.installing` };
    case "idle":
      break;
  }
  const target = status.target_short ?? "";
  if (status.last_result === "updated") {
    return date === null
      ? { key: `${KEY_PREFIX}.status.updated`, options: { target } }
      : { key: `${KEY_PREFIX}.status.updatedAt`, options: { target, date } };
  }
  if (status.last_result === "failed" || status.last_result === "skipped") {
    return {
      key: `${KEY_PREFIX}.status.${status.last_result}`,
      reasonKey: reasonKey(status.reason_code),
    };
  }
  // last_result none: nothing has run yet, so there is nothing to report.
  return null;
}

type TranslateFn = (key: string, options?: Record<string, string>) => string;

function renderLine(t: TranslateFn, line: StatusLine): string {
  if (line.reasonKey === undefined) return t(line.key, line.options);
  return t(line.key, { reason: t(line.reasonKey) });
}

interface AutoUpdateStatusLineProps {
  className?: string;
}

/**
 * Status of the automatic update for the Settings page. Always mounted so the
 * live region is ready; empty (and so hidden) when there is nothing to say.
 */
export const AutoUpdateStatusLine: React.FC<AutoUpdateStatusLineProps> = ({
  className = "",
}) => {
  const { t, i18n } = useTranslation();
  const [status, setStatus] = useState<AutoUpdateStatus | null>(null);

  useEffect(() => {
    let disposed = false;
    // A pushed status is newer than any fetch that was still in flight.
    let pushed = false;
    let unlisten: UnlistenFn | null = null;

    listen<AutoUpdateStatus>(AUTO_UPDATE_EVENT, (event) => {
      if (disposed) return;
      pushed = true;
      setStatus(event.payload);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((error) => {
        console.error("Failed to subscribe to auto-update events:", error);
      });

    commands
      .getAutoUpdateStatus()
      .then((value) => {
        if (!disposed && !pushed) setStatus(value);
      })
      .catch((error) => {
        console.error("Failed to read auto-update status:", error);
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const ts = status?.ts ?? null;
  const date =
    ts === null
      ? null
      : new Intl.DateTimeFormat(i18n.language, {
          dateStyle: "medium",
          timeStyle: "short",
        }).format(new Date(ts * 1000));
  const line = statusLine(status, date);

  return (
    <p
      role="status"
      aria-live="polite"
      className={`text-xs text-muted empty:hidden ${className}`}
    >
      {line === null ? "" : renderLine(t, line)}
    </p>
  );
};
