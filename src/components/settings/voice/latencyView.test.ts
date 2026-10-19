import { describe, expect, it } from "vitest";
import {
  latencyView,
  parseLatencyEvent,
  type LatencyView,
} from "./latencyView";

const payload = {
  report: {
    turn: 3,
    endpoint_wait_ms: 812,
    transcribe_ms: 340.4,
    verify_ms: null,
    brain_ms: 1500,
    to_speech_ms: 2100.6,
    total_ms: 4000,
    mode: "smart_turn",
  },
  stats: {
    endpoint_wait: { median: 800, p90: 900 },
    transcribe: { median: 330, p90: null },
    verify: { median: null, p90: null },
    brain: { median: 1400, p90: 1800 },
    to_speech: { median: 2000, p90: 2500 },
    total: { median: 3900, p90: 4500 },
    samples: 5,
  },
};

describe("parseLatencyEvent", () => {
  it("rejects anything that is not the event shape", () => {
    expect(parseLatencyEvent(null)).toBeNull();
    expect(parseLatencyEvent("text")).toBeNull();
    expect(parseLatencyEvent([])).toBeNull();
    expect(parseLatencyEvent({})).toBeNull();
    expect(parseLatencyEvent({ report: payload.report })).toBeNull();
    expect(
      parseLatencyEvent({
        ...payload,
        report: { ...payload.report, turn: "3" },
      }),
    ).toBeNull();
  });

  it("reads the last turn and the stats, keeping missing values as null", () => {
    const parsed = parseLatencyEvent(payload);
    expect(parsed).not.toBeNull();
    expect(parsed?.turn).toBe(3);
    expect(parsed?.samples).toBe(5);
    expect(parsed?.last.waitForSpeech).toBe(812);
    expect(parsed?.last.speakerCheck).toBeNull();
    expect(parsed?.stats.transcription.p90).toBeNull();
  });

  it("ignores non-numeric field values instead of rendering them", () => {
    const parsed = parseLatencyEvent({
      ...payload,
      report: { ...payload.report, brain_ms: "fast" },
    });
    expect(parsed?.last.thinking).toBeNull();
  });
});

describe("latencyView", () => {
  it("shows a placeholder before the first turn", () => {
    expect(latencyView(null)).toEqual({ kind: "placeholder" });
  });

  it("rounds to whole milliseconds", () => {
    const view = latencyView(parseLatencyEvent(payload)) as Extract<
      LatencyView,
      { kind: "turn" }
    >;
    const transcribe = view.last.find((line) => line.key === "transcription");
    const toSpeech = view.last.find((line) => line.key === "toFirstSpeech");
    expect(transcribe?.ms).toBe(340);
    expect(toSpeech?.ms).toBe(2101);
  });

  it("omits missing last-turn fields", () => {
    const view = latencyView(parseLatencyEvent(payload));
    if (view.kind !== "turn") throw new Error("expected a turn view");
    const keys = view.last.map((line) => line.key);
    expect(keys).not.toContain("speakerCheck");
    expect(keys).toEqual([
      "waitForSpeech",
      "transcription",
      "thinking",
      "toFirstSpeech",
    ]);
  });

  it("omits a stat row when both its median and p90 are missing", () => {
    const view = latencyView(parseLatencyEvent(payload));
    if (view.kind !== "turn") throw new Error("expected a turn view");
    expect(view.stats.map((line) => line.key)).not.toContain("speakerCheck");
    const transcribe = view.stats.find((line) => line.key === "transcription");
    expect(transcribe).toEqual({
      key: "transcription",
      median: 330,
      p90: null,
    });
  });

  it("renders only numbers in every value it returns", () => {
    const view = latencyView(parseLatencyEvent(payload));
    if (view.kind !== "turn") throw new Error("expected a turn view");
    for (const line of view.last) {
      expect(typeof line.ms).toBe("number");
    }
    for (const line of view.stats) {
      for (const value of [line.median, line.p90]) {
        expect(value === null || typeof value === "number").toBe(true);
      }
    }
    expect(typeof view.turn).toBe("number");
    expect(typeof view.samples).toBe("number");
  });
});
