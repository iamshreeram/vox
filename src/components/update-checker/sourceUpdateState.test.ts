import assert from "node:assert/strict";
import type { SourceUpdateInfo, UpdateProgress } from "../../bindings";
import {
  canConfirmInstall,
  footerAction,
  footerLabelKey,
  formatChangeList,
  initialState,
  milestonesCrossed,
  progressTextKey,
  reducer,
  stageLabelKey,
  type Action,
  type UpdateState,
} from "./sourceUpdateState";

function info(partial: Partial<SourceUpdateInfo> = {}): SourceUpdateInfo {
  return {
    state: "available",
    current_commit: "aaaaaaa",
    latest_commit: "bbbbbbb",
    latest_full: "b".repeat(40),
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

function progress(
  runId: string,
  phase: UpdateProgress["phase"],
  percent: number,
  extra: Partial<UpdateProgress> = {},
): UpdateProgress {
  return {
    run_id: runId,
    phase,
    percent,
    target: "abc1234",
    message: "",
    estimated: false,
    ...extra,
  };
}

function run(actions: Action[], from: UpdateState = initialState): UpdateState {
  return actions.reduce(reducer, from);
}

function state(partial: Partial<UpdateState>): UpdateState {
  return { ...initialState, ...partial };
}

const RUN_A = "a".repeat(16);
const RUN_B = "b".repeat(16);

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

  "pushed status is ignored while checking, starting, updating or updated"() {
    const runState = state({
      phase: "updating",
      info: available,
      run: {
        runId: RUN_A,
        percent: 40,
        backendPhase: "compiling",
        target: "abc1234",
        estimated: false,
        terminal: false,
      },
    });
    for (const phase of [
      "checking",
      "starting",
      "updating",
      "updated",
    ] as const) {
      const before = state({ ...runState, phase });
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

  "a start error returns to error with info kept; stray failures change nothing"() {
    const failed = run(
      [{ type: "installFailed", message: "Could not start" }],
      state({ phase: "starting", info: available }),
    );
    assert.equal(failed.phase, "error");
    assert.equal(failed.error, "Could not start");
    assert.deepEqual(failed.info, available);

    const stray = state({ phase: "idle" });
    assert.deepEqual(
      run([{ type: "installFailed", message: "x" }], stray),
      stray,
    );
  },

  "after confirm the footer waits in starting until the first progress"() {
    const starting = run(
      [{ type: "installStarted" }],
      state({ phase: "available", info: available }),
    );
    assert.equal(starting.phase, "starting");
    const first = run(
      [{ type: "progress", progress: progress(RUN_A, "preparing", 0) }],
      starting,
    );
    assert.equal(first.phase, "updating");
    assert.equal(first.run?.runId, RUN_A);
    assert.equal(first.run?.percent, 0);
    assert.equal(first.run?.backendPhase, "preparing");
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

  // ---- progress: happy path, monotonic, dedupe ------------------------------
  "available -> starting -> updating -> updated -> reverts to up to date"() {
    const steps: UpdateProgress[] = [
      progress(RUN_A, "preparing", 0),
      progress(RUN_A, "dependencies", 10),
      progress(RUN_A, "compiling", 25),
      progress(RUN_A, "bundling", 75),
      progress(RUN_A, "installing", 92),
      progress(RUN_A, "restarting", 98),
      progress(RUN_A, "done", 100),
    ];
    let s = run(
      [{ type: "installStarted" }],
      state({ phase: "available", info: available }),
    );
    for (const p of steps) {
      s = run([{ type: "progress", progress: p }], s);
    }
    assert.equal(s.phase, "updated");
    assert.equal(s.run?.target, "abc1234");
    assert.equal(s.run?.percent, 100);

    const reverted = run([{ type: "updatedExpired" }], s);
    assert.equal(reverted.phase, "upToDate");
    assert.equal(reverted.run?.terminal, true);
  },

  "percent never decreases within a run"() {
    const at50 = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 50) },
    ]);
    const after = run(
      [{ type: "progress", progress: progress(RUN_A, "compiling", 30) }],
      at50,
    );
    assert.deepEqual(after, at50);
  },

  "duplicate events change nothing"() {
    const p = progress(RUN_A, "compiling", 50);
    const once = run([{ type: "progress", progress: p }]);
    const twice = run([{ type: "progress", progress: p }], once);
    assert.deepEqual(twice, once);
  },

  "a new run_id resets the percent"() {
    const first = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 80) },
    ]);
    const second = run(
      [{ type: "progress", progress: progress(RUN_B, "preparing", 10) }],
      first,
    );
    assert.equal(second.run?.runId, RUN_B);
    assert.equal(second.run?.percent, 10);
    assert.equal(second.phase, "updating");
  },

  "an older run_id is ignored even when it reports a higher percent"() {
    const first = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 80) },
    ]);
    const second = run(
      [{ type: "progress", progress: progress(RUN_B, "preparing", 10) }],
      first,
    );
    const late = run(
      [{ type: "progress", progress: progress(RUN_A, "compiling", 90) }],
      second,
    );
    assert.deepEqual(late, second);
  },

  "non-terminal progress after updated of the same run is ignored"() {
    const done = run([
      { type: "progress", progress: progress(RUN_A, "done", 100) },
    ]);
    const late = run(
      [{ type: "progress", progress: progress(RUN_A, "compiling", 50) }],
      done,
    );
    assert.deepEqual(late, done);
    const again = run(
      [{ type: "progress", progress: progress(RUN_A, "done", 100) }],
      done,
    );
    assert.deepEqual(again, done);
  },

  "a duplicate done after the revert does not show updated again"() {
    const reverted = run([
      { type: "progress", progress: progress(RUN_A, "done", 100) },
      { type: "updatedExpired" },
    ]);
    const again = run(
      [{ type: "progress", progress: progress(RUN_A, "done", 100) }],
      reverted,
    );
    assert.equal(again.phase, "upToDate");
  },

  "invalid percentages are ignored or clamped"() {
    const idle = state({ phase: "available", info: available });
    const nan = run(
      [{ type: "progress", progress: progress(RUN_A, "compiling", NaN) }],
      idle,
    );
    assert.deepEqual(nan, idle);
    const high = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 150) },
    ]);
    assert.equal(high.run?.percent, 100);
    const low = run([
      { type: "progress", progress: progress(RUN_A, "preparing", -5) },
    ]);
    assert.equal(low.run?.percent, 0);
  },

  "the estimated flag is carried into the run state"() {
    const s = run([
      {
        type: "progress",
        progress: progress(RUN_A, "compiling", 42, { estimated: true }),
      },
    ]);
    assert.equal(s.run?.estimated, true);
  },

  // ---- adoption (window opened mid-run or after a restart) ----------------
  "adoption: the first progress while idle shows updating"() {
    const s = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 40) },
    ]);
    assert.equal(s.phase, "updating");
    assert.equal(s.run?.percent, 40);
  },

  "adoption: a run that finished before mount shows updated"() {
    const s = run([
      { type: "progress", progress: progress(RUN_A, "done", 100) },
    ]);
    assert.equal(s.phase, "updated");
  },

  "adoption: a run that failed before mount shows failed with its message"() {
    const s = run([
      {
        type: "progress",
        progress: progress(RUN_A, "failed", 60, { message: "Build failed" }),
      },
    ]);
    assert.equal(s.phase, "failed");
    assert.equal(s.error, "Build failed");
  },

  "hydration does not overwrite an adopted run"() {
    const s = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 40) },
      { type: "hydrated", info: available },
    ]);
    assert.equal(s.phase, "updating");
  },

  // ---- failure and retry ---------------------------------------------------
  "failed keeps the message verbatim and ignores later progress of that run"() {
    const markup = "<img src=x onerror=alert(1)> <b>bold</b>";
    const failed = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 60) },
      {
        type: "progress",
        progress: progress(RUN_A, "failed", 60, { message: markup }),
      },
    ]);
    assert.equal(failed.phase, "failed");
    assert.equal(failed.error, markup);
    const late = run(
      [{ type: "progress", progress: progress(RUN_A, "compiling", 70) }],
      failed,
    );
    assert.deepEqual(late, failed);
  },

  "failed -> retry -> available"() {
    const failed = run([
      {
        type: "progress",
        progress: progress(RUN_A, "failed", 60, { message: "x" }),
      },
    ]);
    const retrying = run([{ type: "checkStarted", manual: true }], failed);
    assert.equal(retrying.phase, "checking");
    assert.equal(retrying.error, null);
    const back = run([{ type: "checkSucceeded", info: available }], retrying);
    assert.equal(back.phase, "available");
    assert.deepEqual(back.info, available);
  },

  // ---- disable & expiry ------------------------------------------------------
  "disabling checks resets everything, even mid-flow"() {
    for (const phase of [
      "checking",
      "available",
      "starting",
      "updating",
      "updated",
      "failed",
      "error",
    ] as const) {
      const s = run(
        [{ type: "checksDisabled" }],
        state({ phase, info: available, error: "e" }),
      );
      assert.deepEqual(s, initialState, phase);
    }
  },

  "the updated state expires back to up to date only through updatedExpired"() {
    const done = run([
      { type: "progress", progress: progress(RUN_A, "done", 100) },
    ]);
    assert.equal(run([{ type: "notificationExpired" }], done).phase, "updated");
    assert.equal(run([{ type: "updatedExpired" }], done).phase, "upToDate");
    const other = state({ phase: "available", info: available });
    assert.deepEqual(run([{ type: "updatedExpired" }], other), other);
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
    assert.equal(key("updating"), "footer.sourceUpdate.updating");
    assert.equal(key("updated"), "footer.sourceUpdate.updated");
    assert.equal(key("failed"), "footer.sourceUpdate.failed");
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
    assert.equal(act("updating"), "none");
    assert.equal(act("updated"), "none");
    // Retry is its own button inside the failed item, so the item is not one.
    assert.equal(act("failed"), "none");
    assert.equal(act("error"), "check");
    assert.equal(act("untracked"), "check");
    for (const phase of [
      "idle",
      "available",
      "error",
      "updating",
      "failed",
    ] as const) {
      assert.equal(act(phase, false), "none", `disabled ${phase}`);
    }
  },

  "stage labels exist for every backend phase"() {
    assert.equal(
      stageLabelKey("preparing"),
      "footer.sourceUpdate.stage.preparing",
    );
    assert.equal(
      stageLabelKey("dependencies"),
      "footer.sourceUpdate.stage.dependencies",
    );
    assert.equal(
      stageLabelKey("compiling"),
      "footer.sourceUpdate.stage.compiling",
    );
    assert.equal(
      stageLabelKey("bundling"),
      "footer.sourceUpdate.stage.bundling",
    );
    assert.equal(
      stageLabelKey("installing"),
      "footer.sourceUpdate.stage.installing",
    );
    assert.equal(
      stageLabelKey("restarting"),
      "footer.sourceUpdate.stage.restarting",
    );
  },

  "estimated progress uses the estimated text key"() {
    const exact = run([
      { type: "progress", progress: progress(RUN_A, "compiling", 42) },
    ]);
    assert.equal(progressTextKey(exact.run), "footer.sourceUpdate.updating");
    const estimated = run([
      {
        type: "progress",
        progress: progress(RUN_A, "compiling", 42, { estimated: true }),
      },
    ]);
    assert.equal(
      progressTextKey(estimated.run),
      "footer.sourceUpdate.updatingEstimated",
    );
  },

  "live region milestones fire once per 25 percent bucket"() {
    assert.deepEqual(milestonesCrossed(-1, 0), [0]);
    assert.deepEqual(milestonesCrossed(0, 0), []);
    assert.deepEqual(milestonesCrossed(0, 30), [25]);
    assert.deepEqual(milestonesCrossed(0, 60), [25, 50]);
    assert.deepEqual(milestonesCrossed(-1, 100), [0, 25, 50, 75, 100]);
    assert.deepEqual(milestonesCrossed(100, 100), []);
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
