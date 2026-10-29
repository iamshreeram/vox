// Track T spec (DESIGN Revision 3, item 6): the "Conversation window opacity" slider in the
// conversation group. Pure model tests plus source contracts for the component, the placement,
// the search index and the i18n keys in every locale. Run: bun src/components/settings/chatOpacity/chatOpacityForm.test.ts
// Workers must not edit this file.
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PLACED_GROUPS } from "../settingsPlacement";
import { SEARCH_ENTRIES } from "../search/settingsSearchIndex";
import {
  CHAT_OPACITY_DEFAULT,
  CHAT_OPACITY_MAX,
  CHAT_OPACITY_MIN,
  CHAT_OPACITY_STEP,
  chatOpacityCommandArgs,
  clampChatOpacity,
  createOpacityWriter,
  sliderValue,
} from "./chatOpacityForm";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../../..");

function test(name: string, body: () => void): void {
  try {
    body();
    console.log(`ok - ${name}`);
  } catch (error) {
    console.error(`not ok - ${name}`);
    throw error;
  }
}

test("range is 20..100 in steps of 1, default 70", () => {
  assert.equal(CHAT_OPACITY_MIN, 20);
  assert.equal(CHAT_OPACITY_MAX, 100);
  assert.equal(CHAT_OPACITY_STEP, 1);
  assert.equal(CHAT_OPACITY_DEFAULT, 70);
});

test("clampChatOpacity clamps and rounds, non-finite reads as the default", () => {
  assert.equal(clampChatOpacity(5), 20);
  assert.equal(clampChatOpacity(500), 100);
  assert.equal(clampChatOpacity(33.6), 34);
  assert.equal(clampChatOpacity(70), 70);
  assert.equal(clampChatOpacity(Number.NaN), 70);
});

test("sliderValue shows the stored value or the default when none is stored", () => {
  assert.equal(sliderValue(null), 70);
  assert.equal(sliderValue(undefined), 70);
  assert.equal(sliderValue(20), 20);
  assert.equal(sliderValue(250), 100);
});

test("the command argument object is { pct }", () => {
  assert.deepEqual(chatOpacityCommandArgs(55), { pct: 55 });
  assert.deepEqual(chatOpacityCommandArgs(1000), { pct: 100 });
});

test("live changes are debounced: only the latest value is sent", () => {
  const sent: number[] = [];
  let timer: (() => void) | null = null;
  const writer = createOpacityWriter({
    send: (pct) => sent.push(pct),
    schedule: (fn) => {
      timer = fn;
      return 1;
    },
    cancel: () => {
      timer = null;
    },
  });
  writer.change(30);
  writer.change(40);
  assert.deepEqual(sent, [], "nothing sent while the slider moves");
  assert.notEqual(timer, null);
  (timer as unknown as () => void)();
  assert.deepEqual(sent, [40], "one call with the latest value");
});

test("releasing the slider writes the pending value at once", () => {
  const sent: number[] = [];
  let cancelled = 0;
  const writer = createOpacityWriter({
    send: (pct) => sent.push(pct),
    schedule: () => 7,
    cancel: () => {
      cancelled += 1;
    },
  });
  writer.change(64);
  writer.release();
  assert.deepEqual(sent, [64]);
  assert.equal(cancelled, 1, "the pending timer is cancelled");
  writer.release();
  assert.deepEqual(sent, [64], "a release with nothing pending sends nothing");
});

test("reset writes 70 at once and cancels any pending change", () => {
  const sent: number[] = [];
  const writer = createOpacityWriter({
    send: (pct) => sent.push(pct),
    schedule: () => 3,
    cancel: () => undefined,
  });
  writer.change(25);
  writer.reset();
  assert.deepEqual(sent, [70]);
  assert.equal(writer.current(), 70);
});

test("the slider is a placed control in the conversation group, not experimental", () => {
  const group = PLACED_GROUPS.find((g) => g.id === "conversation");
  assert.ok(group, "conversation group exists");
  assert.equal(group.experimental, false);
  const entry = group.controls.find((c) => c.id === "chatOpacity");
  assert.ok(entry, "chatOpacity is placed in the conversation group");
  assert.equal(entry.experimental ?? false, false);
});

test("the search index has a row for the slider in the general section", () => {
  const row = SEARCH_ENTRIES.find((e) => e.id === "general.chatOpacity");
  assert.ok(row, "general.chatOpacity row");
  assert.equal(row.section, "general");
  assert.equal(row.groupId, "conversation");
  assert.equal(row.labelKey, "settings.chatOpacity.title");
});

test("the component calls the generated command and keeps its test ids", () => {
  const src = readFileSync(path.join(here, "ChatOpacitySettings.tsx"), "utf8");
  assert.match(src, /commands\.changeChatOpacitySetting\(/);
  for (const id of [
    "chat-opacity-slider",
    "chat-opacity-value",
    "chat-opacity-reset",
  ]) {
    assert.ok(src.includes(id), `test id ${id}`);
  }
  assert.ok(src.includes("settings.chatOpacity.title"));
});

test('the English label is "Conversation window opacity" (the value is percent opaque)', () => {
  const en = JSON.parse(
    readFileSync(
      path.join(repo, "src", "i18n", "locales", "en", "translation.json"),
      "utf8",
    ),
  ) as { settings: { chatOpacity: { title: string; value: string } } };
  assert.equal(en.settings.chatOpacity.title, "Conversation window opacity");
  assert.equal(en.settings.chatOpacity.value, "{{value}}%");
});

test("every locale carries the slider keys", () => {
  const localesDir = path.join(repo, "src", "i18n", "locales");
  const keys = ["title", "description", "reset"];
  const failures: string[] = [];
  for (const name of readdirSync(localesDir)) {
    const dir = path.join(localesDir, name);
    if (!statSync(dir).isDirectory()) continue;
    const json = JSON.parse(
      readFileSync(path.join(dir, "translation.json"), "utf8"),
    ) as { settings?: { chatOpacity?: Record<string, unknown> } };
    for (const key of keys) {
      const value = json.settings?.chatOpacity?.[key];
      if (typeof value !== "string" || value.trim() === "") {
        failures.push(`${name}: settings.chatOpacity.${key}`);
      }
    }
  }
  assert.deepEqual(failures, []);
});

console.log("chatOpacityForm: all tests passed");
