import assert from "node:assert/strict";
import {
  formatStepAgentProfile,
  isStepAgentType,
  parseStepAgentDraft,
  sameStepAgentProfile,
  STEP_AGENT_TYPES,
} from "./stepAgentsForm";

// Only the four step types with a slot. `general` always uses the main agent.
assert.deepEqual(
  [...STEP_AGENT_TYPES],
  ["research", "browser", "code", "files"],
);

// --- step type guard -------------------------------------------------------
for (const word of ["research", "browser", "code", "files"]) {
  assert.equal(isStepAgentType(word), true, word);
}
for (const bad of [
  "general",
  "Research",
  " code",
  "",
  "__proto__",
  "constructor",
  42,
  null,
  undefined,
  { research: true },
]) {
  assert.equal(isStepAgentType(bad), false, String(bad));
}

// --- formatting stored profiles into row text -------------------------------
assert.deepEqual(formatStepAgentProfile(null), { command: "", args: "" });
assert.deepEqual(formatStepAgentProfile(undefined), { command: "", args: "" });
assert.deepEqual(
  formatStepAgentProfile({ command: "/opt/agent", args: "--acp --fast" }),
  { command: "/opt/agent", args: "--acp --fast" },
);

// --- blank means use the main agent (null clears the slot) ------------------
assert.deepEqual(parseStepAgentDraft({ command: "", args: "" }), {
  ok: true,
  value: null,
});
assert.deepEqual(parseStepAgentDraft({ command: "   \t ", args: "  " }), {
  ok: true,
  value: null,
});

// A blank command clears the slot even when the arguments are filled in.
assert.deepEqual(parseStepAgentDraft({ command: " ", args: "--acp" }), {
  ok: true,
  value: null,
});

// --- a set profile: trimmed command, arguments stored trimmed ---------------
assert.deepEqual(
  parseStepAgentDraft({
    command: "  /opt/agents/research  ",
    args: "  --acp  ",
  }),
  {
    ok: true,
    value: { command: "/opt/agents/research", args: "--acp" },
  },
);
assert.deepEqual(
  parseStepAgentDraft({ command: "/opt/agents/research", args: "" }),
  { ok: true, value: { command: "/opt/agents/research", args: "" } },
);

// --- quoting: no shell parsing, quotes are ordinary characters -------------
// The backend splits on ASCII whitespace, so quotes are stored as typed.
assert.deepEqual(
  parseStepAgentDraft({
    command: "/opt/agent",
    args: '--name "two words"',
  }),
  {
    ok: true,
    value: { command: "/opt/agent", args: '--name "two words"' },
  },
);

// --- invalid command ------------------------------------------------------
assert.deepEqual(parseStepAgentDraft({ command: "agent", args: "" }), {
  ok: false,
  error: "notAbsolute",
});
assert.deepEqual(parseStepAgentDraft({ command: "./agent", args: "" }), {
  ok: false,
  error: "notAbsolute",
});
assert.deepEqual(parseStepAgentDraft({ command: "/opt/a\nb", args: "" }), {
  ok: false,
  error: "controlChars",
});

// Command length: 1024 is accepted, 1025 is not.
const longCommand = `/${"a".repeat(1023)}`;
assert.equal(longCommand.length, 1024);
assert.equal(parseStepAgentDraft({ command: longCommand, args: "" }).ok, true);
assert.deepEqual(
  parseStepAgentDraft({ command: `${longCommand}a`, args: "" }),
  { ok: false, error: "tooLong" },
);

// --- invalid arguments (checked even when the command is blank) -----------
assert.deepEqual(
  parseStepAgentDraft({ command: "/opt/agent", args: "a\tb\u0007" }),
  {
    ok: false,
    error: "controlChars",
  },
);
assert.deepEqual(parseStepAgentDraft({ command: "", args: "a\u0007" }), {
  ok: false,
  error: "controlChars",
});

// Argument count: 8 is accepted, 9 is not.
const eight = Array.from({ length: 8 }, (_, i) => `-${i}`).join(" ");
assert.equal(
  parseStepAgentDraft({ command: "/opt/agent", args: eight }).ok,
  true,
);
const nine = `${eight} -9`;
assert.deepEqual(parseStepAgentDraft({ command: "/opt/agent", args: nine }), {
  ok: false,
  error: "tooMany",
});

// Argument length: 256 is accepted, 257 is not.
assert.equal(
  parseStepAgentDraft({
    command: "/opt/agent",
    args: "x".repeat(256),
  }).ok,
  true,
);
assert.deepEqual(
  parseStepAgentDraft({ command: "/opt/agent", args: "x".repeat(257) }),
  { ok: false, error: "argTooLong" },
);

// --- equality for skipping no-op writes -------------------------------------
assert.equal(sameStepAgentProfile(null, undefined), true);
assert.equal(sameStepAgentProfile(null, null), true);
assert.equal(
  sameStepAgentProfile(null, { command: "/opt/agent", args: "" }),
  false,
);
assert.equal(
  sameStepAgentProfile(
    { command: "/opt/agent", args: "--a" },
    { command: "/opt/agent", args: "--a" },
  ),
  true,
);
assert.equal(
  sameStepAgentProfile(
    { command: "/opt/agent", args: "--a" },
    { command: "/opt/agent", args: "--b" },
  ),
  false,
);

console.log("step agents form: all checks passed");
