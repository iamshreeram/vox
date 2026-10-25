import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type StepAgentProfile } from "@/bindings";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";
import type { AcpFormError } from "./acpAgentForm";
import {
  formatStepAgentProfile,
  parseStepAgentDraft,
  type StepAgentDraft,
  type StepAgentType,
  STEP_AGENT_TYPES,
  sameStepAgentProfile,
} from "./stepAgentsForm";

type RowError =
  | { kind: "form"; code: AcpFormError }
  | { kind: "backend"; message: string };

interface StepAgentRowProps {
  stepType: StepAgentType;
  descriptionMode: "inline" | "tooltip";
  grouped: boolean;
}

const StepAgentRow: React.FC<StepAgentRowProps> = ({
  stepType,
  descriptionMode,
  grouped,
}) => {
  const { t } = useTranslation();
  const { getSetting, isUpdating } = useSettings();

  const stored: StepAgentProfile | null =
    getSetting("acp_step_agents")?.[stepType] ?? null;
  const storedCommand = stored?.command ?? "";
  const storedArgs = stored?.args ?? "";
  const updateKey = `acp_step_agents.${stepType}`;
  const busy = isUpdating(updateKey);

  const [draft, setDraft] = useState<StepAgentDraft>(() =>
    formatStepAgentProfile(stored),
  );
  const [error, setError] = useState<RowError | null>(null);

  useEffect(() => {
    setDraft({ command: storedCommand, args: storedArgs });
  }, [storedCommand, storedArgs]);

  // Literal keys only, so the settings search index can see every string.
  const text = (() => {
    switch (stepType) {
      case "research":
        return {
          label: t("settings.advanced.acpAgent.stepAgents.research.label"),
          description: t(
            "settings.advanced.acpAgent.stepAgents.research.description",
          ),
        };
      case "browser":
        return {
          label: t("settings.advanced.acpAgent.stepAgents.browser.label"),
          description: t(
            "settings.advanced.acpAgent.stepAgents.browser.description",
          ),
        };
      case "code":
        return {
          label: t("settings.advanced.acpAgent.stepAgents.code.label"),
          description: t(
            "settings.advanced.acpAgent.stepAgents.code.description",
          ),
        };
      case "files":
        return {
          label: t("settings.advanced.acpAgent.stepAgents.files.label"),
          description: t(
            "settings.advanced.acpAgent.stepAgents.files.description",
          ),
        };
    }
  })();

  const formErrorText = (code: AcpFormError): string => {
    switch (code) {
      case "notAbsolute":
        return t("settings.advanced.acpAgent.errors.notAbsolute");
      case "tooLong":
        return t("settings.advanced.acpAgent.errors.tooLong");
      case "controlChars":
        return t("settings.advanced.acpAgent.errors.controlChars");
      case "tooMany":
        return t("settings.advanced.acpAgent.errors.tooMany");
      case "argTooLong":
        return t("settings.advanced.acpAgent.errors.argTooLong");
      case "outOfRange":
        return t("settings.advanced.acpAgent.errors.outOfRange");
      case "notInteger":
        return t("settings.advanced.acpAgent.errors.notInteger");
    }
  };

  const errorMessage =
    error === null
      ? null
      : error.kind === "form"
        ? formErrorText(error.code)
        : error.message;

  // Writes the slot into the store at once; the backend result decides whether it stays.
  const setStoredSlot = (value: StepAgentProfile | null) => {
    useSettingsStore.setState((state) => {
      if (state.settings === null) return {};
      const current = state.settings.acp_step_agents ?? {};
      return {
        settings: {
          ...state.settings,
          acp_step_agents: { ...current, [stepType]: value },
        },
      };
    });
  };

  // An invalid draft reverts to the stored value and shows the error. A backend
  // rejection reverts the store and shows the backend's own message.
  const commit = async () => {
    const parsed = parseStepAgentDraft(draft);
    if (!parsed.ok) {
      setError({ kind: "form", code: parsed.error });
      setDraft({ command: storedCommand, args: storedArgs });
      return;
    }
    setError(null);
    if (sameStepAgentProfile(parsed.value, stored)) return;

    const next = parsed.value;
    const previous = stored;
    const { setUpdating } = useSettingsStore.getState();
    setUpdating(updateKey, true);
    setStoredSlot(next);

    let failure: string | null = null;
    try {
      const result = await commands.changeAcpStepAgentSetting(
        stepType,
        next?.command ?? null,
        next?.args ?? "",
      );
      if (result.status === "error") failure = result.error;
    } catch (cause) {
      failure = cause instanceof Error ? cause.message : String(cause);
    } finally {
      setUpdating(updateKey, false);
    }

    if (failure !== null) {
      setStoredSlot(previous);
      setDraft({
        command: previous?.command ?? "",
        args: previous?.args ?? "",
      });
      setError({ kind: "backend", message: failure });
    }
  };

  const blurOnEnter = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") event.currentTarget.blur();
  };

  return (
    <SettingContainer
      title={text.label}
      description={text.description}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <div className="flex flex-col items-end gap-1">
        <Input
          type="text"
          value={draft.command}
          placeholder={t(
            "settings.advanced.acpAgent.stepAgents.command.placeholder",
          )}
          onChange={(event) => {
            setDraft({ ...draft, command: event.target.value });
            setError(null);
          }}
          onBlur={() => void commit()}
          onKeyDown={blurOnEnter}
          aria-label={`${text.label}: ${t("settings.advanced.acpAgent.stepAgents.command.label")}`}
          disabled={busy}
          variant="compact"
          className="w-72 font-mono"
        />
        <Input
          type="text"
          value={draft.args}
          placeholder={t(
            "settings.advanced.acpAgent.stepAgents.args.placeholder",
          )}
          onChange={(event) => {
            setDraft({ ...draft, args: event.target.value });
            setError(null);
          }}
          onBlur={() => void commit()}
          onKeyDown={blurOnEnter}
          aria-label={`${text.label}: ${t("settings.advanced.acpAgent.stepAgents.args.label")}`}
          disabled={busy}
          variant="compact"
          className="w-72 font-mono"
        />
        {errorMessage !== null && (
          <p role="alert" className="max-w-xs text-end text-xs text-red-500">
            {errorMessage}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};

/**
 * Settings > Agents: the optional specialist agent for each step type. General steps have
 * no row because they always use the main agent. Blank rows use the main agent.
 */
export const StepAgentSettings: React.FC<{
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}> = ({ descriptionMode = "tooltip", grouped = false }) => {
  const { t } = useTranslation();
  return (
    <>
      <div className="px-4 pt-3">
        <p className="text-sm font-medium">
          {t("settings.advanced.acpAgent.stepAgents.title")}
        </p>
        <p className="text-xs text-mid-gray">
          {t("settings.advanced.acpAgent.stepAgents.description")}
        </p>
      </div>
      {STEP_AGENT_TYPES.map((stepType) => (
        <StepAgentRow
          key={stepType}
          stepType={stepType}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      ))}
    </>
  );
};
