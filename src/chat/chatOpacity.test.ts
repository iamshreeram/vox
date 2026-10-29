// Track T spec (DESIGN Revision 3, A-T4 page side). The chat page is listen-only: it reads the
// opacity from its URL (?opacity=&rev=) and applies chat-opacity events only when their rev is
// strictly higher than the one it has seen. Workers must not edit this file.
import { describe, expect, it } from "vitest";
import {
  CHAT_OPACITY_DEFAULT,
  CHAT_OPACITY_MAX,
  CHAT_OPACITY_MIN,
  applyOpacityEvent,
  initialOpacity,
  parseOpacityEvent,
  parseOpacityPct,
} from "./chatOpacity";

describe("chat opacity constants", () => {
  it("is 20..100 with a default of 70", () => {
    expect(CHAT_OPACITY_MIN).toBe(20);
    expect(CHAT_OPACITY_MAX).toBe(100);
    expect(CHAT_OPACITY_DEFAULT).toBe(70);
  });
});

describe("parseOpacityPct (URL and init value: integer 20..100, else 70)", () => {
  it("accepts integers in range", () => {
    expect(parseOpacityPct("20")).toBe(20);
    expect(parseOpacityPct("70")).toBe(70);
    expect(parseOpacityPct("100")).toBe(100);
  });
  it("falls back to 70 for everything else", () => {
    for (const raw of [null, "", "abc", "19", "101", "20.5", "-3", "NaN"]) {
      expect(parseOpacityPct(raw), String(raw)).toBe(70);
    }
  });
});

describe("initialOpacity reads ?opacity and ?rev", () => {
  it("reads both values from the query", () => {
    expect(initialOpacity("?opacity=20&rev=1")).toEqual({ rev: 1, pct: 20 });
  });
  it("uses 70 and rev 0 when absent or invalid", () => {
    expect(initialOpacity("")).toEqual({ rev: 0, pct: 70 });
    expect(initialOpacity("?opacity=bad&rev=x")).toEqual({ rev: 0, pct: 70 });
  });
});

describe("parseOpacityEvent validates the payload shape", () => {
  it("accepts { rev, pct } with a positive integer rev and pct 20..100", () => {
    expect(parseOpacityEvent({ rev: 2, pct: 20 })).toEqual({ rev: 2, pct: 20 });
  });
  it("ignores anything else", () => {
    const bad: unknown[] = [
      null,
      undefined,
      "x",
      { pct: 20 },
      { rev: 2 },
      { rev: "2", pct: 20 },
      { rev: 2.5, pct: 20 },
      { rev: -1, pct: 20 },
      { rev: 2, pct: 19 },
      { rev: 2, pct: 101 },
      { rev: 2, pct: "20" },
      { rev: 2, pct: 20.5 },
    ];
    for (const payload of bad) {
      expect(parseOpacityEvent(payload), JSON.stringify(payload)).toBeNull();
    }
  });
});

describe("applyOpacityEvent applies only strictly higher revs", () => {
  it("updates on a higher rev", () => {
    const state = { rev: 1, pct: 20 };
    expect(applyOpacityEvent(state, { rev: 2, pct: 70 })).toEqual({
      rev: 2,
      pct: 70,
    });
  });
  it("ignores a stale rev and an equal rev with a different value", () => {
    const state = { rev: 3, pct: 50 };
    expect(applyOpacityEvent(state, { rev: 2, pct: 90 })).toBe(state);
    expect(applyOpacityEvent(state, { rev: 3, pct: 90 })).toBe(state);
  });
  it("ignores invalid payloads", () => {
    const state = { rev: 1, pct: 20 };
    expect(applyOpacityEvent(state, { rev: 9, pct: 5 })).toBe(state);
    expect(applyOpacityEvent(state, "junk")).toBe(state);
  });
  it("converges when a late replay carries the latest commit", () => {
    // The window was built at rev 1 / 70; rev 2 / 20 was committed before this page listened.
    let state = initialOpacity("?opacity=70&rev=1");
    state = applyOpacityEvent(state, { rev: 2, pct: 20 });
    expect(state).toEqual({ rev: 2, pct: 20 });
    // a replay of the same snapshot changes nothing
    expect(applyOpacityEvent(state, { rev: 2, pct: 20 })).toBe(state);
  });
});
