import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

// The wake word now only starts a conversation. The English copy must never describe it
// as dictation or as a "wake-word recording" (the removed one-shot path).

const here = path.dirname(fileURLToPath(import.meta.url));
const enFile = path.join(here, "locales", "en", "translation.json");
const en: unknown = JSON.parse(readFileSync(enFile, "utf8"));

function collectStrings(node: unknown, out: string[]): string[] {
  if (typeof node === "string") {
    out.push(node);
  } else if (node !== null && typeof node === "object") {
    for (const value of Object.values(node)) collectStrings(value, out);
  }
  return out;
}

function lookup(root: unknown, dottedKey: string): unknown {
  let current: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (current === null || typeof current !== "object") return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

const wakeWordBlock = lookup(en, "settings.advanced.wakeWord");
assert.ok(
  wakeWordBlock !== undefined && wakeWordBlock !== null,
  "en settings.advanced.wakeWord block exists",
);
const strings = collectStrings(wakeWordBlock, []);
assert.ok(strings.length > 0, "wake word block has strings to check");

for (const text of strings) {
  assert.doesNotMatch(
    text,
    /dictation/i,
    `wake word copy must not mention dictation: ${text}`,
  );
  assert.doesNotMatch(
    text,
    /wake-word recording/i,
    `wake word copy must not mention a wake-word recording: ${text}`,
  );
}

console.log(
  `wakeWordCopy.test.ts: all assertions passed (${strings.length} strings)`,
);
