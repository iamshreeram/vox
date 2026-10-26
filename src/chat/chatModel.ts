// Pure chat-window model: protocol reducers, scroll rule, system-note mapping and avatar face.
// Contract: .ram-status/chat-window-20261026-093119/DESIGN-CHAT.md rev 6 (A4, A8, A14, 2.4).

export type ChatRole = "user" | "assistant" | "system";

export interface ChatEntry {
  id: number;
  tsMs: number;
  role: ChatRole;
  text: string;
}

/** Wire shape from Rust: `{ seq, kind: { type, ... } }` (kind is internally tagged by `type`). */
export type ChatEventKind =
  | { type: "entry"; entry: ChatEntry }
  | { type: "reset" }
  | { type: "clear" }
  | { type: "state"; state: string; end: string | null }
  | { type: "config"; recording: boolean };

export interface ChatEvent {
  seq: number;
  kind: ChatEventKind;
}

export interface ChatSnapshot {
  seq: number;
  entries: ChatEntry[];
  state: string;
  end: string | null;
  recording: boolean;
}

export interface ChatView {
  seq: number;
  entries: ChatEntry[];
  state: string;
  end: string | null;
  recording: boolean;
  ready: boolean;
}

export const VIEW_CAP = 500;

export type AvatarFace =
  | "idle"
  | "listening"
  | "thinking"
  | "acting"
  | "speaking"
  | "sleep"
  | "concerned";

/** Session end labels from converse_runtime.rs `end_label` (source of truth). */
const KNOWN_END_LABELS = new Set([
  "user_ended",
  "idle",
  "cancelled",
  "turn_limit",
  "not_configured",
  "audio_unavailable",
  "speech_stuck",
]);

/** Ends that mean something went wrong: the avatar looks concerned instead of asleep. */
const ERROR_ENDS = new Set(["not_configured", "audio_unavailable", "speech_stuck"]);

/** States that drive their own face; any other state is shown as idle. */
const ACTIVE_STATES = new Set<AvatarFace>(["listening", "thinking", "speaking", "acting"]);

export function initialView(): ChatView {
  return {
    seq: 0,
    entries: [],
    state: "idle",
    end: null,
    recording: true,
    ready: false,
  };
}

function capEntries(entries: ChatEntry[]): ChatEntry[] {
  return entries.length > VIEW_CAP ? entries.slice(entries.length - VIEW_CAP) : entries;
}

export function applyEvent(view: ChatView, ev: ChatEvent): ChatView {
  if (ev.seq <= view.seq) return view;
  const kind = ev.kind;
  switch (kind.type) {
    case "entry":
      return {
        ...view,
        seq: ev.seq,
        entries: capEntries([...view.entries, kind.entry]),
      };
    case "reset":
    case "clear":
      return { ...view, seq: ev.seq, entries: [] };
    case "state":
      return { ...view, seq: ev.seq, state: kind.state, end: kind.end };
    case "config":
      return { ...view, seq: ev.seq, recording: kind.recording };
  }
}

export function applySnapshot(
  view: ChatView,
  snap: ChatSnapshot,
  buffered: ChatEvent[],
): { view: ChatView; stale: boolean } {
  if (snap.seq < view.seq) return { view, stale: true };
  let next: ChatView = {
    seq: snap.seq,
    entries: capEntries([...snap.entries]),
    state: snap.state,
    end: snap.end,
    recording: snap.recording,
    ready: true,
  };
  const ordered = buffered
    .filter((ev) => ev.seq > snap.seq)
    .sort((a, b) => a.seq - b.seq);
  for (const ev of ordered) {
    next = applyEvent(next, ev);
  }
  return { view: next, stale: false };
}

/**
 * True when the event reveals a missed event: the view is hydrated and the event
 * skips past view.seq + 1. Duplicates and older events are never a gap.
 */
export function needsResync(view: ChatView, ev: ChatEvent): boolean {
  return view.ready && ev.seq > view.seq + 1;
}

export function shouldAutoScroll(
  metrics: { scrollTop: number; scrollHeight: number; clientHeight: number },
  threshold = 48,
): boolean {
  const distanceFromBottom = metrics.scrollHeight - metrics.scrollTop - metrics.clientHeight;
  return distanceFromBottom <= threshold;
}

/** Maps a system-note text marker (`system:started`, `system:dropped:N`, `system:end:<label>`) to an i18n key. */
export function describeSystem(text: string): { key: string; count?: number } {
  if (text === "system:started") return { key: "chat.system.started" };
  const dropped = /^system:dropped:(\d+)$/.exec(text);
  if (dropped) return { key: "chat.system.dropped", count: Number(dropped[1]) };
  const end = /^system:end:([a-z_]+)$/.exec(text);
  if (end && KNOWN_END_LABELS.has(end[1] ?? "")) {
    return { key: `chat.system.end.${end[1]}` };
  }
  return { key: "chat.system.unknown" };
}

export function avatarFace(state: string, end: string | null): AvatarFace {
  if (ACTIVE_STATES.has(state as AvatarFace)) return state as AvatarFace;
  if (state !== "idle") return "idle";
  if (!end) return "idle";
  return ERROR_ENDS.has(end) ? "concerned" : "sleep";
}
