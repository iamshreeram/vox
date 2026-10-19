/**
 * Pure rules for the live latency panel. No React, no Tauri.
 * Spec = latencyView.test.ts. The payload is untrusted: anything malformed
 * parses to null, and only finite, non-negative numbers are ever returned.
 */

export type LatencyFieldKey =
  | "waitForSpeech"
  | "transcription"
  | "thinking"
  | "toFirstSpeech"
  | "speakerCheck";

/** Display order of the stages. */
export const LATENCY_FIELDS: readonly LatencyFieldKey[] = [
  "waitForSpeech",
  "transcription",
  "thinking",
  "toFirstSpeech",
  "speakerCheck",
];

/** A parsed `converse-latency` payload. Missing values are null. */
export interface LatencyEventView {
  turn: number;
  samples: number;
  last: Record<LatencyFieldKey, number | null>;
  stats: Record<LatencyFieldKey, { median: number | null; p90: number | null }>;
}

export interface LatencyLine {
  key: LatencyFieldKey;
  ms: number;
}

export interface LatencyStatLine {
  key: LatencyFieldKey;
  median: number | null;
  p90: number | null;
}

export type LatencyView =
  | { kind: "placeholder" }
  | {
      kind: "turn";
      turn: number;
      samples: number;
      last: LatencyLine[];
      stats: LatencyStatLine[];
    };

const REPORT_FIELDS: Record<string, LatencyFieldKey> = {
  endpoint_wait_ms: "waitForSpeech",
  transcribe_ms: "transcription",
  brain_ms: "thinking",
  to_speech_ms: "toFirstSpeech",
  verify_ms: "speakerCheck",
};

const STATS_FIELDS: Record<string, LatencyFieldKey> = {
  endpoint_wait: "waitForSpeech",
  transcribe: "transcription",
  brain: "thinking",
  to_speech: "toFirstSpeech",
  verify: "speakerCheck",
};

type Rec = Record<string, unknown>;

function isRecord(value: unknown): value is Rec {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A finite, non-negative number, rounded to whole milliseconds; anything else is null. */
function wholeMs(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    return null;
  }
  return Math.round(value);
}

function emptyLast(): Record<LatencyFieldKey, number | null> {
  return {
    waitForSpeech: null,
    transcription: null,
    thinking: null,
    toFirstSpeech: null,
    speakerCheck: null,
  };
}

function emptyStats(): LatencyEventView["stats"] {
  const none = { median: null, p90: null };
  return {
    waitForSpeech: { ...none },
    transcription: { ...none },
    thinking: { ...none },
    toFirstSpeech: { ...none },
    speakerCheck: { ...none },
  };
}

/** Turns an untrusted `converse-latency` payload into a view, or null when the shape is wrong. */
export function parseLatencyEvent(payload: unknown): LatencyEventView | null {
  if (!isRecord(payload)) return null;
  const report = payload.report;
  const stats = payload.stats;
  if (!isRecord(report) || !isRecord(stats)) return null;
  const turn = report.turn;
  const samples = stats.samples;
  if (typeof turn !== "number" || !Number.isFinite(turn)) return null;
  if (typeof samples !== "number" || !Number.isFinite(samples)) return null;

  const last = emptyLast();
  for (const [field, key] of Object.entries(REPORT_FIELDS)) {
    last[key] = wholeMs(report[field]);
  }

  const parsedStats = emptyStats();
  for (const [field, key] of Object.entries(STATS_FIELDS)) {
    const entry = stats[field];
    if (!isRecord(entry)) continue;
    parsedStats[key] = {
      median: wholeMs(entry.median),
      p90: wholeMs(entry.p90),
    };
  }

  return { turn, samples, last, stats: parsedStats };
}

/** The panel's view. Before the first turn it shows a placeholder, never a number. */
export function latencyView(event: LatencyEventView | null): LatencyView {
  if (event === null) return { kind: "placeholder" };

  const last: LatencyLine[] = [];
  const stats: LatencyStatLine[] = [];
  for (const key of LATENCY_FIELDS) {
    const ms = event.last[key];
    if (ms !== null) last.push({ key, ms });
    const { median, p90 } = event.stats[key];
    if (median !== null || p90 !== null) {
      stats.push({ key, median, p90 });
    }
  }
  return {
    kind: "turn",
    turn: event.turn,
    samples: event.samples,
    last,
    stats,
  };
}
