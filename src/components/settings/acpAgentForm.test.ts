import assert from "node:assert/strict";
import {
  ACP_MAX_ARGS,
  ACP_MAX_ARG_LEN,
  ACP_MAX_COMMAND_LEN,
  ACP_TIMEOUT_MAX_SECS,
  ACP_TIMEOUT_MIN_SECS,
  parseAcpArgs,
  parseAcpCommand,
  parseAcpTimeout,
  splitAcpArgs,
} from "./acpAgentForm";

// Bounds mirror the Rust validators in settings.rs (the backend stays authoritative).
assert.equal(ACP_TIMEOUT_MIN_SECS, 10);
assert.equal(ACP_TIMEOUT_MAX_SECS, 900);
assert.equal(ACP_MAX_ARGS, 8);
assert.equal(ACP_MAX_ARG_LEN, 256);
assert.equal(ACP_MAX_COMMAND_LEN, 1024);

// --- command -------------------------------------------------------------
// Blank means "unset" (null), never an error.
assert.deepEqual(parseAcpCommand(""), { ok: true, value: null });
assert.deepEqual(parseAcpCommand("   \t "), { ok: true, value: null });
// Trimmed, absolute paths are accepted.
assert.deepEqual(parseAcpCommand("  /path/to/my-agent  "), {
  ok: true,
  value: "/path/to/my-agent",
});
assert.deepEqual(parseAcpCommand("/opt/a b/agent"), {
  ok: true,
  value: "/opt/a b/agent",
});
// Relative paths and bare program names are refused (no PATH lookup).
for (const bad of ["my-agent", "./agent", "../agent", "~/agent", "agent --acp"]) {
  assert.deepEqual(parseAcpCommand(bad), { ok: false, error: "notAbsolute" });
}
// Length boundary: 1024 characters is fine, 1025 is not.
const slash = "/";
assert.equal(parseAcpCommand(slash + "a".repeat(ACP_MAX_COMMAND_LEN - 1)).ok, true);
assert.deepEqual(parseAcpCommand(slash + "a".repeat(ACP_MAX_COMMAND_LEN)), {
  ok: false,
  error: "tooLong",
});
// NUL / newline / other control characters are refused.
for (const bad of ["/a\u0000b", "/a\nb", "/a\rb", "/a\u001bb", "/a\u007fb"]) {
  assert.deepEqual(parseAcpCommand(bad), { ok: false, error: "controlChars" });
}

// --- args ----------------------------------------------------------------
// Whitespace split only; no quoting or shell parsing. The raw string is stored as typed.
assert.deepEqual(splitAcpArgs(""), []);
assert.deepEqual(splitAcpArgs("   "), []);
assert.deepEqual(splitAcpArgs("--acp"), ["--acp"]);
assert.deepEqual(splitAcpArgs("  --acp \t --verbose  "), ["--acp", "--verbose"]);
assert.deepEqual(splitAcpArgs('--name "two words" $HOME `x` ;'), [
  "--name",
  '"two',
  'words"',
  "$HOME",
  "`x`",
  ";",
]);
assert.deepEqual(parseAcpArgs(""), { ok: true, value: "" });
assert.deepEqual(parseAcpArgs("--acp --flag"), { ok: true, value: "--acp --flag" });
// Tabs and spaces are separators, not control-character errors.
assert.equal(parseAcpArgs("a\tb").ok, true);
// Count boundary: 8 args fine, 9 refused.
const eight = Array.from({ length: 8 }, (_, i) => `a${i}`).join(" ");
const nine = `${eight} a8`;
assert.equal(parseAcpArgs(eight).ok, true);
assert.deepEqual(parseAcpArgs(nine), { ok: false, error: "tooMany" });
// Per-argument length boundary: 256 fine, 257 refused.
assert.equal(parseAcpArgs("x".repeat(ACP_MAX_ARG_LEN)).ok, true);
assert.deepEqual(parseAcpArgs("x".repeat(ACP_MAX_ARG_LEN + 1)), {
  ok: false,
  error: "argTooLong",
});
// NUL, newline and other control characters are refused.
for (const bad of ["a\u0000b", "a\nb", "a\rb", "a\u001bb", "a\u007fb"]) {
  assert.deepEqual(parseAcpArgs(bad), { ok: false, error: "controlChars" });
}

// --- turn timeout ----------------------------------------------------------
assert.deepEqual(parseAcpTimeout("120"), { ok: true, value: 120 });
assert.deepEqual(parseAcpTimeout("  45 "), { ok: true, value: 45 });
assert.deepEqual(parseAcpTimeout("10"), { ok: true, value: 10 });
assert.deepEqual(parseAcpTimeout("900"), { ok: true, value: 900 });
for (const bad of ["9", "901", "0", "-5", "100000"]) {
  assert.deepEqual(parseAcpTimeout(bad), { ok: false, error: "outOfRange" });
}
for (const bad of ["", "  ", "abc", "1.5", "1e2", "12 3", "+30", "0x20", "NaN"]) {
  assert.deepEqual(parseAcpTimeout(bad), { ok: false, error: "notInteger" });
}

console.log("acpAgentForm: all assertions passed");
