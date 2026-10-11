import assert from "node:assert/strict";
import type { SourceUpdateInfo } from "../../bindings";
import {
  canConfirmInstall,
  footerAction,
  footerLabelKey,
  formatChangeList,
  initialState,
  reducer,
  type Action,
  type UpdateState,
} from "./sourceUpdateState";

function info(partial: Partial<SourceUpdateInfo> = {}): SourceUpdateInfo {
  return {
    state: "available",
    current_commit: "aaaaaaa",
    latest_commit: "bbbbbbb",
    behind_by: 2,
    changes: ["one", "two"],
    compare_url: "https://github.com/iamshreeram/vox/compare/a...b",
    note: null,
    can_install: true,
    install_blocker: null,
    ...partial,
  };
}

const available = info();
const upToDate = info({
  state: "up_to_date",
  behind_by: 0,
  changes: [],
});
const untracked = info({
  state: "untracked",
  behind_by: 0,
  changes: [],
  note: "diverged",
  can_install: false,
});

function run(actions: Action[], from: UpdateState = initialState): UpdateState {
  return actions.reduce(reducer, from);
}

function state(partial: Partial<UpdateState>): UpdateState {
  return { ...initialState, ...partial };
}

const scenarios: Record<string, () => void> = {
  // ---- checking ---------------------------------------------------------
  "a check moves idle to checking and clears any error"() {
    const s = run(
      [{ type: "checkStarted", manual: true }],
      state({ phase: "error", error: "boom" }),
    );
    assert.equal(s.phase, "checking");
    assert.equal(s.error, null);
    assert.equal(s.manual, true);
  },

  "an available result shows the update, manual or automatic"() {
    for (const manual of [true, false]) {
      const s = run([
        { type: "checkStarted", manual },
        { type: "checkSucceeded", info: available },
      ]);
      assert.equal(s.phase, "available");
      assert.deepEqual(s.info, available);
      assert.equal(s.manual, false);
    }
  },

  "up to date: manual shows the confirmation, automatic stays silent"() {
    const manual = run([
      { type: "checkStarted", manual: true },
      { type: "checkSucceeded", info: upToDate },
    ]);
    assert.equal(manual.phase, "upToDate");
    const auto = run([
      { type: "checkStarted", manual: false },
      { type: "checkSucceeded", info: upToDate },
    ]);
    assert.equal(auto.phase, "idle");
    assert.deepEqual(auto.info, upToDate);
  },

  "untracked: manual shows the note state, automatic stays silent"() {
    const manual = run([
      { type: "checkStarted", manual: true },
      { type: "checkSucceeded", info: untracked },
    ]);
    assert.equal(manual.phase, "untracked");
    assert.equal(manual.info?.note, "diverged");
    const auto = run([
      { type: "checkStarted", manual: false },
      { type: "checkSucceeded", info: untracked },
    ]);
    assert.equal(auto.phase, "idle");
  },

  "a manual failure surfaces the message"() {
    const s = run([
      { type: "checkStarted", manual: true },
      { type: "checkFailed", message: "GitHub rate limit" },
    ]);
    assert.equal(s.phase, "error");
    assert.equal(s.error, "GitHub rate limit");
  },

  "an automatic failure is silent and keeps what was already known"() {
    const idle = run([
      { type: "checkStarted", manual: false },
      { type: "checkFailed", message: "offline" },
    ]);
    assert.equal(idle.phase, "idle");
    assert.equal(idle.error, null);

    const known = run(
      [
        { type: "checkStarted", manual: false },
        { type: "checkFailed", message: "offline" },
      ],
      state({ phase: "available", info: available }),
    );
    assert.equal(known.phase, "available");
    assert.deepEqual(known.info, available);
  },

  // ---- pushed / hydrated status ------------------------------------------
  "a pushed Available status shows the update; a pushed UpToDate clears it"() {
    const shown = run([{ type: "statusPushed", info: available }]);
    assert.equal(shown.phase, "available");
    const cleared = run(
      [{ type: "statusPushed", info: upToDate }],
      state({ phase: "available", info: available }),
    );
    assert.equal(cleared.phase, "idle");
    assert.deepEqual(cleared.info, upToDate);
  },

  "a pushed null (checks disabled or cleared) resets to idle"() {
    const s = run(
      [{ type: "statusPushed", info: null }],
      state({ phase: "available", info: available }),
    );
    assert.equal(s.phase, "idle");
    assert.equal(s.info, null);
  },

  "pushed status is ignored while checking, starting or started"() {
    for (const phase of ["checking", "starting", "started"] as const) {
      const before = state({ phase, info: available });
      const after = run([{ type: "statusPushed", info: upToDate }], before);
      assert.deepEqual(after, before, phase);
    }
  },

  "hydrating only applies to an idle state"() {
    assert.equal(
      run([{ type: "hydrated", info: available }]).phase,
      "available",
    );
    const busy = state({ phase: "checking" });
    assert.deepEqual(run([{ type: "hydrated", info: available }], busy), busy);
    assert.equal(run([{ type: "hydrated", info: null }]).phase, "idle");
  },

  // ---- install flow --------------------------------------------------------
  "install starts only from an installable available update"() {
    const ok = run(
      [{ type: "installStarted" }],
      state({ phase: "available", info: available }),
    );
    assert.equal(ok.phase, "starting");

    const blocked = state({
      phase: "available",
      info: info({ can_install: false, install_blocker: "dirty" }),
    });
    assert.deepEqual(run([{ type: "installStarted" }], blocked), blocked);

    const idle = state({ phase: "idle" });
    assert.deepEqual(run([{ type: "installStarted" }], idle), idle);
  },

  "launched moves starting to started; failure returns an error with info kept"() {
    const started = run(
      [{ type: "installLaunched" }],
      state({ phase: "starting", info: available }),
    );
    assert.equal(started.phase, "started");

    const failed = run(
      [{ type: "installFailed", message: "Terminal would not open" }],
      state({ phase: "starting", info: available }),
    );
    assert.equal(failed.phase, "error");
    assert.equal(failed.error, "Terminal would not open");
    assert.deepEqual(failed.info, available);

    // Stray install events outside the starting phase change nothing.
    const stray = state({ phase: "idle" });
    assert.deepEqual(run([{ type: "installLaunched" }], stray), stray);
    assert.deepEqual(
      run([{ type: "installFailed", message: "x" }], stray),
      stray,
    );
  },

  "while starting, checks are ignored entirely"() {
    const before = state({ phase: "starting", info: available });
    for (const a of [
      { type: "checkStarted", manual: true },
      { type: "checkSucceeded", info: upToDate },
      { type: "checkFailed", message: "x" },
    ] as Action[]) {
      assert.deepEqual(run([a], before), before, a.type);
    }
  },

  "started is not terminal: a manual re-check is allowed and an automatic one is ignored"() {
    const started = state({ phase: "started", info: available });
    const auto = run(
      [
        { type: "checkStarted", manual: false },
        { type: "checkSucceeded", info: upToDate },
      ],
      started,
    );
    assert.deepEqual(auto, started);

    const manual = run(
      [
        { type: "checkStarted", manual: true },
        { type: "checkSucceeded", info: available },
      ],
      started,
    );
    assert.equal(manual.phase, "available");
  },

  // ---- disable & expiry ------------------------------------------------------
  "disabling checks resets everything, even mid-flow"() {
    for (const phase of [
      "checking",
      "available",
      "starting",
      "started",
      "error",
    ] as const) {
      const s = run(
        [{ type: "checksDisabled" }],
        state({ phase, info: available, error: "e" }),
      );
      assert.deepEqual(s, initialState, phase);
    }
  },

  "the transient up-to-date and untracked notices expire back to idle"() {
    for (const phase of ["upToDate", "untracked"] as const) {
      const s = run(
        [{ type: "notificationExpired" }],
        state({ phase, info: upToDate }),
      );
      assert.equal(s.phase, "idle");
      assert.deepEqual(s.info, upToDate);
    }
    const other = state({ phase: "available", info: available });
    assert.deepEqual(run([{ type: "notificationExpired" }], other), other);
  },

  "reducer never mutates its input"() {
    const before = state({ phase: "available", info: available });
    const snapshot = JSON.stringify(before);
    run(
      [
        { type: "checkStarted", manual: true },
        { type: "checkSucceeded", info: upToDate },
        { type: "checksDisabled" },
      ],
      before,
    );
    assert.equal(JSON.stringify(before), snapshot);
  },

  // ---- footer ---------------------------------------------------------------
  "footer label keys for every phase"() {
    const key = (phase: UpdateState["phase"], enabled = true) =>
      footerLabelKey(state({ phase }), enabled);
    assert.equal(key("idle"), "footer.checkForUpdates");
    assert.equal(key("checking"), "footer.checkingUpdates");
    assert.equal(key("available"), "footer.updateAvailableShort");
    assert.equal(key("upToDate"), "footer.upToDate");
    assert.equal(key("untracked"), "footer.sourceUpdate.untracked");
    assert.equal(key("starting"), "footer.sourceUpdate.starting");
    assert.equal(key("started"), "footer.sourceUpdate.started");
    assert.equal(key("error"), "footer.sourceUpdate.error");
    assert.equal(key("available", false), "footer.updateCheckingDisabled");
    assert.equal(key("idle", false), "footer.updateCheckingDisabled");
  },

  "footer click action for every phase"() {
    const act = (phase: UpdateState["phase"], enabled = true) =>
      footerAction(state({ phase }), enabled);
    assert.equal(act("idle"), "check");
    assert.equal(act("available"), "openDialog");
    assert.equal(act("upToDate"), "none");
    assert.equal(act("checking"), "none");
    assert.equal(act("starting"), "none");
    assert.equal(act("started"), "check");
    assert.equal(act("error"), "check");
    assert.equal(act("untracked"), "check");
    for (const phase of ["idle", "available", "started", "error"] as const) {
      assert.equal(act(phase, false), "none", `disabled ${phase}`);
    }
  },

  // ---- helpers ---------------------------------------------------------------
  "canConfirmInstall needs an available, installable, unblocked update"() {
    assert.equal(canConfirmInstall(available), true);
    assert.equal(canConfirmInstall(null), false);
    assert.equal(canConfirmInstall(upToDate), false);
    assert.equal(canConfirmInstall(untracked), false);
    assert.equal(canConfirmInstall(info({ can_install: false })), false);
    assert.equal(
      canConfirmInstall(info({ install_blocker: "uncommitted changes" })),
      false,
    );
  },

  "formatChangeList trims, drops blanks, caps at five and counts the rest"() {
    const full = formatChangeList(
      info({
        behind_by: 12,
        changes: ["  a ", "", "b", "   ", "c", "d", "e", "f", "g"],
      }),
    );
    assert.deepEqual(full.lines, ["a", "b", "c", "d", "e"]);
    assert.equal(full.moreCount, 7);
    assert.equal(full.countLabel, "12");

    const small = formatChangeList(info({ behind_by: 2, changes: ["x", "y"] }));
    assert.deepEqual(small.lines, ["x", "y"]);
    assert.equal(small.moreCount, 0);
    assert.equal(small.countLabel, "2");

    const none = formatChangeList(info({ behind_by: 0, changes: [] }));
    assert.deepEqual(none.lines, []);
    assert.equal(none.moreCount, 0);
    assert.equal(none.countLabel, "0");
  },

  "formatChangeList never reports a negative remainder"() {
    const r = formatChangeList(
      info({ behind_by: 1, changes: ["a", "b", "c"] }),
    );
    assert.equal(r.moreCount, 0);
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(
    `${failed} of ${Object.keys(scenarios).length} source-update scenarios failed`,
  );
}
console.log(
  `source update state: ${Object.keys(scenarios).length} scenarios passed`,
);
