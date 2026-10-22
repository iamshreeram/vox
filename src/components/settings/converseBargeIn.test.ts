import assert from "node:assert/strict";
import {
  CONVERSE_BARGE_IN_VALUES,
  normalizeConverseBargeIn,
} from "./converseBargeIn";

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

scenario("the two modes are listed in display order", () => {
  assert.deepEqual([...CONVERSE_BARGE_IN_VALUES], ["off", "headphones"]);
});

scenario("valid values are kept as they are", () => {
  assert.equal(normalizeConverseBargeIn("off"), "off");
  assert.equal(normalizeConverseBargeIn("headphones"), "headphones");
});

scenario("a missing value falls back to off", () => {
  assert.equal(normalizeConverseBargeIn(undefined), "off");
  assert.equal(normalizeConverseBargeIn(null), "off");
});

scenario("unknown or differently-cased values fall back to off", () => {
  assert.equal(normalizeConverseBargeIn("speakers"), "off");
  assert.equal(normalizeConverseBargeIn("HEADPHONES"), "off");
  assert.equal(normalizeConverseBargeIn(""), "off");
  assert.equal(normalizeConverseBargeIn(1), "off");
});

console.log(`converseBargeIn: ${passed} scenarios passed`);
