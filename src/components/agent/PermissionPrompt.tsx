import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import {
  permissionViewModel,
  type PermissionRequestEvent,
} from "./permissionViewModel";

const EVENT_REQUEST = "acp-permission-request";
const EVENT_CLOSED = "acp-permission-closed";
const KEY_PREFIX = "settings.advanced.acpPermission.prompt";

/** A request waiting for an answer, with the time it was received (for the countdown). */
interface QueuedRequest extends PermissionRequestEvent {
  receivedAt: number;
}

/** Whole seconds left before the backend rejects the request on its own. */
function secondsLeft(request: QueuedRequest, now: number): number {
  const elapsed = Math.floor((now - request.receivedAt) / 1000);
  return Math.max(0, request.timeoutSecs - elapsed);
}

/**
 * Shows one approval request at a time for agent tool calls. Deny is the default:
 * it has initial focus, and Enter and Escape both deny. Allow needs an explicit click.
 * Titles and details come from the agent and are rendered only as text.
 */
export default function PermissionPrompt() {
  const { t } = useTranslation();
  const [queue, setQueue] = useState<QueuedRequest[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const answeringRef = useRef<string | null>(null);

  // Receive requests and closures. Each listener is removed on unmount.
  useEffect(() => {
    const unlisteners: Promise<UnlistenFn>[] = [
      listen<PermissionRequestEvent>(EVENT_REQUEST, (event) => {
        const request: QueuedRequest = {
          ...event.payload,
          receivedAt: Date.now(),
        };
        setQueue((items) =>
          items.some((item) => item.token === request.token)
            ? items
            : [...items, request],
        );
      }),
      listen<{ token: string }>(EVENT_CLOSED, (event) => {
        const { token } = event.payload;
        setQueue((items) => items.filter((item) => item.token !== token));
      }),
    ];
    return () => {
      unlisteners.forEach((pending) => {
        pending.then((unlisten) => unlisten());
      });
    };
  }, []);

  const current = queue[0];

  // Tick once a second while a request is shown, for the countdown.
  useEffect(() => {
    if (!current) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [current]);

  const answer = (token: string, allow: boolean) => {
    // Ignore repeated activations for the same request while the answer is sent.
    if (answeringRef.current === token) return;
    answeringRef.current = token;
    commands
      .acpPermissionRespond(token, allow)
      .catch(() => false)
      .finally(() => {
        answeringRef.current = null;
        setQueue((items) => items.filter((item) => item.token !== token));
      });
  };

  // Escape and Enter deny. Allow is never triggered from the keyboard by default.
  useEffect(() => {
    if (!current) return;
    const token = current.token;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" || event.key === "Enter") {
        event.preventDefault();
        answer(token, false);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [current?.token]);

  if (!current) return null;

  const view = permissionViewModel(current);
  const queued = view.queued;
  const seconds = secondsLeft(current, now);
  const titleId = `acp-permission-title-${view.token}`;
  const descriptionId = `acp-permission-description-${view.token}`;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={descriptionId}
        className="flex max-h-[calc(100dvh-2rem)] w-full max-w-lg flex-col gap-3 overflow-hidden rounded-lg border border-mid-gray/20 bg-background p-4 shadow-xl"
      >
        <div className="flex items-start justify-between gap-3">
          <h2 id={titleId} className="text-base font-semibold text-text">
            {t(`${KEY_PREFIX}.title`)}
          </h2>
          <span className="shrink-0 rounded-md border border-mid-gray/40 px-2 py-0.5 text-xs font-medium text-text">
            {t(view.kindLabelKey)}
          </span>
        </div>
        <p id={descriptionId} className="text-sm text-mid-gray">
          {t(`${KEY_PREFIX}.untrustedNote`)}
        </p>
        <p className="break-words text-sm text-text">{view.title}</p>
        {view.showUnavailableWarning ? (
          <p
            role="note"
            className="rounded-md border border-warning bg-warning-soft p-2 text-sm font-medium text-text"
          >
            {t(`${KEY_PREFIX}.detailsUnavailable`)}
          </p>
        ) : (
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border border-mid-gray/30 p-2 font-mono text-xs text-text">
            {view.detailsText}
          </pre>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-mid-gray">
          <span>{t(`${KEY_PREFIX}.countdown`, { seconds })}</span>
          {queued > 0 && (
            <span>{t(`${KEY_PREFIX}.queued`, { count: queued })}</span>
          )}
        </div>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            onClick={() => answer(view.token, true)}
            className="cursor-pointer rounded-md border border-mid-gray/30 px-3 py-1.5 text-sm text-text hover:bg-mid-gray/10"
          >
            {t(`${KEY_PREFIX}.allowOnce`)}
          </button>
          <button
            key={view.token}
            type="button"
            autoFocus={view.defaultFocus === "deny"}
            onClick={() => answer(view.token, false)}
            className="cursor-pointer rounded-md border border-logo-primary bg-logo-primary/20 px-3 py-1.5 text-sm font-medium text-text hover:bg-logo-primary/30"
          >
            {t(`${KEY_PREFIX}.deny`)}
          </button>
        </div>
      </div>
    </div>
  );
}
