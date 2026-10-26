// SKELETON (step 7, tests first). Bodies throw until the implementation step.
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

export function initialView(): ChatView {
  throw new Error("not implemented");
}

export function applyEvent(view: ChatView, ev: ChatEvent): ChatView {
  void view;
  void ev;
  throw new Error("not implemented");
}

export function applySnapshot(
  view: ChatView,
  snap: ChatSnapshot,
  buffered: ChatEvent[],
): { view: ChatView; stale: boolean } {
  void view;
  void snap;
  void buffered;
  throw new Error("not implemented");
}

/**
 * True when the event reveals a missed event: the view is hydrated and the event
 * skips past view.seq + 1. Duplicates and older events are never a gap.
 */
export function needsResync(view: ChatView, ev: ChatEvent): boolean {
  void view;
  void ev;
  throw new Error("not implemented");
}

export function shouldAutoScroll(
  metrics: { scrollTop: number; scrollHeight: number; clientHeight: number },
  threshold = 48,
): boolean {
  void metrics;
  void threshold;
  throw new Error("not implemented");
}

export function describeSystem(text: string): { key: string; count?: number } {
  void text;
  throw new Error("not implemented");
}

export function avatarFace(state: string, end: string | null): AvatarFace {
  void state;
  void end;
  throw new Error("not implemented");
}
