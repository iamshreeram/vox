// Refusal copy (DESIGN Revision 3, A-C3, N6): the orb's sr-only status text and the bubble
// notice are the same accurate sentence for every notice code, and no text shows a raw code.
// Authoritative: do not weaken an assertion. Run with `bun run test:orb`.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { NOTICE_CODES } from "../bubbles/bubbleModel";
import { REFUSAL_CODES } from "./orbRefusal";

const read = (relative: string): string =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");

type Translation = {
  bubbles: { notice: Record<string, string> };
  orb: { refusal: Record<string, string> };
};

const en = JSON.parse(
  read("../i18n/locales/en/translation.json"),
) as Translation;

// Every orb refusal code is also a notice code, and the notice codes add only `unknown`.
for (const code of REFUSAL_CODES) {
  assert.ok(
    (NOTICE_CODES as readonly string[]).includes(code),
    `${code} is a refusal code with no notice code`,
  );
}

// The English notice and the orb text are identical for every code.
for (const code of NOTICE_CODES) {
  const notice = en.bubbles.notice[code];
  const orb = en.orb.refusal[code];
  assert.ok(notice, `bubbles.notice.${code} is missing in en`);
  assert.ok(orb, `orb.refusal.${code} is missing in en`);
  assert.equal(orb, notice, `${code}: orb text must equal the bubble text`);
}

// No text shows a raw code identifier (snake_case) or an i18n key. Plain words such as
// "ambient" are fine in a sentence.
const snakeIdentifier = /\b[a-z]+_[a-z_]+\b/;
for (const code of NOTICE_CODES) {
  for (const text of [en.bubbles.notice[code], en.orb.refusal[code]]) {
    assert.doesNotMatch(text, snakeIdentifier, `"${text}" shows an identifier`);
    assert.doesNotMatch(text, /\bbubbles\.|\borb\.refusal\b/, `"${text}"`);
  }
}

console.log("refusal copy: all assertions passed");
