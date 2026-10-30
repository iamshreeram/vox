// Orb refusal state (DESIGN Revision 3, A-C2/A-C3): pure model for the sr-only status text and the
// `concerned` face after a refused click. Authoritative: do not weaken an assertion.
// Run with `bun run test:orb`.
//
// Contract (src/orb/orbRefusal.ts):
//   REFUSAL_CODES = dictation_busy, tts_off, composer_off, ambient, experimental_off, text_ending
//   refusalCode(payload) -> a known code, or "unknown" for anything else (never throws)
//   refusalKey(payload) -> `orb.refusal.<code>`
//   refusalReducer: refuse -> seq+1 and key; clear (optional seq) clears only the matching seq
//   startRefusalTimers(host, callbacks) -> cancel: text clears at 4000 ms, face returns at 1200 ms
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  CONCERNED_MS,
  REFUSAL_CODES,
  REFUSAL_TEXT_MS,
  initialRefusal,
  refusalCode,
  refusalKey,
  refusalReducer,
  startRefusalTimers,
  type TimerHost,
} from "./orbRefusal";

const read = (relative: string): string =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");

// Every known code maps to itself and to its own i18n key.
assert.deepEqual(
  [...REFUSAL_CODES],
  [
    "dictation_busy",
    "tts_off",
    "composer_off",
    "ambient",
    "experimental_off",
    "text_ending",
  ],
);
for (const code of REFUSAL_CODES) {
  assert.equal(refusalCode({ code }), code);
  assert.equal(refusalKey({ code }), `orb.refusal.${code}`);
}

// Unknown, missing or garbage payloads read as unknown, never as a raw value.
const GARBAGE: unknown[] = [
  undefined,
  null,
  "dictation_busy",
  42,
  { code: "unknown_thing" },
  { code: "<img src=x>" },
  { code: ["tts_off"] },
  { code: null },
  { code: "" },
  { notCode: "tts_off" },
  [],
];
for (const payload of GARBAGE) {
  assert.equal(refusalCode(payload), "unknown", JSON.stringify(payload));
  assert.equal(refusalKey(payload), "orb.refusal.unknown");
}

// Reducer: refuse sets the key and a new seq; a repeat refusal gets a new seq.
let state = refusalReducer(initialRefusal, {
  type: "refuse",
  payload: { code: "dictation_busy" },
});
assert.equal(state.key, "orb.refusal.dictation_busy");
assert.equal(state.seq, 1);
state = refusalReducer(state, {
  type: "refuse",
  payload: { code: "tts_off" },
});
assert.equal(state.key, "orb.refusal.tts_off");
assert.equal(state.seq, 2);

// A timer for an old seq never clears a newer message.
const stale = refusalReducer(state, { type: "clear", seq: 1 });
assert.equal(stale, state, "stale clear is ignored");

// A clear for the current seq (the 4 s timer) clears the text.
const timed = refusalReducer(state, { type: "clear", seq: 2 });
assert.equal(timed.key, null);
assert.equal(timed.seq, 2);

// A clear without seq (click or conversation start) clears whatever is shown.
const clicked = refusalReducer(state, { type: "clear" });
assert.equal(clicked.key, null);

// Clearing an already empty state changes nothing.
assert.equal(refusalReducer(initialRefusal, { type: "clear" }), initialRefusal);

// Unknown payload through the reducer uses the generic key.
const unknown = refusalReducer(initialRefusal, {
  type: "refuse",
  payload: "<b>x</b>",
});
assert.equal(unknown.key, "orb.refusal.unknown");

// Timers: a fake host records each schedule.
assert.equal(REFUSAL_TEXT_MS, 4000);
assert.equal(CONCERNED_MS, 1200);

interface Scheduled {
  ms: number;
  fn: () => void;
  cancelled: boolean;
}

function fakeHost(): { host: TimerHost; scheduled: Scheduled[] } {
  const scheduled: Scheduled[] = [];
  const host: TimerHost = {
    setTimeout(fn, ms) {
      const entry: Scheduled = { ms, fn, cancelled: false };
      scheduled.push(entry);
      return entry;
    },
    clearTimeout(handle) {
      (handle as Scheduled).cancelled = true;
    },
  };
  return { host, scheduled };
}

{
  const { host, scheduled } = fakeHost();
  const events: string[] = [];
  const cancel = startRefusalTimers(host, {
    onConcernedEnd: () => events.push("face"),
    onTextEnd: () => events.push("text"),
  });
  assert.deepEqual(
    scheduled.map((s) => s.ms),
    [CONCERNED_MS, REFUSAL_TEXT_MS],
  );
  scheduled[0].fn();
  assert.deepEqual(events, ["face"]);
  scheduled[1].fn();
  assert.deepEqual(events, ["face", "text"]);
  cancel();
  assert.ok(
    scheduled.every((s) => s.cancelled),
    "cancel clears both timers",
  );
}

// Cancelling before the timers fire (a click or a new refusal) runs nothing.
{
  const { host, scheduled } = fakeHost();
  const events: string[] = [];
  const cancel = startRefusalTimers(host, {
    onConcernedEnd: () => events.push("face"),
    onTextEnd: () => events.push("text"),
  });
  cancel();
  assert.equal(scheduled.length, 2);
  assert.ok(scheduled.every((s) => s.cancelled));
  assert.deepEqual(events, []);
}

// Static: the orb page's refusal path adds no command and no GPU or blur effect.
const orbApp = read("./OrbApp.tsx");
const allowedInvokes = new Set([
  "orb_get_state",
  "orb_pointer_down",
  "orb_set_reduced_motion",
]);
const invokeNames = [...orbApp.matchAll(/invoke\(\s*"([a-z_]+)"/g)].map(
  (m) => m[1],
);
for (const name of invokeNames) {
  assert.ok(allowedInvokes.has(name), `OrbApp invokes ${name}`);
}
const sendNames = [...orbApp.matchAll(/send\(\s*"([a-z_]+)"/g)].map(
  (m) => m[1],
);
for (const name of sendNames) {
  assert.ok(
    ["orb_menu", "orb_activate", "orb_set_reduced_motion"].includes(name),
    `OrbApp sends ${name}`,
  );
}
assert.ok(orbApp.includes('"orb-refusal"'), "OrbApp listens to orb-refusal");

const modelFiles = [
  "./OrbApp.tsx",
  "./orbRefusal.ts",
  "./orbAccentResolve.ts",
  "./orbAppearance.ts",
  "./orb.css",
];
for (const file of modelFiles) {
  const source = read(file);
  assert.doesNotMatch(source, /backdrop-filter/, `${file} backdrop-filter`);
  assert.doesNotMatch(source, /blur\(/, `${file} blur()`);
  assert.doesNotMatch(source, /will-change/, `${file} will-change`);
}
for (const file of [
  "./orbRefusal.ts",
  "./orbAccentResolve.ts",
  "./orbAppearance.ts",
]) {
  assert.doesNotMatch(read(file), /\binvoke\(/, `${file} must not invoke`);
}

console.log("orb refusal: all assertions passed");
