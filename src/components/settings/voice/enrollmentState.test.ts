import { describe, expect, it } from "vitest";
import {
  ENROLL_STEPS,
  ENROLL_WINDOW_MS,
  cancelTargetOf,
  countdownSeconds,
  enrollmentReducer,
  failureFromCmdError,
  initialEnrollState,
  type EnrollEvent,
  type EnrollPhase,
  type EnrollState,
} from "./enrollmentState";

const PHASES: EnrollPhase[] = [
  "idle",
  "starting",
  "ready",
  "stepping",
  "finishing",
  "done",
];
const SID = 7;
const OTHER = 99;

function stateIn(phase: EnrollPhase, over: Partial<EnrollState> = {}) {
  const hasSession =
    phase === "ready" || phase === "stepping" || phase === "finishing";
  const base: EnrollState = {
    phase,
    sessionId: hasSession ? SID : null,
    step: 0,
    okSteps: 0,
    message: null,
  };
  return { ...base, ...over };
}

const EVENTS: EnrollEvent[] = [
  { type: "BEGIN" },
  { type: "BEGUN", sessionId: SID },
  { type: "BEGIN_FAILED", failure: "Busy" },
  { type: "RECORD" },
  { type: "STEP_OK", sessionId: SID },
  { type: "STEP_FAILED", sessionId: SID, failure: "TooLittleSpeech" },
  { type: "FINISH" },
  { type: "FINISHED", sessionId: SID },
  { type: "FINISH_FAILED", sessionId: SID, failure: "Inconsistent" },
  { type: "CANCEL" },
];

const EVENT_NAMES = [
  "BEGIN",
  "BEGUN",
  "BEGIN_FAILED",
  "RECORD",
  "STEP_OK",
  "STEP_FAILED",
  "FINISH",
  "FINISHED",
  "FINISH_FAILED",
  "CANCEL",
] as const;

// Expected next phase for every (phase, event) pair. Event order = EVENT_NAMES.
const TABLE: Record<EnrollPhase, EnrollPhase[]> = {
  //            BEGIN      BEGUN      BEGIN_F    RECORD     STEP_OK    STEP_F     FINISH     FINISHED   FINISH_F   CANCEL
  idle: [
    "starting",
    "idle",
    "idle",
    "idle",
    "idle",
    "idle",
    "idle",
    "idle",
    "idle",
    "idle",
  ],
  starting: [
    "starting",
    "ready",
    "idle",
    "starting",
    "starting",
    "starting",
    "starting",
    "starting",
    "starting",
    "idle",
  ],
  ready: [
    "ready",
    "ready",
    "ready",
    "stepping",
    "ready",
    "ready",
    "ready",
    "ready",
    "ready",
    "idle",
  ],
  stepping: [
    "stepping",
    "stepping",
    "stepping",
    "stepping",
    "ready",
    "ready",
    "stepping",
    "stepping",
    "stepping",
    "idle",
  ],
  finishing: [
    "finishing",
    "finishing",
    "finishing",
    "finishing",
    "finishing",
    "finishing",
    "finishing",
    "done",
    "ready",
    "idle",
  ],
  done: [
    "starting",
    "done",
    "done",
    "done",
    "done",
    "done",
    "done",
    "done",
    "done",
    "idle",
  ],
};

describe("enrollment transition table: every state x every event", () => {
  for (const phase of PHASES) {
    for (const [index, event] of EVENTS.entries()) {
      const name = EVENT_NAMES[index];
      it(`${phase} + ${name} -> ${TABLE[phase][index]}`, () => {
        const next = enrollmentReducer(stateIn(phase), event);
        expect(next.phase).toBe(TABLE[phase][index]);
      });
    }
  }
});

describe("start and begin", () => {
  it("BEGIN from idle and done goes to starting with a clean slate", () => {
    const next = enrollmentReducer(
      stateIn("done", { okSteps: 3, message: "generic" }),
      { type: "BEGIN" },
    );
    expect(next).toEqual({
      phase: "starting",
      sessionId: null,
      step: 0,
      okSteps: 0,
      message: null,
    });
  });

  it("BEGUN records the session and starts at step 0", () => {
    const next = enrollmentReducer(stateIn("starting"), {
      type: "BEGUN",
      sessionId: SID,
    });
    expect(next).toEqual({
      phase: "ready",
      sessionId: SID,
      step: 0,
      okSteps: 0,
      message: null,
    });
  });

  it("BEGIN_FAILED maps Busy, ModelMissing and others to messages", () => {
    const busy = enrollmentReducer(stateIn("starting"), {
      type: "BEGIN_FAILED",
      failure: "Busy",
    });
    expect(busy.phase).toBe("idle");
    expect(busy.message).toBe("busy");
    expect(busy.sessionId).toBeNull();

    const missing = enrollmentReducer(stateIn("starting"), {
      type: "BEGIN_FAILED",
      failure: "ModelMissing",
    });
    expect(missing.message).toBe("model_missing");

    const other = enrollmentReducer(stateIn("starting"), {
      type: "BEGIN_FAILED",
      failure: "StoreFailed",
    });
    expect(other.message).toBe("generic");
  });
});

describe("steps", () => {
  it("a successful step advances the step index and the ok count", () => {
    const next = enrollmentReducer(stateIn("stepping"), {
      type: "STEP_OK",
      sessionId: SID,
    });
    expect(next.phase).toBe("ready");
    expect(next.step).toBe(1);
    expect(next.okSteps).toBe(1);
    expect(next.message).toBeNull();
  });

  it("retry after TooLittleSpeech keeps the step index and the ok count", () => {
    const at1 = stateIn("ready", { step: 1, okSteps: 1 });
    const failed = enrollmentReducer(
      enrollmentReducer(at1, { type: "RECORD" }),
      { type: "STEP_FAILED", sessionId: SID, failure: "TooLittleSpeech" },
    );
    expect(failed.phase).toBe("ready");
    expect(failed.step).toBe(1);
    expect(failed.okSteps).toBe(1);
    expect(failed.message).toBe("too_little_speech");

    const retried = enrollmentReducer(failed, { type: "RECORD" });
    expect(retried.phase).toBe("stepping");
    expect(retried.step).toBe(1);
  });

  it("Busy and other step failures keep the step and say why", () => {
    const busy = enrollmentReducer(
      stateIn("stepping", { step: 2, okSteps: 2 }),
      {
        type: "STEP_FAILED",
        sessionId: SID,
        failure: "Busy",
      },
    );
    expect(busy).toMatchObject({ phase: "ready", step: 2, okSteps: 2 });
    expect(busy.message).toBe("busy");

    const embed = enrollmentReducer(stateIn("stepping"), {
      type: "STEP_FAILED",
      sessionId: SID,
      failure: "EmbedFailed",
    });
    expect(embed).toMatchObject({ phase: "ready", step: 0 });
    expect(embed.message).toBe("generic");
  });

  it("a cancelled step ends the session quietly", () => {
    const next = enrollmentReducer(stateIn("stepping"), {
      type: "STEP_FAILED",
      sessionId: SID,
      failure: "Cancelled",
    });
    expect(next).toEqual({
      phase: "idle",
      sessionId: null,
      step: 0,
      okSteps: 0,
      message: null,
    });
  });

  it("a lost session ends the wizard with a session_ended message", () => {
    const next = enrollmentReducer(stateIn("stepping"), {
      type: "STEP_FAILED",
      sessionId: SID,
      failure: "StaleSession",
    });
    expect(next.phase).toBe("idle");
    expect(next.message).toBe("session_ended");
  });

  it("RECORD is ignored once all steps are recorded", () => {
    const full = stateIn("ready", {
      step: ENROLL_STEPS,
      okSteps: ENROLL_STEPS,
    });
    expect(enrollmentReducer(full, { type: "RECORD" })).toEqual(full);
  });

  it("an ok step with no step in flight is ignored (out of order)", () => {
    const ready = stateIn("ready", { step: 0, okSteps: 0 });
    expect(
      enrollmentReducer(ready, { type: "STEP_OK", sessionId: SID }),
    ).toEqual(ready);
  });
});

describe("finish", () => {
  it("FINISH is ignored until three ok steps exist", () => {
    const two = stateIn("ready", { step: 2, okSteps: 2 });
    expect(enrollmentReducer(two, { type: "FINISH" })).toEqual(two);
  });

  it("FINISH after three ok steps moves to finishing", () => {
    const three = stateIn("ready", { step: 3, okSteps: 3 });
    expect(enrollmentReducer(three, { type: "FINISH" }).phase).toBe(
      "finishing",
    );
  });

  it("FINISHED ends the wizard and drops the session", () => {
    const next = enrollmentReducer(
      stateIn("finishing", { step: 3, okSteps: 3 }),
      { type: "FINISHED", sessionId: SID },
    );
    expect(next.phase).toBe("done");
    expect(next.sessionId).toBeNull();
    expect(next.message).toBeNull();
  });

  it("Inconsistent restarts the steps from step 0 with a message", () => {
    const next = enrollmentReducer(
      stateIn("finishing", { step: 3, okSteps: 3 }),
      { type: "FINISH_FAILED", sessionId: SID, failure: "Inconsistent" },
    );
    expect(next).toEqual({
      phase: "ready",
      sessionId: SID,
      step: 0,
      okSteps: 0,
      message: "inconsistent",
    });
  });

  it("Busy and store failures keep the session for a retry", () => {
    const busy = enrollmentReducer(
      stateIn("finishing", { step: 3, okSteps: 3 }),
      { type: "FINISH_FAILED", sessionId: SID, failure: "Busy" },
    );
    expect(busy).toMatchObject({ phase: "ready", sessionId: SID, okSteps: 3 });
    expect(busy.message).toBe("busy");

    const store = enrollmentReducer(
      stateIn("finishing", { step: 3, okSteps: 3 }),
      { type: "FINISH_FAILED", sessionId: SID, failure: "StoreFailed" },
    );
    expect(store).toMatchObject({ phase: "ready", sessionId: SID });
    expect(store.message).toBe("generic");
  });

  it("a lost session during finish ends the wizard", () => {
    const next = enrollmentReducer(
      stateIn("finishing", { step: 3, okSteps: 3 }),
      { type: "FINISH_FAILED", sessionId: SID, failure: "NoActiveSession" },
    );
    expect(next.phase).toBe("idle");
    expect(next.message).toBe("session_ended");
  });
});

describe("stale session ids are ignored", () => {
  for (const phase of ["ready", "stepping", "finishing"] as const) {
    it(`${phase} ignores results from another session`, () => {
      const before = stateIn(phase, { step: 1, okSteps: 1 });
      const events: EnrollEvent[] = [
        { type: "STEP_OK", sessionId: OTHER },
        { type: "STEP_FAILED", sessionId: OTHER, failure: "Busy" },
        { type: "FINISHED", sessionId: OTHER },
        { type: "FINISH_FAILED", sessionId: OTHER, failure: "Inconsistent" },
      ];
      for (const event of events) {
        expect(enrollmentReducer(before, event)).toEqual(before);
      }
    });
  }
});

describe("cancel", () => {
  for (const phase of PHASES) {
    it(`cancel from ${phase} ends idle with no session`, () => {
      const next = enrollmentReducer(
        stateIn(phase, { step: 2, okSteps: 2, message: "busy" }),
        { type: "CANCEL" },
      );
      expect(next).toEqual({
        phase: "idle",
        sessionId: null,
        step: 0,
        okSteps: 0,
        message: null,
      });
    });
  }

  it("cancelTargetOf names the session only while one is live", () => {
    expect(cancelTargetOf(stateIn("ready"))).toBe(SID);
    expect(cancelTargetOf(stateIn("stepping"))).toBe(SID);
    expect(cancelTargetOf(stateIn("finishing"))).toBe(SID);
    expect(cancelTargetOf(stateIn("starting"))).toBeNull();
    expect(cancelTargetOf(stateIn("idle"))).toBeNull();
    expect(cancelTargetOf(stateIn("done"))).toBeNull();
  });
});

describe("full path and helpers", () => {
  it("happy path: begin, three steps, finish", () => {
    let s = initialEnrollState();
    s = enrollmentReducer(s, { type: "BEGIN" });
    s = enrollmentReducer(s, { type: "BEGUN", sessionId: SID });
    for (let i = 0; i < ENROLL_STEPS; i++) {
      s = enrollmentReducer(s, { type: "RECORD" });
      s = enrollmentReducer(s, { type: "STEP_OK", sessionId: SID });
    }
    expect(s.okSteps).toBe(ENROLL_STEPS);
    s = enrollmentReducer(s, { type: "FINISH" });
    s = enrollmentReducer(s, { type: "FINISHED", sessionId: SID });
    expect(s.phase).toBe("done");
  });

  it("failureFromCmdError reads the three command error shapes", () => {
    expect(failureFromCmdError("ModelMissing")).toBe("ModelMissing");
    expect(failureFromCmdError("StoreFailed")).toBe("StoreFailed");
    expect(failureFromCmdError({ Enroll: "Busy" })).toBe("Busy");
    expect(failureFromCmdError({ Enroll: "TooLittleSpeech" })).toBe(
      "TooLittleSpeech",
    );
  });

  it("the window and the step count match the backend", () => {
    expect(ENROLL_STEPS).toBe(3);
    expect(ENROLL_WINDOW_MS).toBe(6000);
  });

  it("countdown shows whole seconds left, never negative, never above 6", () => {
    expect(countdownSeconds(0)).toBe(6);
    expect(countdownSeconds(1)).toBe(6);
    expect(countdownSeconds(1000)).toBe(5);
    expect(countdownSeconds(5999)).toBe(1);
    expect(countdownSeconds(6000)).toBe(0);
    expect(countdownSeconds(9000)).toBe(0);
    expect(countdownSeconds(-50)).toBe(6);
    expect(countdownSeconds(Number.NaN)).toBe(6);
  });
});
