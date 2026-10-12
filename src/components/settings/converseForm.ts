/**
 * Pure rules for the conversation settings section: numeric limits that mirror
 * the backend clamps, parsing of the `converse-state` event payload, i18n key
 * selection, and the start/stop button decision. No React, no Tauri imports.
 */

export type ConverseState =
  | "listening"
  | "thinking"
  | "speaking"
  | "acting"
  | "idle";

export type ConverseEnd =
  | "user_ended"
  | "idle"
  | "cancelled"
  | "not_configured"
  | "audio_unavailable"
  | "speech_stuck"
  | "turn_limit";

export interface ConverseStateEvent {
  state: ConverseState;
  end: ConverseEnd | null;
}

export type ConverseSetting = "silenceMs" | "idleTurns" | "contextTurns";

export interface ConverseLimit {
  min: number;
  max: number;
  step: number;
  default: number;
}

/** Ranges and defaults; these must match the Rust clamps and defaults. */
export const CONVERSE_LIMITS: Record<ConverseSetting, ConverseLimit> = {
  silenceMs: { min: 600, max: 4000, step: 100, default: 1200 },
  idleTurns: { min: 1, max: 5, step: 1, default: 2 },
  contextTurns: { min: 0, max: 12, step: 1, default: 6 },
};

const STATES: readonly ConverseState[] = [
  "listening",
  "thinking",
  "speaking",
  "acting",
  "idle",
];

const ENDS: readonly ConverseEnd[] = [
  "user_ended",
  "idle",
  "cancelled",
  "not_configured",
  "audio_unavailable",
  "speech_stuck",
  "turn_limit",
];

/** Rounds to a whole number and clamps to the setting's range. Non-finite input yields the default. */
export function clampSetting(setting: ConverseSetting, value: number): number {
  const limit = CONVERSE_LIMITS[setting];
  if (!Number.isFinite(value)) return limit.default;
  return Math.min(limit.max, Math.max(limit.min, Math.round(value)));
}

function isState(value: unknown): value is ConverseState {
  return (
    typeof value === "string" && (STATES as readonly string[]).includes(value)
  );
}

function isEnd(value: unknown): value is ConverseEnd {
  return (
    typeof value === "string" && (ENDS as readonly string[]).includes(value)
  );
}

/**
 * Turns an untrusted event payload into a known state. Anything malformed
 * degrades to `idle` with no end reason. An end reason is kept only on idle.
 */
export function parseConverseState(payload: unknown): ConverseStateEvent {
  const fallback: ConverseStateEvent = { state: "idle", end: null };
  if (
    typeof payload !== "object" ||
    payload === null ||
    Array.isArray(payload)
  ) {
    return fallback;
  }
  if (!Object.prototype.hasOwnProperty.call(payload, "state")) return fallback;
  const record = payload as Record<string, unknown>;
  const state = record.state;
  if (!isState(state)) return fallback;
  const end = record.end;
  if (state === "idle" && isEnd(end)) {
    return { state, end };
  }
  return { state, end: null };
}

export function stateLabelKey(state: ConverseState): string {
  return `converse.state.${state}`;
}

export function endLabelKey(end: ConverseEnd | null): string | null {
  return end === null ? null : `converse.end.${end}`;
}

export function isSessionActive(state: ConverseState): boolean {
  return state !== "idle";
}

export type ButtonAction = "start" | "stop" | "disabled";

/** While a request is in flight the button is disabled, whatever the session state. */
export function buttonAction(active: boolean, busy: boolean): ButtonAction {
  if (busy) return "disabled";
  return active ? "stop" : "start";
}
