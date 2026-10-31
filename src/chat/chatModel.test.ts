import { describe, expect, it } from "vitest";
import {
  VIEW_CAP,
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

function entry(id: number, text = `m${id}`): ChatEntry {
  return { id, tsMs: 1000 + id, role: "user", text };
}

function entryEv(seq: number, id: number, text?: string): ChatEvent {
  return { seq, kind: { type: "entry", entry: entry(id, text) } };
}

function viewWith(seq: number, entries: ChatEntry[]): ChatView {
  return { ...initialView(), seq, entries, ready: true };
}

describe("initialView", () => {
  it("starts empty, idle, recording, not ready, at seq 0", () => {
    expect(initialView()).toEqual({
      seq: 0,
      entries: [],
      state: "idle",
      end: null,
      recording: true,
      ready: false,
      activeSession: null,
    });
  });
});

describe("active session identity (T1b Stop)", () => {
  const stateEv = (
    seq: number,
    state: string,
    end: string | null,
    activeSession?: number | null,
  ): ChatEvent => ({
    seq,
    kind: { type: "state", state, end },
    activeSession,
  });

  it("the snapshot's activeSession is kept in the view", () => {
    const snap: ChatSnapshot = {
      seq: 4,
      entries: [],
      state: "listening",
      end: null,
      recording: true,
      activeSession: 7,
    };
    const { view } = applySnapshot(initialView(), snap, []);
    expect(view.activeSession).toBe(7);
  });

  it("a snapshot without the field reads as no session", () => {
    const snap = {
      seq: 1,
      entries: [],
      state: "idle",
      end: null,
      recording: true,
    } as ChatSnapshot;
    expect(applySnapshot(initialView(), snap, []).view.activeSession).toBe(
      null,
    );
  });

  it("a state event sets the session it carries", () => {
    const view = applyEvent(viewWith(1, []), stateEv(2, "speaking", null, 9));
    expect(view.activeSession).toBe(9);
    expect(view.state).toBe("speaking");
  });

  it("an idle state event with a null session drops it", () => {
    const busy = applyEvent(viewWith(1, []), stateEv(2, "acting", null, 9));
    const idle = applyEvent(busy, stateEv(3, "idle", "user_ended", null));
    expect(idle.activeSession).toBeNull();
    expect(idle.end).toBe("user_ended");
  });

  it("a state event without the field drops the session", () => {
    const busy = applyEvent(viewWith(1, []), stateEv(2, "acting", null, 9));
    const next = applyEvent(busy, stateEv(3, "listening", null));
    expect(next.activeSession).toBeNull();
  });

  it("entry and config events keep the session they arrive under", () => {
    const busy = applyEvent(viewWith(1, []), stateEv(2, "thinking", null, 5));
    const withEntry = applyEvent(busy, entryEv(3, 1));
    const withConfig = applyEvent(withEntry, {
      seq: 4,
      kind: { type: "config", recording: false },
    });
    expect(withConfig.activeSession).toBe(5);
  });

  it("a stale state event cannot change the session", () => {
    const busy = applyEvent(viewWith(1, []), stateEv(5, "speaking", null, 8));
    const stale = applyEvent(busy, stateEv(4, "idle", null, null));
    expect(stale.activeSession).toBe(8);
  });
});

describe("applyEvent", () => {
  it("appends entries in arrival order and advances seq", () => {
    let view = initialView();
    view = applyEvent(view, entryEv(1, 1, "a"));
    view = applyEvent(view, entryEv(2, 2, "b"));
    view = applyEvent(view, entryEv(3, 3, "c"));
    expect(view.entries.map((e) => e.text)).toEqual(["a", "b", "c"]);
    expect(view.seq).toBe(3);
  });

  it("does not mutate the view it receives", () => {
    const before = initialView();
    const snapshotOfBefore = JSON.stringify(before);
    applyEvent(before, entryEv(1, 1));
    expect(JSON.stringify(before)).toBe(snapshotOfBefore);
  });

  it("ignores events whose seq is at or below the view seq (stale and duplicate)", () => {
    const view = viewWith(5, [entry(1)]);
    expect(applyEvent(view, entryEv(5, 9, "dup"))).toBe(view);
    expect(applyEvent(view, entryEv(4, 8, "old"))).toBe(view);
    expect(applyEvent(view, { seq: 2, kind: { type: "clear" } })).toBe(view);
  });

  it("keeps only the newest VIEW_CAP entries when the cap is exceeded", () => {
    expect(VIEW_CAP).toBe(500);
    let view = initialView();
    for (let i = 1; i <= VIEW_CAP + 1; i += 1) {
      view = applyEvent(view, entryEv(i, i));
    }
    expect(view.entries).toHaveLength(500);
    expect(view.entries[0]?.id).toBe(2);
    expect(view.entries[499]?.id).toBe(VIEW_CAP + 1);
  });

  it("reset empties the entries", () => {
    const view = viewWith(3, [entry(1), entry(2)]);
    const after = applyEvent(view, { seq: 4, kind: { type: "reset" } });
    expect(after.entries).toEqual([]);
    expect(after.seq).toBe(4);
  });

  it("clear empties the entries", () => {
    const view = viewWith(3, [entry(1), entry(2)]);
    const after = applyEvent(view, { seq: 4, kind: { type: "clear" } });
    expect(after.entries).toEqual([]);
    expect(after.seq).toBe(4);
  });

  it("state event updates state and end", () => {
    const after = applyEvent(initialView(), {
      seq: 1,
      kind: { type: "state", state: "thinking", end: null },
    });
    expect(after.state).toBe("thinking");
    expect(after.end).toBeNull();
    const ended = applyEvent(after, {
      seq: 2,
      kind: { type: "state", state: "idle", end: "user_ended" },
    });
    expect(ended.state).toBe("idle");
    expect(ended.end).toBe("user_ended");
  });

  it("config event updates the recording flag", () => {
    const off = applyEvent(initialView(), {
      seq: 1,
      kind: { type: "config", recording: false },
    });
    expect(off.recording).toBe(false);
    expect(
      applyEvent(off, { seq: 2, kind: { type: "config", recording: true } })
        .recording,
    ).toBe(true);
  });
});

describe("needsResync", () => {
  it("is false when the event is the next one in sequence", () => {
    expect(needsResync(viewWith(5, []), entryEv(6, 1))).toBe(false);
  });

  it("is true when exactly one event is missing (gap of 1)", () => {
    expect(needsResync(viewWith(5, []), entryEv(7, 1))).toBe(true);
  });

  it("is true for a larger gap", () => {
    expect(needsResync(viewWith(5, []), entryEv(40, 1))).toBe(true);
  });

  it("is false before the view is hydrated, even with a gap", () => {
    const notReady = { ...viewWith(5, []), ready: false };
    expect(needsResync(notReady, entryEv(9, 1))).toBe(false);
  });

  it("is false for a duplicate event (same seq)", () => {
    expect(needsResync(viewWith(5, []), entryEv(5, 1))).toBe(false);
  });

  it("is false for an older event", () => {
    expect(needsResync(viewWith(5, []), entryEv(3, 1))).toBe(false);
  });
});

describe("applySnapshot", () => {
  const snap: ChatSnapshot = {
    seq: 10,
    entries: [entry(1, "a"), entry(2, "b")],
    state: "listening",
    end: null,
    recording: true,
  };

  it("replaces the view from the snapshot, marks it ready, and applies only buffered events with seq > snapshot seq in ascending order", () => {
    const buffered: ChatEvent[] = [
      entryEv(12, 4, "d"),
      entryEv(10, 3, "c-same-as-snap"),
      entryEv(11, 3, "c"),
      entryEv(9, 0, "old"),
    ];
    const result = applySnapshot(initialView(), snap, buffered);
    expect(result.stale).toBe(false);
    expect(result.view.ready).toBe(true);
    expect(result.view.seq).toBe(12);
    expect(result.view.state).toBe("listening");
    expect(result.view.entries.map((e) => e.text)).toEqual([
      "a",
      "b",
      "c",
      "d",
    ]);
  });

  it("applies buffered events in ascending seq even when they arrive out of order", () => {
    const buffered: ChatEvent[] = [
      entryEv(13, 5, "third"),
      entryEv(11, 3, "first"),
      entryEv(12, 4, "second"),
    ];
    const result = applySnapshot(initialView(), snap, buffered);
    expect(result.view.entries.map((e) => e.text)).toEqual([
      "a",
      "b",
      "first",
      "second",
      "third",
    ]);
  });

  it("rejects a stale snapshot that is older than the view after a clear event; view is returned unchanged", () => {
    // Events already applied up to seq 12 (including a clear), then a snapshot
    // taken at seq 9 arrives late. Its entries must not resurrect.
    let view = viewWith(9, []);
    view = applyEvent(view, entryEv(10, 1, "x"));
    view = applyEvent(view, entryEv(11, 2, "y"));
    view = applyEvent(view, { seq: 12, kind: { type: "clear" } });
    const staleSnap: ChatSnapshot = {
      seq: 9,
      entries: [entry(1, "x"), entry(2, "y")],
      state: "idle",
      end: null,
      recording: true,
    };
    const result = applySnapshot(view, staleSnap, []);
    expect(result.stale).toBe(true);
    expect(result.view).toBe(view);
    expect(result.view.entries).toEqual([]);
    expect(result.view.seq).toBe(12);
  });

  it("accepts a snapshot whose seq equals the view seq", () => {
    const view = viewWith(10, [entry(9, "old")]);
    const result = applySnapshot(view, snap, []);
    expect(result.stale).toBe(false);
    expect(result.view.entries.map((e) => e.text)).toEqual(["a", "b"]);
    expect(result.view.seq).toBe(10);
  });
});

describe("shouldAutoScroll", () => {
  // distance from bottom = scrollHeight - scrollTop - clientHeight
  it("is true exactly at the threshold distance from the bottom", () => {
    expect(
      shouldAutoScroll({
        scrollTop: 452,
        scrollHeight: 1000,
        clientHeight: 500,
      }),
    ).toBe(true);
  });

  it("is false one pixel beyond the threshold", () => {
    expect(
      shouldAutoScroll({
        scrollTop: 451,
        scrollHeight: 1000,
        clientHeight: 500,
      }),
    ).toBe(false);
  });

  it("is true when scrolled fully to the bottom", () => {
    expect(
      shouldAutoScroll({
        scrollTop: 500,
        scrollHeight: 1000,
        clientHeight: 500,
      }),
    ).toBe(true);
  });

  it("honours an explicit threshold", () => {
    const m = { scrollTop: 400, scrollHeight: 1000, clientHeight: 500 };
    expect(shouldAutoScroll(m, 100)).toBe(true);
    expect(shouldAutoScroll(m, 99)).toBe(false);
  });
});

describe("describeSystem", () => {
  it.each([
    ["system:started", { key: "chat.system.started" }],
    ["system:dropped:3", { key: "chat.system.dropped", count: 3 }],
    ["system:dropped:abc", { key: "chat.system.unknown" }],
    ["system:dropped:", { key: "chat.system.unknown" }],
    ["system:end:user_ended", { key: "chat.system.end.user_ended" }],
    [
      "system:end:audio_unavailable",
      { key: "chat.system.end.audio_unavailable" },
    ],
    ["system:end:", { key: "chat.system.unknown" }],
    ["system:bogus", { key: "chat.system.unknown" }],
    ["hello there", { key: "chat.system.unknown" }],
    ["", { key: "chat.system.unknown" }],
  ])("maps %j", (text, expected) => {
    expect(describeSystem(text)).toEqual(expected);
  });
});

describe("avatarFace", () => {
  it.each([
    ["listening", null, "listening"],
    ["thinking", null, "thinking"],
    ["speaking", null, "speaking"],
    ["acting", null, "acting"],
    ["listening", "user_ended", "listening"],
    ["idle", null, "idle"],
    ["idle", "user_ended", "sleep"],
    ["idle", "idle", "sleep"],
    ["idle", "cancelled", "sleep"],
    ["idle", "turn_limit", "sleep"],
    ["idle", "not_configured", "concerned"],
    ["idle", "audio_unavailable", "concerned"],
    ["idle", "speech_stuck", "concerned"],
    ["unknown_state", null, "idle"],
    ["unknown_state", "user_ended", "idle"],
  ])("state=%j end=%j -> %j", (state, end, face) => {
    expect(avatarFace(state, end)).toBe(face);
  });
});
