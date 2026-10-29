import assert from "node:assert/strict";
import { WAKE_WORD_ACTION } from "./wakeWordAction";

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

scenario("a wake word detection always starts a conversation", () => {
  assert.equal(WAKE_WORD_ACTION, "conversation");
});

console.log(`${passed} scenarios passed`);
