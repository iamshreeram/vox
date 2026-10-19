import React, { useEffect, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../../ui/Button";
import {
  ENROLL_STEPS,
  cancelTargetOf,
  countdownSeconds,
  enrollmentReducer,
  failureFromCmdError,
  initialEnrollState,
  type EnrollEvent,
  type EnrollMessage,
  type EnrollState,
} from "./enrollmentState";

interface SpeakerEnrollWizardProps {
  /** The speaker model is Ready. Enrollment cannot start without it. */
  modelReady: boolean;
  /** Called after a voiceprint was saved, so the caller can refresh its status. */
  onFinished: () => void;
}

const TICK_MS = 250;

/**
 * Three counted recordings, each a 6 s window, then Finish. The rules live in
 * enrollmentState.ts. This component only calls the backend when the reducer
 * accepts the move, and it cancels any live session on unmount and page hide.
 * Audio never reaches this component; the backend returns numbers only.
 */
export const SpeakerEnrollWizard: React.FC<SpeakerEnrollWizardProps> = ({
  modelReady,
  onFinished,
}) => {
  const { t } = useTranslation();
  const [state, dispatch] = useReducer(
    enrollmentReducer,
    undefined,
    initialEnrollState,
  );
  // Mirrors the latest state so async handlers read the live session.
  const stateRef = useRef<EnrollState>(state);
  stateRef.current = state;
  // Set when the user cancels while the start call is still in flight.
  const cancelBeforeBeginRef = useRef(false);
  const stepStartedAtRef = useRef(0);
  const [now, setNow] = useState(0);

  useEffect(() => {
    const cancelLive = () => {
      const target = cancelTargetOf(stateRef.current);
      if (stateRef.current.phase === "starting") {
        cancelBeforeBeginRef.current = true;
      }
      if (target !== null) void commands.speakerEnrollCancel(target);
    };
    window.addEventListener("pagehide", cancelLive);
    window.addEventListener("beforeunload", cancelLive);
    return () => {
      window.removeEventListener("pagehide", cancelLive);
      window.removeEventListener("beforeunload", cancelLive);
      cancelLive();
    };
  }, []);

  // Live countdown while a recording window is open.
  useEffect(() => {
    if (state.phase !== "stepping") return;
    const timer = setInterval(() => setNow(Date.now()), TICK_MS);
    return () => clearInterval(timer);
  }, [state.phase]);

  const moves = (event: EnrollEvent): boolean =>
    enrollmentReducer(stateRef.current, event).phase !== stateRef.current.phase;

  const send = (event: EnrollEvent) => dispatch(event);

  const start = async () => {
    if (!moves({ type: "BEGIN" })) return;
    cancelBeforeBeginRef.current = false;
    send({ type: "BEGIN" });
    try {
      const result = await commands.speakerEnrollBegin();
      if (result.status === "ok") {
        if (cancelBeforeBeginRef.current) {
          void commands.speakerEnrollCancel(result.data);
          return;
        }
        send({ type: "BEGUN", sessionId: result.data });
      } else {
        send({
          type: "BEGIN_FAILED",
          failure: failureFromCmdError(result.error),
        });
      }
    } catch (err) {
      console.warn("Starting enrollment failed:", err);
      send({ type: "BEGIN_FAILED", failure: "StoreFailed" });
    }
  };

  const recordStep = async () => {
    if (!moves({ type: "RECORD" })) return;
    const { sessionId, step } = stateRef.current;
    if (sessionId === null) return;
    stepStartedAtRef.current = Date.now();
    setNow(stepStartedAtRef.current);
    send({ type: "RECORD" });
    try {
      const result = await commands.speakerEnrollStep(sessionId, step);
      if (result.status === "ok" && result.data.ok) {
        send({ type: "STEP_OK", sessionId });
      } else if (result.status === "ok") {
        send({ type: "STEP_FAILED", sessionId, failure: "EmbedFailed" });
      } else {
        send({
          type: "STEP_FAILED",
          sessionId,
          failure: failureFromCmdError(result.error),
        });
      }
    } catch (err) {
      console.warn("Recording a step failed:", err);
      send({ type: "STEP_FAILED", sessionId, failure: "StoreFailed" });
    }
  };

  const finish = async () => {
    if (!moves({ type: "FINISH" })) return;
    const sessionId = stateRef.current.sessionId;
    if (sessionId === null) return;
    send({ type: "FINISH" });
    try {
      const result = await commands.speakerEnrollFinish(sessionId);
      if (result.status === "ok") {
        send({ type: "FINISHED", sessionId });
        onFinished();
      } else {
        send({
          type: "FINISH_FAILED",
          sessionId,
          failure: failureFromCmdError(result.error),
        });
      }
    } catch (err) {
      console.warn("Finishing enrollment failed:", err);
      send({ type: "FINISH_FAILED", sessionId, failure: "StoreFailed" });
    }
  };

  const cancel = () => {
    const target = cancelTargetOf(stateRef.current);
    if (stateRef.current.phase === "starting") {
      cancelBeforeBeginRef.current = true;
    }
    send({ type: "CANCEL" });
    if (target !== null) void commands.speakerEnrollCancel(target);
  };

  const messageText = (message: EnrollMessage): string => {
    switch (message) {
      case "busy":
        return t("converse.voice.speaker.enroll.messages.busy");
      case "too_little_speech":
        return t("converse.voice.speaker.enroll.messages.too_little_speech");
      case "inconsistent":
        return t("converse.voice.speaker.enroll.messages.inconsistent");
      case "model_missing":
        return t("converse.voice.speaker.enroll.messages.model_missing");
      case "session_ended":
        return t("converse.voice.speaker.enroll.messages.session_ended");
      default:
        return t("converse.voice.speaker.enroll.messages.generic");
    }
  };

  const promptsFor = (step: number): string[] => {
    switch (step) {
      case 0:
        return [
          t("converse.voice.speaker.enroll.prompts.s1a"),
          t("converse.voice.speaker.enroll.prompts.s1b"),
          t("converse.voice.speaker.enroll.prompts.s1c"),
        ];
      case 1:
        return [
          t("converse.voice.speaker.enroll.prompts.s2a"),
          t("converse.voice.speaker.enroll.prompts.s2b"),
          t("converse.voice.speaker.enroll.prompts.s2c"),
        ];
      default:
        return [
          t("converse.voice.speaker.enroll.prompts.s3a"),
          t("converse.voice.speaker.enroll.prompts.s3b"),
          t("converse.voice.speaker.enroll.prompts.s3c"),
        ];
    }
  };

  const { phase, step, okSteps, message } = state;
  const inWizard =
    phase === "ready" || phase === "stepping" || phase === "finishing";
  const seconds = countdownSeconds(
    stepStartedAtRef.current === 0 ? 0 : now - stepStartedAtRef.current,
  );

  return (
    <div className="flex flex-col gap-3">
      {phase === "done" && (
        <p className="text-sm">{t("converse.voice.speaker.enroll.done")}</p>
      )}

      {(phase === "idle" || phase === "done") && (
        <div className="flex flex-col gap-2">
          <p className="text-xs text-mid-gray">
            {t("converse.voice.speaker.enroll.intro")}
          </p>
          {!modelReady && (
            <p className="text-xs text-mid-gray">
              {t("converse.voice.speaker.enroll.needModel")}
            </p>
          )}
          <Button
            variant="primary"
            size="md"
            onClick={() => void start()}
            disabled={!modelReady}
            className="self-start"
          >
            {t("converse.voice.speaker.enroll.start")}
          </Button>
        </div>
      )}

      {phase === "starting" && (
        <div className="flex items-center gap-2">
          <span className="text-sm text-mid-gray">
            {t("converse.voice.speaker.enroll.starting")}
          </span>
          <Button variant="secondary" size="md" onClick={cancel}>
            {t("converse.voice.speaker.enroll.cancel")}
          </Button>
        </div>
      )}

      {inWizard && (
        <div className="flex flex-col gap-3">
          <p className="text-sm">
            {t("converse.voice.speaker.enroll.progress", {
              done: okSteps,
              total: ENROLL_STEPS,
            })}
          </p>
          {okSteps < ENROLL_STEPS && (
            <div className="flex flex-col gap-2 rounded-lg border border-mid-gray/30 p-3">
              <span className="text-sm font-medium">
                {t("converse.voice.speaker.enroll.stepTitle", {
                  step: step + 1,
                  total: ENROLL_STEPS,
                })}
              </span>
              <span className="text-xs text-mid-gray">
                {t("converse.voice.speaker.enroll.readThese")}
              </span>
              <ul className="list-disc ps-5 text-sm">
                {promptsFor(step).map((sentence) => (
                  <li key={sentence}>{sentence}</li>
                ))}
              </ul>
              {phase === "stepping" ? (
                <span role="timer" className="text-sm font-medium">
                  {t("converse.voice.speaker.enroll.recording", { seconds })}
                </span>
              ) : (
                <Button
                  variant="primary"
                  size="md"
                  onClick={() => void recordStep()}
                  disabled={phase !== "ready"}
                  className="self-start"
                >
                  {t("converse.voice.speaker.enroll.record")}
                </Button>
              )}
            </div>
          )}
          {okSteps === ENROLL_STEPS && phase === "ready" && (
            <p className="text-xs text-mid-gray">
              {t("converse.voice.speaker.enroll.allRecorded")}
            </p>
          )}
          <div className="flex items-center gap-2">
            <Button
              variant="primary"
              size="md"
              onClick={() => void finish()}
              disabled={phase !== "ready" || okSteps !== ENROLL_STEPS}
            >
              {t("converse.voice.speaker.enroll.finish")}
            </Button>
            <Button
              variant="secondary"
              size="md"
              onClick={cancel}
              disabled={phase === "finishing"}
            >
              {t("converse.voice.speaker.enroll.cancel")}
            </Button>
          </div>
        </div>
      )}

      {message !== null && (
        <p role="alert" className="text-xs text-red-400">
          {messageText(message)}
        </p>
      )}
    </div>
  );
};
