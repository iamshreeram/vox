// Conversation window body. Protocol (DESIGN-CHAT A4): subscribe to `converse-chat` first,
// buffer until the first snapshot is applied, then apply live events; a gap refetches once,
// a stale snapshot is discarded and refetched at most once more, window focus refetches once.
// The only commands used are get_chat_snapshot and clear_chat_log (A8).
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import VoxAvatar from "./VoxAvatar";
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

function captionFor(state: string, end: string | null): { key: string; reasonKey?: string } {
  const face = avatarFace(state, end);
  if (face === "listening") return { key: "chat.status.listening" };
  if (face === "thinking") return { key: "chat.status.thinking" };
  if (face === "speaking") return { key: "chat.status.speaking" };
  if (face === "acting") return { key: "chat.status.acting" };
  if (state === "idle" && end) {
    const reason = describeSystem(`system:end:${end}`);
    const reasonKey = reason.key === "chat.system.unknown" ? undefined : reason.key;
    return { key: "chat.status.ended", reasonKey };
  }
  return { key: "chat.status.idle" };
}

function Bubble({ entry, t }: { entry: ChatEntry; t: TFn }) {
  const when = new Date(entry.tsMs).toLocaleTimeString();
  if (entry.role === "system") {
    const note = describeSystem(entry.text);
    const label = note.count === undefined ? t(note.key) : t(note.key, { count: note.count });
    return (
      <li className="chat-bubble chat-system" data-role="system" title={when}>
        {label}
      </li>
    );
  }
  return (
    <li className={`chat-bubble chat-${entry.role}`} data-role={entry.role} title={when}>
      <div className="chat-text">{entry.text}</div>
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
      const current = viewRef.current;
      if (!current.ready) {
        bufferRef.current.push(ev);
        if (bufferRef.current.length > PRE_HYDRATE_BUFFER_CAP) bufferRef.current.shift();
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
        const unlisten = await listen<ChatEvent>("converse-chat", (event) => onEvent(event.payload));
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
    const lastId = entries.length > 0 ? entries[entries.length - 1]?.id : undefined;
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

  const face = avatarFace(view.state, view.end);
  const caption = captionFor(view.state, view.end);

  return (
    <div className="chat-root">
      <section className="chat-stage" data-face={face}>
        <VoxAvatar face={face} />
        <p className="chat-caption">
          <span>{t(caption.key)}</span>
          {caption.reasonKey ? <span className="chat-reason">{t(caption.reasonKey)}</span> : null}
        </p>
      </section>
      <header className="chat-header">
        {view.recording ? null : <span className="chat-rec-off">{t("chat.recordingOff")}</span>}
        <button type="button" className="chat-clear" onClick={onClear} disabled={clearing}>
          {t("chat.clear")}
        </button>
      </header>
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
            view.entries.map((entry) => <Bubble key={entry.id} entry={entry} t={t} />)
          )}
        </ol>
        {unseen ? (
          <button type="button" className="chat-jump" onClick={jumpToLatest}>
            {t("chat.jumpToLatest")}
          </button>
        ) : null}
      </div>
      <footer className="chat-footer">{t("chat.footerHint")}</footer>
    </div>
  );
}
