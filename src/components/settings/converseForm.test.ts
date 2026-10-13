import assert from "node:assert/strict";
import {
  CONVERSE_LIMITS,
  buttonAction,
  clampSetting,
  endLabelKey,
  isSessionActive,
  parseConverseState,
  stateLabelKey,
} from "./converseForm";

let passed = 0;
function scenario(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`ok - ${name}`);
}

scenario("limits match the backend clamps", () => {
  assert.deepEqual(CONVERSE_LIMITS.silenceMs, {
    min: 600,
    max: 4000,
    step: 100,
    default: 1200,
  });
  assert.deepEqual(CONVERSE_LIMITS.idleTurns, {
    min: 1,
    max: 5,
    step: 1,
    default: 2,
  });
  assert.deepEqual(CONVERSE_LIMITS.contextTurns, {
    min: 0,
    max: 12,
    step: 1,
    default: 6,
  });
});

scenario("clampSetting clamps both ends and keeps in-range values", () => {
  assert.equal(clampSetting("silenceMs", 0), 600);
  assert.equal(clampSetting("silenceMs", 100000), 4000);
  assert.equal(clampSetting("silenceMs", 2500), 2500);
  assert.equal(clampSetting("idleTurns", 0), 1);
  assert.equal(clampSetting("idleTurns", 99), 5);
  assert.equal(clampSetting("contextTurns", -3), 0);
  assert.equal(clampSetting("contextTurns", 200), 12);
  assert.equal(clampSetting("contextTurns", 0), 0);
});

scenario(
  "clampSetting rounds and rejects non-finite values with the default",
  () => {
    assert.equal(clampSetting("idleTurns", 2.6), 3);
    assert.equal(clampSetting("silenceMs", 1234.4), 1234);
    assert.equal(clampSetting("silenceMs", Number.NaN), 1200);
    assert.equal(clampSetting("idleTurns", Number.POSITIVE_INFINITY), 2);
    assert.equal(clampSetting("contextTurns", Number.NEGATIVE_INFINITY), 6);
  },
);

scenario(
  "parseConverseState accepts every documented state and end reason",
  () => {
    for (const state of [
      "listening",
      "thinking",
      "speaking",
      "acting",
      "idle",
    ]) {
      assert.deepEqual(parseConverseState({ state, end: null }), {
        state,
        end: null,
      });
    }
    for (const end of [
      "user_ended",
      "idle",
      "cancelled",
      "not_configured",
      "audio_unavailable",
      "speech_stuck",
      "turn_limit",
    ]) {
      assert.deepEqual(parseConverseState({ state: "idle", end }), {
        state: "idle",
        end,
      });
    }
  },
);

scenario(
  "parseConverseState degrades garbage to idle with no end reason",
  () => {
    const idle = { state: "idle", end: null };
    assert.deepEqual(parseConverseState(null), idle);
    assert.deepEqual(parseConverseState(undefined), idle);
    assert.deepEqual(parseConverseState("listening"), idle);
    assert.deepEqual(parseConverseState(42), idle);
    assert.deepEqual(parseConverseState({}), idle);
    assert.deepEqual(parseConverseState({ state: 7, end: 9 }), idle);
    assert.deepEqual(
      parseConverseState({ state: "dancing", end: "exploded" }),
      idle,
    );
    // a known state with an unknown end keeps the state, drops the end
    assert.deepEqual(
      parseConverseState({ state: "speaking", end: "exploded" }),
      {
        state: "speaking",
        end: null,
      },
    );
    // prototype-pollution style keys are ignored
    assert.deepEqual(
      parseConverseState(JSON.parse('{"__proto__":{"state":"listening"}}')),
      idle,
    );
  },
);

scenario("state and end label keys are stable i18n keys", () => {
  assert.equal(stateLabelKey("listening"), "converse.state.listening");
  assert.equal(stateLabelKey("thinking"), "converse.state.thinking");
  assert.equal(stateLabelKey("speaking"), "converse.state.speaking");
  assert.equal(stateLabelKey("acting"), "converse.state.acting");
  assert.equal(stateLabelKey("idle"), "converse.state.idle");
  assert.equal(endLabelKey(null), null);
  assert.equal(endLabelKey("user_ended"), "converse.end.user_ended");
  assert.equal(endLabelKey("not_configured"), "converse.end.not_configured");
  assert.equal(endLabelKey("turn_limit"), "converse.end.turn_limit");
});

scenario("isSessionActive is true for every state except idle", () => {
  assert.equal(isSessionActive("idle"), false);
  for (const s of ["listening", "thinking", "speaking", "acting"] as const) {
    assert.equal(isSessionActive(s), true);
  }
});

scenario(
  "buttonAction never offers start/stop while a request is in flight",
  () => {
    assert.equal(buttonAction(false, false), "start");
    assert.equal(buttonAction(true, false), "stop");
    assert.equal(buttonAction(false, true), "disabled");
    assert.equal(buttonAction(true, true), "disabled");
  },
);

console.log(`${passed} scenarios passed`);
