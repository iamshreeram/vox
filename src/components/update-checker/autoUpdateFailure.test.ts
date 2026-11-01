import assert from "node:assert/strict";
import type { AutoUpdateStatus } from "../../bindings";
import {
  DISMISS_KEY,
  autoUpdateFailureNotice,
  parseDismissed,
} from "./autoUpdateFailure";

function status(partial: Partial<AutoUpdateStatus> = {}): AutoUpdateStatus {
  return {
    enabled: true,
    effective: true,
    phase: "idle",
    last_result: "failed",
    reason_code: "build_failed",
    target_short: "abc1234",
    ts: 1700000000,
    ...partial,
  };
}

const KEY = "settings.debug.autoUpdate.reasons";

const cases: Record<string, () => void> = {
  "no status shows nothing"() {
    assert.equal(autoUpdateFailureNotice(null, null), null);
  },
  "a failed idle attempt shows the notice with its reason and target"() {
    assert.deepEqual(autoUpdateFailureNotice(status(), null), {
      reasonKey: `${KEY}.build_failed`,
      ts: 1700000000,
      targetShort: "abc1234",
    });
  },
  "skipped never shows"() {
    assert.equal(
      autoUpdateFailureNotice(status({ last_result: "skipped" }), null),
      null,
    );
  },
  "updated never shows"() {
    assert.equal(
      autoUpdateFailureNotice(status({ last_result: "updated" }), null),
      null,
    );
  },
  "none never shows"() {
    assert.equal(
      autoUpdateFailureNotice(status({ last_result: "none" }), null),
      null,
    );
  },
  "a new attempt in progress hides the old failure"() {
    for (const phase of ["building", "waiting", "installing"] as const) {
      assert.equal(autoUpdateFailureNotice(status({ phase }), null), null);
    }
  },
  "switching automatic updates off hides the failure"() {
    assert.equal(
      autoUpdateFailureNotice(status({ enabled: false }), null),
      null,
    );
  },
  "a failure without a timestamp is not shown (it cannot be dismissed)"() {
    assert.equal(autoUpdateFailureNotice(status({ ts: null }), null), null);
  },
  "the dismissed failure stays hidden, a newer one shows again"() {
    assert.equal(autoUpdateFailureNotice(status(), 1700000000), null);
    const again = autoUpdateFailureNotice(
      status({ ts: 1700000500 }),
      1700000000,
    );
    assert.equal(again?.ts, 1700000500);
  },
  "unknown or missing reason codes use the generic reason"() {
    assert.equal(
      autoUpdateFailureNotice(status({ reason_code: "weird_code" }), null)
        ?.reasonKey,
      `${KEY}.unknown`,
    );
    assert.equal(
      autoUpdateFailureNotice(status({ reason_code: null }), null)?.reasonKey,
      `${KEY}.unknown`,
    );
  },
  "a missing target is passed through as null"() {
    assert.equal(
      autoUpdateFailureNotice(status({ target_short: null }), null)
        ?.targetShort,
      null,
    );
  },
  "parseDismissed accepts only a non-negative integer string"() {
    assert.equal(parseDismissed(null), null);
    assert.equal(parseDismissed(""), null);
    assert.equal(parseDismissed("abc"), null);
    assert.equal(parseDismissed("1.5"), null);
    assert.equal(parseDismissed("-3"), null);
    assert.equal(parseDismissed("1e3"), null);
    assert.equal(parseDismissed(" 7 "), null);
    assert.equal(parseDismissed("123"), 123);
    assert.equal(parseDismissed("0"), 0);
    assert.equal(parseDismissed("99999999999999999999999"), null);
  },
  "the storage key is stable"() {
    assert.equal(DISMISS_KEY, "vox.autoUpdate.dismissedFailureTs");
  },
};

let failed = 0;
for (const [name, run] of Object.entries(cases)) {
  try {
    run();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL - ${name}\n${error}`);
  }
}
console.log(
  `${Object.keys(cases).length - failed}/${Object.keys(cases).length} passed`,
);
if (failed > 0) process.exit(1);
