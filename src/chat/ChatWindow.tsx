// Conversation window body. Protocol (DESIGN-CHAT A4): subscribe to `converse-chat` first,
// buffer until the first snapshot is applied, then apply live events; a gap refetches once,
// a stale snapshot is discarded and refetched at most once more, window focus refetches once.
// The only commands used are get_chat_snapshot, clear_chat_log and send_chat_message (DESIGN-TYPE).
// The typing box sends a message and clears itself only on success; the transcript line arrives
// through the same converse-chat events as a spoken one (no local echo).
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
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
  const [clearFailed, setClearFailed] = useState(false);
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

  const onClear = async () => {
    if (clearing) return;
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

  return (
    <div
      className="chat-root"
      data-face={face}
      style={
        { ...accentStyle(face), ...accentTextStyle(face) } as CSSProperties
      }
    >
      <header className="chat-header">
        <div className="chat-brand">
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
            className="chat-clear"
            onClick={onClear}
            disabled={clearing}
          >
            {t("chat.clear")}
          </button>
        </div>
      </header>
      <section className="chat-stage">
        <VoxAvatar face={face} />
        <div className="chat-eq" aria-hidden="true">
          <span />
          <span />
          <span />
          <span />
          <span />
        </div>
        <p className="chat-pill">
          <span className="chat-pill-dot" aria-hidden="true" />
          <span className="chat-pill-text">{t(caption.key)}</span>
          {caption.reasonKey ? (
            <span className="chat-reason">{t(caption.reasonKey)}</span>
          ) : null}
        </p>
      </section>
      {clearFailed ? (
        <p className="chat-error" role="alert">
          {t("chat.clearFailed")}
        </p>
      ) : null}
      <div className="chat-body">
        <ol
          ref={logRef}
          className="chat-log"
          role="log"
          aria-live="polite"
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
