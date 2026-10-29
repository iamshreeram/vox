import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import {
  canAddAgentName,
  cleanAgentNames,
  effectivePromptFlag,
  isValidAgentFlag,
  isValidAgentName,
  MAX_AGENT_NAMES,
  resultPanelText,
} from "../../lib/agentJobs";

interface AgentSettingProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const COPY_FEEDBACK_MS = 1500;

/** Allowlist editor for the coding agent names Vox may choose from. */
export const AgentNamesField: React.FC<AgentSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const names = getSetting("agent_jobs_agents") ?? [];
  const [candidate, setCandidate] = useState("");
  const pending = isUpdating("agent_jobs_agents");
  const canAdd = canAddAgentName(names, candidate);
  const showInvalid = candidate !== "" && !isValidAgentName(candidate);
  const atLimit = names.length >= MAX_AGENT_NAMES;

  const save = (next: string[]) => {
    void updateSetting("agent_jobs_agents", cleanAgentNames(next));
  };

  const add = () => {
    if (!canAdd) return;
    save([...names, candidate]);
    setCandidate("");
  };

  return (
    <SettingContainer
      title={t("settings.experimental.jobs.agents.label")}
      description={t("settings.experimental.jobs.agents.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
      layout="stacked"
    >
      <div className="space-y-2">
        <ul className="space-y-1">
          {names.map((name) => (
            <li key={name} className="flex items-center justify-between gap-2">
              <span className="truncate font-mono text-xs">{name}</span>
              <Button
                variant="secondary"
                size="sm"
                disabled={pending}
                onClick={() => save(names.filter((n) => n !== name))}
              >
                {t("settings.experimental.jobs.agents.remove")}
              </Button>
            </li>
          ))}
        </ul>
        <div className="flex items-center gap-2">
          <Input
            type="text"
            variant="compact"
            className="w-40"
            value={candidate}
            placeholder={t(
              "settings.experimental.jobs.agents.inputPlaceholder",
            )}
            disabled={pending || atLimit}
            onChange={(event) => setCandidate(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") add();
            }}
          />
          <Button
            variant="secondary"
            size="sm"
            disabled={!canAdd || pending}
            onClick={add}
          >
            {t("settings.experimental.jobs.agents.add")}
          </Button>
        </div>
        {showInvalid && (
          <p className="text-xs text-danger">
            {t("settings.experimental.jobs.agents.invalid")}
          </p>
        )}
        {atLimit && (
          <p className="text-xs text-mid-gray">
            {t("settings.experimental.jobs.agents.limit", {
              max: MAX_AGENT_NAMES,
            })}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};

/** Flag that passes the chosen agent name to the coding CLI. Shown only when names exist. */
export const AgentFlagField: React.FC<AgentSettingProps> = ({
  descriptionMode = "tooltip",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const names = getSetting("agent_jobs_agents") ?? [];
  const stored = getSetting("agent_jobs_agent_flag") ?? "";
  const promptFlag = effectivePromptFlag(
    getSetting("agent_bridge_prompt_flag") ?? "",
  );
  const [draft, setDraft] = useState(stored);
  const [invalid, setInvalid] = useState(false);
  const pending = isUpdating("agent_jobs_agent_flag");

  // Follows the stored value, which also reverts the draft when a save fails.
  useEffect(() => {
    setDraft(stored);
    setInvalid(false);
  }, [stored]);

  const commit = () => {
    if (draft === stored) return;
    if (!isValidAgentFlag(draft, promptFlag)) {
      setInvalid(true);
      return;
    }
    void updateSetting("agent_jobs_agent_flag", draft);
  };

  if (names.length === 0) return null;

  return (
    <SettingContainer
      title={t("settings.experimental.jobs.flag.label")}
      description={t("settings.experimental.jobs.flag.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <div className="flex flex-col items-end gap-1">
        <Input
          type="text"
          variant="compact"
          className="w-32 font-mono"
          value={draft}
          disabled={pending}
          onChange={(event) => {
            setDraft(event.target.value);
            setInvalid(false);
          }}
          onBlur={commit}
          onKeyDown={(event) => {
            if (event.key === "Enter") commit();
          }}
        />
        {invalid && (
          <p className="max-w-xs text-end text-xs text-danger">
            {t("settings.experimental.jobs.flag.invalid")}
          </p>
        )}
      </div>
    </SettingContainer>
  );
};

/** Plain-text, scrollable panel with the last job result and a copy button. */
export const LastResultPanel: React.FC<{ result: string | null }> = ({
  result,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const text = resultPanelText(result);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), COPY_FEEDBACK_MS);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (text === null) return null;

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between">
        <p className="text-sm font-medium">
          {t("settings.experimental.jobs.result.label")}
        </p>
        <Button variant="secondary" size="sm" onClick={() => void handleCopy()}>
          {copied
            ? t("settings.experimental.jobs.result.copied")
            : t("settings.experimental.jobs.result.copy")}
        </Button>
      </div>
      <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border border-mid-gray/40 bg-mid-gray/10 p-2 font-mono text-xs">
        {text}
      </pre>
    </div>
  );
};
