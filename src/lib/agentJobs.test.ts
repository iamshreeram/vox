import assert from "node:assert/strict";
import {
  clampTimeoutMinutes,
  formatElapsed,
  isRunning,
  stateLabelKey,
} from "./agentJobs";

// clampTimeoutMinutes: non-finite input falls back to the default of 30.
assert.equal(clampTimeoutMinutes(Number.NaN), 30);
assert.equal(clampTimeoutMinutes(Number.POSITIVE_INFINITY), 30);
assert.equal(clampTimeoutMinutes(Number.NEGATIVE_INFINITY), 30);

// Boundaries and clamping to 1..120.
assert.equal(clampTimeoutMinutes(0), 1);
assert.equal(clampTimeoutMinutes(1), 1);
assert.equal(clampTimeoutMinutes(120), 120);
assert.equal(clampTimeoutMinutes(121), 120);
assert.equal(clampTimeoutMinutes(-5), 1);
assert.equal(clampTimeoutMinutes(1000000), 120);

// Fractions are floored first, then clamped.
assert.equal(clampTimeoutMinutes(12.9), 12);
assert.equal(clampTimeoutMinutes(0.5), 1);
assert.equal(clampTimeoutMinutes(120.9), 120);

// formatElapsed: seconds under one minute.
assert.equal(formatElapsed(0), "0s");
assert.equal(formatElapsed(59), "59s");
assert.equal(formatElapsed(59.9), "59s");

// Minutes and seconds (seconds zero-padded).
assert.equal(formatElapsed(60), "1m 00s");
assert.equal(formatElapsed(65), "1m 05s");
assert.equal(formatElapsed(3599), "59m 59s");

// Hours and minutes (seconds dropped, minutes zero-padded).
assert.equal(formatElapsed(3600), "1h 00m");
assert.equal(formatElapsed(3720), "1h 02m");
assert.equal(formatElapsed(360000), "100h 00m");

// Invalid input is shown as zero elapsed time.
assert.equal(formatElapsed(-1), "0s");
assert.equal(formatElapsed(Number.NaN), "0s");

// stateLabelKey: every known state maps to its own translation key.
const known: Array<[string, string]> = [
  ["idle", "settings.experimental.jobs.state.idle"],
  ["running", "settings.experimental.jobs.state.running"],
  ["succeeded", "settings.experimental.jobs.state.succeeded"],
  ["failed", "settings.experimental.jobs.state.failed"],
  ["cancelled", "settings.experimental.jobs.state.cancelled"],
  ["timed_out", "settings.experimental.jobs.state.timedOut"],
  ["spawn_failed", "settings.experimental.jobs.state.couldntStart"],
];
for (const [state, key] of known) {
  assert.equal(stateLabelKey(state), key, `label key for ${state}`);
}

// Unknown, empty and differently-cased states fall back to the idle key.
const idleKey = "settings.experimental.jobs.state.idle";
for (const state of [
  "",
  "bogus",
  "RUNNING",
  "Running",
  " running",
  "timed out",
]) {
  assert.equal(
    stateLabelKey(state),
    idleKey,
    `fallback for ${JSON.stringify(state)}`,
  );
}

// isRunning: true only for the exact running state.
assert.equal(isRunning("running"), true);
for (const state of [
  "idle",
  "succeeded",
  "failed",
  "cancelled",
  "timed_out",
  "spawn_failed",
  "",
  "RUNNING",
  "bogus",
]) {
  assert.equal(isRunning(state), false, `isRunning(${JSON.stringify(state)})`);
}

console.log("agentJobs.test.ts: all assertions passed");
