import assert from "node:assert/strict";
import {
  canAddAgentName,
  cleanAgentNames,
  clampTimeoutMinutes,
  formatElapsed,
  isRunning,
  isValidAgentFlag,
  isValidAgentName,
  MAX_AGENT_NAMES,
  resultPanelText,
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

// ---- M4 S4: agent name, agent flag and result panel helpers (D7 parity with Rust D1) ----

// MAX_AGENT_NAMES is the backend cap of 20.
assert.equal(MAX_AGENT_NAMES, 20);

// isValidAgentName: accepted shapes (first char alnum, then alnum . _ -, up to 64 chars).
for (const name of [
  "a",
  "Ram",
  "agent-1",
  "agent.v2",
  "agent_x",
  "0agent",
  "A".repeat(64),
  "a" + "b".repeat(63),
]) {
  assert.equal(
    isValidAgentName(name),
    true,
    `valid name ${JSON.stringify(name)}`,
  );
}

// isValidAgentName: rejected shapes (empty, leading punctuation, spaces, controls, paths, non-ASCII, 65 chars).
for (const name of [
  "",
  " ",
  "-x",
  "--agent",
  ".x",
  "_x",
  "a b",
  "a\tb",
  "a\nb",
  "a\u0000b",
  "../x",
  "\u00e9",
  "a\u00e9",
  "\uff41",
  " ram",
  "ram ",
  "a".repeat(65),
]) {
  assert.equal(
    isValidAgentName(name),
    false,
    `invalid name ${JSON.stringify(name)}`,
  );
}

// cleanAgentNames: invalid entries are dropped and order of the rest is kept.
assert.deepEqual(cleanAgentNames(["alpha", "-bad", "beta", "a b", "gamma"]), [
  "alpha",
  "beta",
  "gamma",
]);

// cleanAgentNames: case-insensitive duplicates are dropped, the first spelling is kept.
assert.deepEqual(cleanAgentNames(["Ram", "ram", "RAM", "other"]), [
  "Ram",
  "other",
]);
assert.deepEqual(cleanAgentNames(["ram", "Ram"]), ["ram"]);

// cleanAgentNames: the cap is 20 valid names; the 21st is dropped.
const twentyOne = Array.from({ length: 21 }, (_, i) => `agent${i}`);
assert.deepEqual(cleanAgentNames(twentyOne), twentyOne.slice(0, 20));

// cleanAgentNames: invalid entries do not count toward the cap.
assert.deepEqual(
  cleanAgentNames(["-bad", ...twentyOne.slice(0, 20)]),
  twentyOne.slice(0, 20),
);

// cleanAgentNames: empty input gives an empty list.
assert.deepEqual(cleanAgentNames([]), []);

// isValidAgentFlag: accepted flags (single or double dash, letter first, up to 32 chars after the dashes).
for (const flag of [
  "--agent",
  "-a",
  "--agent-name",
  "--A1",
  "--" + "a".repeat(32),
]) {
  assert.equal(
    isValidAgentFlag(flag, "--prompt"),
    true,
    `valid flag ${JSON.stringify(flag)}`,
  );
}

// isValidAgentFlag: rejected flags (empty, bare dashes, no letter first, spaces, too long, equal to prompt flag).
for (const flag of [
  "",
  "agent",
  "-",
  "--",
  "---x",
  "--a b",
  "--1x",
  "--" + "a".repeat(33),
]) {
  assert.equal(
    isValidAgentFlag(flag, "--prompt"),
    false,
    `invalid flag ${JSON.stringify(flag)}`,
  );
}
assert.equal(
  isValidAgentFlag("--agent", "--agent"),
  false,
  "flag equal to prompt flag",
);

// canAddAgentName: true for a valid new name when there is room.
assert.equal(canAddAgentName(["alpha"], "beta"), true);
assert.equal(canAddAgentName([], "beta"), true);

// canAddAgentName: false for a case-insensitive duplicate of an existing entry.
assert.equal(canAddAgentName(["Ram"], "ram"), false);

// canAddAgentName: false for an invalid candidate.
assert.equal(canAddAgentName([], "a b"), false);
assert.equal(canAddAgentName([], ""), false);

// canAddAgentName: false once 20 entries exist, even for a valid new name.
const full = twentyOne.slice(0, 20);
assert.equal(canAddAgentName(full, "fresh"), false);
assert.equal(canAddAgentName(twentyOne.slice(0, 19), "fresh"), true);

// resultPanelText: null, undefined, empty and whitespace-only text give null.
assert.equal(resultPanelText(null), null);
assert.equal(resultPanelText(undefined), null);
assert.equal(resultPanelText(""), null);
assert.equal(resultPanelText("   "), null);
assert.equal(resultPanelText("\n\t \r\n"), null);

// resultPanelText: text is returned unchanged (not trimmed).
assert.equal(resultPanelText("  hi  "), "  hi  ");

// resultPanelText: multi-line text is kept byte-identical.
const multi = "line one\n\n  line two\r\nline three\t";
assert.equal(resultPanelText(multi), multi);

// resultPanelText: no truncation of long text (the backend caps it).
const long = "x".repeat(10000);
assert.equal(resultPanelText(long), long);

console.log("agentJobs.test.ts: all assertions passed");
