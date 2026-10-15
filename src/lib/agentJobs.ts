// Pure helpers for the Experimental "Coding jobs" settings section (design D9).
// The Rust side is authoritative for job state; these helpers only format and
// classify values for the settings UI.

/** Default timeout in minutes used when the input is not a finite number. */
export const DEFAULT_TIMEOUT_MINUTES = 30;

/** Smallest timeout the UI accepts, in minutes. */
export const MIN_TIMEOUT_MINUTES = 1;

/** Largest timeout the UI accepts, in minutes. */
export const MAX_TIMEOUT_MINUTES = 120;

/** Clamps a timeout to 1..120 minutes after flooring. NaN or non-finite gives 30. */
export function clampTimeoutMinutes(minutes: number): number {
  if (!Number.isFinite(minutes)) return DEFAULT_TIMEOUT_MINUTES;
  const whole = Math.floor(minutes);
  return Math.min(MAX_TIMEOUT_MINUTES, Math.max(MIN_TIMEOUT_MINUTES, whole));
}

/** Formats elapsed seconds as "0s", "59s", "1m 05s" or "1h 02m". */
export function formatElapsed(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "0s";
  const total = Math.floor(seconds);
  if (total < 60) return `${total}s`;
  if (total < 3600) {
    const minutes = Math.floor(total / 60);
    const rest = total % 60;
    return `${minutes}m ${String(rest).padStart(2, "0")}s`;
  }
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  return `${hours}h ${String(minutes).padStart(2, "0")}m`;
}

const IDLE_LABEL_KEY = "settings.experimental.jobs.state.idle";

const STATE_LABEL_KEYS = new Map<string, string>([
  ["idle", IDLE_LABEL_KEY],
  ["running", "settings.experimental.jobs.state.running"],
  ["succeeded", "settings.experimental.jobs.state.succeeded"],
  ["failed", "settings.experimental.jobs.state.failed"],
  ["cancelled", "settings.experimental.jobs.state.cancelled"],
  ["timed_out", "settings.experimental.jobs.state.timedOut"],
  ["spawn_failed", "settings.experimental.jobs.state.couldntStart"],
]);

/** Maps a job state name from the backend to its translation key. Unknown states give the idle key. */
export function stateLabelKey(state: string): string {
  return STATE_LABEL_KEYS.get(state) ?? IDLE_LABEL_KEY;
}

/** True only when the job state is "running". */
export function isRunning(state: string): boolean {
  return state === "running";
}

/** Maximum number of agent names kept in the allowlist (mirrors the backend). */
export const MAX_AGENT_NAMES = 20;

const AGENT_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const AGENT_FLAG_PATTERN = /^--?[A-Za-z][A-Za-z0-9-]{0,31}$/;

/** Returns true when the name matches the agent name rule (ASCII only, no trimming). */
export function isValidAgentName(name: string): boolean {
  return AGENT_NAME_PATTERN.test(name);
}

/** Drops invalid and case-insensitive duplicate names, keeps order and the first spelling, caps at MAX_AGENT_NAMES. */
export function cleanAgentNames(names: string[]): string[] {
  const kept: string[] = [];
  const seen = new Set<string>();
  for (const name of names) {
    if (kept.length >= MAX_AGENT_NAMES) break;
    if (!isValidAgentName(name)) continue;
    const key = name.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    kept.push(name);
  }
  return kept;
}

/** Returns true when the flag matches the agent flag rule and is not the prompt flag. */
export function isValidAgentFlag(flag: string, promptFlag: string): boolean {
  return AGENT_FLAG_PATTERN.test(flag) && flag !== promptFlag;
}

/** Returns true when the candidate is a valid, non-duplicate name and the list is below MAX_AGENT_NAMES. */
export function canAddAgentName(
  existing: string[],
  candidate: string,
): boolean {
  if (existing.length >= MAX_AGENT_NAMES) return false;
  if (!isValidAgentName(candidate)) return false;
  const key = candidate.toLowerCase();
  return !existing.some((name) => name.toLowerCase() === key);
}

/** Returns the result text for the result panel, or null when it is empty or whitespace-only. Text is not trimmed or truncated. */
export function resultPanelText(
  result: string | null | undefined,
): string | null {
  if (result === null || result === undefined) return null;
  return result.trim() === "" ? null : result;
}
