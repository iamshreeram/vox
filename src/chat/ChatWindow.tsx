// Conversation window body. Protocol (DESIGN-CHAT A4): subscribe to `converse-chat` first,
// buffer until the first snapshot is applied, then apply live events; a gap refetches once,
// a stale snapshot is discarded and refetched at most once more, window focus refetches once.
// The only commands used are get_chat_snapshot, clear_chat_log, send_chat_message (DESIGN-TYPE),
// chat_hide_window (Escape and the Hide button hide the window, DESIGN D1b) and chat_stop_conversation
// (the Stop control, DESIGN Revision 4 B2b; it passes the session id the control was rendered with).
// The typing box sends a message and clears itself only on success; the transcript line arrives
// through the same converse-chat events as a spoken one (no local echo).
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import type {
  ChangeEvent,
  CSSProperties,
  FormEvent,
  KeyboardEvent,
} from "react";
import VoxAvatar from "./VoxAvatar";
import { getChatOrbLook, subscribeChatOrbLook } from "./chatOrbLook";
import {
  CLEAR_CONFIRM_MS,
  clearSecondsLeft,
  nextClearStep,
  type ClearStep,
} from "./clearConfirm";
import { stopVisible } from "./stopControl";
import { createOnceGuard, escAction } from "./escHide";
import voxIcon from "./assets/vox-icon.png";
import { accentStyle, accentTextStyle } from "./chatTheme";
import {
  MAX_CHARS,
  counterVisible,
  errorKey,
  prepareSend,
  shouldSubmitOnKey,
} from "./composer";
import {
  applyEvent,
  applySnapshot,
  avatarFace,
  describeSystem,
  initialView,
  needsResync,
  shouldAutoScroll,
  type ChatEntry,
  type ChatEvent,
  type ChatSnapshot,
  type ChatView,
} from "./chatModel";

/** Events that arrive before the first snapshot is applied; older ones are dropped beyond this. */
const PRE_HYDRATE_BUFFER_CAP = 1000;

type TFn = ReturnType<typeof useTranslation>["t"];

function captionFor(
  state: string,
  end: string | null,
): { key: string; reasonKey?: string } {
  const face = avatarFace(state, end);
  if (face === "listening") return { key: "chat.status.listening" };
  if (face === "thinking") return { key: "chat.status.thinking" };
  if (face === "speaking") return { key: "chat.status.speaking" };
  if (face === "acting") return { key: "chat.status.acting" };
  if (state === "idle" && end) {
    const reason = describeSystem(`system:end:${end}`);
    const reasonKey =
      reason.key === "chat.system.unknown" ? undefined : reason.key;
    return { key: "chat.status.ended", reasonKey };
  }
  return { key: "chat.status.idle" };
}

function Bubble({ entry, t }: { entry: ChatEntry; t: TFn }) {
  if (entry.role === "system") {
    const note = describeSystem(entry.text);
    const label =
      note.count === undefined
        ? t(note.key)
        : t(note.key, { count: note.count });
    return (
      <li className="chat-bubble chat-system" data-role="system">
        <div className="chat-text" data-testid="chat-text">
          {label}
        </div>
      </li>
    );
  }
  const date = new Date(entry.tsMs);
  const valid = !Number.isNaN(date.getTime());
  const sender = entry.role === "user" ? t("chat.you") : t("chat.vox");
  return (
    <li className={`chat-bubble chat-${entry.role}`} data-role={entry.role}>
      <div className="chat-meta">
        <span className="chat-sender">{sender}</span>
        <time
          className="chat-time"
          dateTime={valid ? date.toISOString() : undefined}
        >
          {valid ? date.toLocaleTimeString() : ""}
        </time>
      </div>
      <div className="chat-text" data-testid="chat-text">
        {entry.text}
      </div>
    </li>
  );
}

export default function ChatWindow() {
  const { t } = useTranslation();
  const [view, setView] = useState<ChatView>(initialView);
  const viewRef = useRef<ChatView>(view);
  const bufferRef = useRef<ChatEvent[]>([]);
  const logRef = useRef<HTMLOListElement>(null);
  const pinnedRef = useRef(true);
  const lastEntryIdRef = useRef<number | undefined>(undefined);
  const [unseen, setUnseen] = useState(false);
  const [clearing, setClearing] = useState(false);
  // Inline two-step Clear (U15a): idle shows Clear; confirming shows a bar below the header with
  // Cancel (focused) and Confirm clear, and auto-cancels after CLEAR_CONFIRM_MS (T1b countdown).
  const [clearStep, setClearStep] = useState<ClearStep>("idle");
  const [clearFailed, setClearFailed] = useState(false);
  const [clearSeconds, setClearSeconds] = useState(
    clearSecondsLeft(CLEAR_CONFIRM_MS),
  );
  const clearDeadlineRef = useRef(0);
  const clearBtnRef = useRef<HTMLButtonElement>(null);
  const cancelBtnRef = useRef<HTMLButtonElement>(null);
  const wasConfirmingRef = useRef(false);
  // Stop (T1b): one call in flight at a time; a failed call shows one notice line.
  const [stopping, setStopping] = useState(false);
  const stoppingRef = useRef(false);
  const [stopFailed, setStopFailed] = useState(false);
  // Typing box. draftRef mirrors draft so handlers always see the latest text; sendingRef is the
  // synchronous in-flight guard (a same-tick second submit must not wait for a React render).
  const [draft, setDraft] = useState("");
  const draftRef = useRef("");
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const sendingRef = useRef(false);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);

  const commit = useCallback((next: ChatView) => {
    viewRef.current = next;
    setView(next);
  }, []);

  // Escape (D1b): cancels the inline Clear confirmation first, otherwise hides the window with one
  // in-flight call at a time. The draft is left alone so it is still there when the window returns.
  const hideGuard = useMemo(
    () => createOnceGuard(() => commands.chatHideWindow()),
    [],
  );

  const onHide = () => {
    hideGuard.trigger();
  };

  useEffect(() => {
    const onKeyDown = (e: globalThis.KeyboardEvent) => {
      const action = escAction(
        { confirming: clearStep === "confirming", hiding: hideGuard.inFlight },
        {
          key: e.key,
          altKey: e.altKey,
          ctrlKey: e.ctrlKey,
          metaKey: e.metaKey,
          shiftKey: e.shiftKey,
          isComposing: e.isComposing,
          keyCode: e.keyCode,
        },
      );
      if (action === "ignore") return;
      e.preventDefault();
      if (action === "cancelConfirm") {
        setClearStep((step) => nextClearStep(step, "cancel"));
      } else {
        hideGuard.trigger();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [clearStep, hideGuard]);

  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];

    const refetch = async (retriesLeft: number): Promise<void> => {
      let snap: ChatSnapshot;
      try {
        snap = await commands.getChatSnapshot();
      } catch {
        return;
      }
      if (disposed) return;
      const result = applySnapshot(viewRef.current, snap, bufferRef.current);
      if (result.stale) {
        if (retriesLeft > 0) await refetch(retriesLeft - 1);
        return;
      }
      bufferRef.current = [];
      commit(result.view);
    };

    const onEvent = (ev: ChatEvent) => {
      // A handler from a disposed effect (StrictMode double-mount, or an unlisten that failed) must not act.
      if (disposed) return;
      const current = viewRef.current;
      if (!current.ready) {
        bufferRef.current.push(ev);
        if (bufferRef.current.length > PRE_HYDRATE_BUFFER_CAP)
          bufferRef.current.shift();
        return;
      }
      if (needsResync(current, ev)) {
        void refetch(1);
        return;
      }
      commit(applyEvent(current, ev));
    };

    const onFocus = () => {
      void refetch(0);
    };

    const start = async () => {
      try {
        const unlisten = await listen<ChatEvent>("converse-chat", (event) =>
          onEvent(event.payload),
        );
        if (disposed) {
          unlisten();
          return;
        }
        cleanups.push(unlisten);
      } catch (error) {
        console.warn("chat: failed to subscribe to converse-chat", error);
      }
      if (disposed) return;
      window.addEventListener("focus", onFocus);
      cleanups.push(() => window.removeEventListener("focus", onFocus));
      await refetch(0);
    };

    void start();
    return () => {
      disposed = true;
      for (const cleanup of cleanups) cleanup();
    };
  }, [commit]);

  // Pin to the newest entry while the reader is at the bottom; otherwise offer "Jump to latest".
  useLayoutEffect(() => {
    const el = logRef.current;
    const entries = view.entries;
    const lastId =
      entries.length > 0 ? entries[entries.length - 1]?.id : undefined;
    const changed = lastId !== lastEntryIdRef.current;
    lastEntryIdRef.current = lastId;
    if (!el || !changed) return;
    if (pinnedRef.current) {
      el.scrollTop = el.scrollHeight;
      setUnseen(false);
    } else if (lastId !== undefined) {
      setUnseen(true);
    }
  }, [view.entries]);

  const onLogScroll = () => {
    const el = logRef.current;
    if (!el) return;
    const atBottom = shouldAutoScroll(el);
    pinnedRef.current = atBottom;
    if (atBottom) setUnseen(false);
  };

  const jumpToLatest = () => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
    pinnedRef.current = true;
    setUnseen(false);
  };

  useEffect(() => {
    if (clearStep !== "confirming") return;
    const timer = window.setTimeout(
      () => setClearStep((step) => nextClearStep(step, "timeout")),
      CLEAR_CONFIRM_MS,
    );
    return () => window.clearTimeout(timer);
  }, [clearStep]);

  // Countdown text on the bar (4 s to 1 s). Reduced motion keeps this text and drops the track.
  useEffect(() => {
    if (clearStep !== "confirming") return;
    const tick = () =>
      setClearSeconds(clearSecondsLeft(clearDeadlineRef.current - Date.now()));
    const interval = window.setInterval(tick, 250);
    return () => window.clearInterval(interval);
  }, [clearStep]);

  // Focus: Cancel (the safe default) takes focus when the bar opens. When the bar closes with focus
  // lost (Cancel, Escape, the timeout or Confirm removed the focused button), focus returns to the
  // Clear button, but only once Clear is enabled again: it is disabled while a clear is in flight,
  // and a disabled button cannot take focus. Until then the close is remembered.
  useEffect(() => {
    if (clearStep === "confirming") {
      wasConfirmingRef.current = true;
      cancelBtnRef.current?.focus();
      return;
    }
    if (!wasConfirmingRef.current || clearing) return;
    wasConfirmingRef.current = false;
    const active = document.activeElement;
    if (active === null || active === document.body) {
      clearBtnRef.current?.focus();
    }
  }, [clearStep, clearing]);

  const onRequestClear = () => {
    if (clearStep === "confirming") return;
    clearDeadlineRef.current = Date.now() + CLEAR_CONFIRM_MS;
    setClearSeconds(clearSecondsLeft(CLEAR_CONFIRM_MS));
    setClearStep((step) => nextClearStep(step, "request"));
  };

  const onStop = async (sessionId: number) => {
    if (stoppingRef.current) return;
    stoppingRef.current = true;
    setStopping(true);
    setStopFailed(false);
    let failed = false;
    try {
      // A false result means the session already ended: silent. The end-of-session state event
      // updates the face and status; nothing is changed locally.
      await commands.chatStopConversation(sessionId);
    } catch {
      failed = true;
    }
    stoppingRef.current = false;
    setStopping(false);
    if (failed) setStopFailed(true);
  };

  const onCancelClear = () => {
    setClearStep((step) => nextClearStep(step, "cancel"));
  };

  const onClear = async () => {
    if (clearing) return;
    setClearStep((step) => nextClearStep(step, "confirm"));
    setClearing(true);
    setClearFailed(false);
    try {
      const result = await commands.clearChatLog();
      if (result.status === "ok") {
        commit({ ...viewRef.current, entries: [] });
      } else {
        setClearFailed(true);
      }
    } catch {
      setClearFailed(true);
    } finally {
      setClearing(false);
    }
  };

  const setDraftValue = (value: string) => {
    draftRef.current = value;
    setDraft(value);
  };

  // Auto-grow up to the CSS max-height (four lines); beyond that the box scrolls.
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight + (el.offsetHeight - el.clientHeight)}px`;
  }, [draft]);

  const onDraftChange = (e: ChangeEvent<HTMLTextAreaElement>) => {
    setDraftValue(e.target.value);
    setSendError(null);
  };

  const submit = async () => {
    if (sendingRef.current) return;
    const submitted = draftRef.current;
    const prepared = prepareSend(submitted);
    if (!prepared.ok) {
      if (prepared.reason === "too_long") setSendError("too_long");
      return;
    }
    sendingRef.current = true;
    setSending(true);
    setSendError(null);
    let failure: string | null = null;
    try {
      const result = await commands.sendChatMessage(prepared.text);
      if (result.status === "error") failure = result.error;
    } catch {
      failure = "failed";
    }
    sendingRef.current = false;
    setSending(false);
    if (failure === null) {
      // Clear only the draft that was sent; text typed while it was in flight stays.
      if (draftRef.current === submitted) setDraftValue("");
      inputRef.current?.focus();
    } else {
      setSendError(failure);
    }
  };

  const onComposerSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    void submit();
  };

  const onDraftKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      !shouldSubmitOnKey({
        key: e.key,
        shiftKey: e.shiftKey,
        altKey: e.altKey,
        isComposing: e.nativeEvent.isComposing,
        keyCode: e.nativeEvent.keyCode,
      })
    ) {
      return;
    }
    e.preventDefault();
    void submit();
  };

  const canSend = !sending && prepareSend(draft).ok;

  const face = avatarFace(view.state, view.end);
  const caption = captionFor(view.state, view.end);
  const stopId = view.activeSession;
  const showStop = stopVisible(face, stopId);
  // The globe's look: orb colour, app accent and the chat surface bucket (A3).
  const orbLook = useSyncExternalStore(
    subscribeChatOrbLook,
    getChatOrbLook,
    getChatOrbLook,
  );

  return (
    <div
      className="chat-root"
      data-face={face}
      style={
        { ...accentStyle(face), ...accentTextStyle(face) } as CSSProperties
      }
    >
      <header className="chat-header" data-tauri-drag-region>
        <div className="chat-brand" data-tauri-drag-region="deep">
          <img
            data-testid="chat-brand-icon"
            src={voxIcon}
            alt=""
            width={32}
            height={32}
            className="chat-brand-icon"
          />
          <div className="chat-brand-text">
            {/* eslint-disable-next-line i18next/no-literal-string -- product name, never translated */}
            <span data-testid="chat-brand-name" className="chat-brand-name">
              Vox
            </span>
            <span data-testid="chat-brand-sub" className="chat-brand-sub">
              {t("chat.subtitle")}
            </span>
          </div>
        </div>
        <div className="chat-header-end">
          {view.recording ? null : (
            <span className="chat-rec-off">{t("chat.recordingOff")}</span>
          )}
          <button
            type="button"
            ref={clearBtnRef}
            className="chat-clear"
            onClick={onRequestClear}
            disabled={clearing}
          >
            {t("chat.clear")}
          </button>
          <button
            type="button"
            className="chat-hide"
            data-testid="chat-hide"
            aria-label={t("chat.hide")}
            title={t("chat.hide")}
            onClick={onHide}
          >
            <svg
              aria-hidden="true"
              focusable="false"
              width="14"
              height="14"
              viewBox="0 0 14 14"
            >
              <path
                d="M3 3l8 8M11 3l-8 8"
                stroke="currentColor"
                strokeWidth="2"
                strokeLinecap="round"
                fill="none"
              />
            </svg>
          </button>
        </div>
      </header>
      {/* Always present: the Clear question is written into this region when the bar opens, so the
          announcement is made by a live region that already exists (not one inserted with its text). */}
      <div
        className="chat-sr-only"
        role="status"
        aria-live="polite"
        data-testid="chat-clear-announce"
      >
        {clearStep === "confirming" ? t("chat.clearPrompt") : ""}
      </div>
      {clearStep === "confirming" ? (
        <div
          className="chat-clear-bar"
          role="group"
          aria-labelledby="chat-clear-question"
        >
          <div className="chat-clear-copy">
            <p id="chat-clear-question" className="chat-clear-question">
              {t("chat.clearPrompt")}
            </p>
            <span className="chat-clear-countdown" aria-hidden="true">
              {t("chat.clearCountdown", { count: clearSeconds })}
            </span>
            <span className="chat-clear-track" aria-hidden="true">
              <span className="chat-clear-fill" />
            </span>
          </div>
          <div className="chat-clear-actions">
            <button
              type="button"
              ref={cancelBtnRef}
              className="chat-clear"
              onClick={onCancelClear}
            >
              {t("chat.clearCancel")}
            </button>
            <button
              type="button"
              className="chat-clear chat-clear-danger"
              onClick={onClear}
              disabled={clearing}
            >
              {t("chat.clearConfirm")}
            </button>
          </div>
        </div>
      ) : null}
      <section className="chat-stage">
        <VoxAvatar face={face} look={orbLook} />
        <div className="chat-eq" aria-hidden="true">
          <span />
          <span />
          <span />
          <span />
          <span />
        </div>
        <div className="chat-status-row">
          <p className="chat-pill" role="status">
            <span className="chat-pill-dot" aria-hidden="true" />
            <span className="chat-pill-text">{t(caption.key)}</span>
            {caption.reasonKey ? (
              <span className="chat-reason">{t(caption.reasonKey)}</span>
            ) : null}
          </p>
          {showStop && stopId !== null ? (
            <button
              type="button"
              className="chat-stop"
              data-testid="chat-stop"
              aria-label={t("chat.stopAria")}
              title={t("chat.stopAria")}
              disabled={stopping}
              onClick={() => void onStop(stopId)}
            >
              <svg
                aria-hidden="true"
                focusable="false"
                width="12"
                height="12"
                viewBox="0 0 12 12"
              >
                <rect
                  x="1"
                  y="1"
                  width="10"
                  height="10"
                  rx="1.5"
                  fill="currentColor"
                />
              </svg>
              <span>{t("chat.stop")}</span>
            </button>
          ) : null}
        </div>
      </section>
      {clearFailed ? (
        <p className="chat-error" role="alert">
          {t("chat.clearFailed")}
        </p>
      ) : null}
      {stopFailed ? (
        <p className="chat-error" role="alert">
          {t("chat.stopFailed")}
        </p>
      ) : null}
      <div className="chat-body">
        <ol
          ref={logRef}
          className="chat-log"
          role="log"
          aria-live="polite"
          aria-relevant="additions"
          onScroll={onLogScroll}
        >
          {view.entries.length === 0 ? (
            <li className="chat-empty">{t("chat.empty")}</li>
          ) : (
            view.entries.map((entry) => (
              <Bubble key={entry.id} entry={entry} t={t} />
            ))
          )}
        </ol>
        {unseen ? (
          <button type="button" className="chat-jump" onClick={jumpToLatest}>
            {t("chat.jumpToLatest")}
          </button>
        ) : null}
      </div>
      <form
        className="chat-composer"
        data-testid="chat-composer"
        onSubmit={onComposerSubmit}
      >
        <div className="chat-composer-row">
          <textarea
            ref={inputRef}
            data-testid="chat-input"
            className="chat-input"
            aria-label={t("chat.input.label")}
            placeholder={t("chat.input.placeholder")}
            maxLength={MAX_CHARS}
            rows={1}
            value={draft}
            onChange={onDraftChange}
            onKeyDown={onDraftKeyDown}
          />
          <button
            type="submit"
            className="chat-send"
            data-testid="chat-send"
            disabled={!canSend}
          >
            {t("chat.send")}
          </button>
        </div>
        {sendError !== null ? (
          <p className="chat-error chat-send-error" role="alert">
            {t(errorKey(sendError))}
          </p>
        ) : null}
        {counterVisible(draft.length) ? (
          <span className="chat-count" data-testid="chat-count">
            {`${draft.length} / ${MAX_CHARS}`}
          </span>
        ) : null}
      </form>
      <footer className="chat-footer">{t("chat.footerHint")}</footer>
    </div>
  );
}
