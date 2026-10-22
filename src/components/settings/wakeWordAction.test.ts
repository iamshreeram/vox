import assert from "node:assert/strict";
import {
  DEFAULT_WAKE_WORD_ACTION,
  WAKE_WORD_ACTIONS,
  normalizeWakeWordAction,
  wakeWordActionLabelKey,
} from "./wakeWordAction";

let passed = 0;
function scenario(name: string, body: () => void) {
  try {
    body();
    passed += 1;
  } catch (err) {
    console.error(`FAILED: ${name}`);
    throw err;
  }
}

scenario("the two actions are listed in display order", () => {
  assert.deepEqual([...WAKE_WORD_ACTIONS], ["voice_request", "conversation"]);
});

scenario("the default is today's one-shot voice request", () => {
  assert.equal(DEFAULT_WAKE_WORD_ACTION, "voice_request");
});

scenario("valid values are kept as they are", () => {
  assert.equal(normalizeWakeWordAction("voice_request"), "voice_request");
  assert.equal(normalizeWakeWordAction("conversation"), "conversation");
});

scenario("anything else falls back to the default", () => {
  for (const bad of [undefined, null, "", "banana", "Conversation", 3, {}]) {
    assert.equal(normalizeWakeWordAction(bad), "voice_request");
  }
});

scenario("label keys live next to the other wake word strings", () => {
  assert.equal(
    wakeWordActionLabelKey("voice_request"),
    "settings.advanced.wakeWord.action.voiceRequest",
  );
  assert.equal(
    wakeWordActionLabelKey("conversation"),
    "settings.advanced.wakeWord.action.conversation",
  );
});

console.log(`${passed} scenarios passed`);
