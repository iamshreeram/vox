import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";

// Bounds mirror AGENT_BRIDGE_TIMEOUT_MIN_SECS / AGENT_BRIDGE_TIMEOUT_MAX_SECS
// in settings.rs. The backend is authoritative; this only avoids a round trip.
const TIMEOUT_MIN_SECS = 5;
const TIMEOUT_MAX_SECS = 600;

interface AgentBridgeAdvancedSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AgentBridgeAdvancedSettings: React.FC<AgentBridgeAdvancedSettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const agentBridgeEnabled = getSetting("agent_bridge_enabled") ?? false;
    const timeoutSecs = getSetting("agent_bridge_timeout_secs") ?? 60;
    const promptFlag = getSetting("agent_bridge_prompt_flag") ?? "-p";

    const [timeoutDraft, setTimeoutDraft] = useState(String(timeoutSecs));
    const [flagDraft, setFlagDraft] = useState(promptFlag);

    useEffect(() => {
      setTimeoutDraft(String(timeoutSecs));
    }, [timeoutSecs]);

    useEffect(() => {
      setFlagDraft(promptFlag);
    }, [promptFlag]);

    if (!agentBridgeEnabled) return null;

    // updateSetting reverts the store on a backend rejection. Re-read the stored
    // value afterwards so the field shows what was actually kept.
    const storedNow = <
      K extends "agent_bridge_timeout_secs" | "agent_bridge_prompt_flag",
    >(
      key: K,
    ) => useSettingsStore.getState().settings?.[key];

    const commitTimeout = async () => {
      const parsed = Number(timeoutDraft);
      const valid =
        Number.isInteger(parsed) &&
        parsed >= TIMEOUT_MIN_SECS &&
        parsed <= TIMEOUT_MAX_SECS;
      if (!valid) {
        setTimeoutDraft(String(timeoutSecs));
        return;
      }
      if (parsed === timeoutSecs) return;
      await updateSetting("agent_bridge_timeout_secs", parsed);
      setTimeoutDraft(
        String(storedNow("agent_bridge_timeout_secs") ?? timeoutSecs),
      );
    };

    const commitFlag = async () => {
      if (flagDraft === promptFlag) return;
      await updateSetting("agent_bridge_prompt_flag", flagDraft);
      setFlagDraft(String(storedNow("agent_bridge_prompt_flag") ?? promptFlag));
    };

    return (
      <>
        <p className="px-4 pt-2 text-xs font-medium text-mid-gray">
          {t("settings.advanced.agentBridge.advancedLabel")}
        </p>
        <SettingContainer
          title={t("settings.advanced.agentBridge.timeoutLabel")}
          description={t("settings.advanced.agentBridge.timeoutDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Input
            type="number"
            min={TIMEOUT_MIN_SECS}
            max={TIMEOUT_MAX_SECS}
            step={1}
            value={timeoutDraft}
            onChange={(event) => setTimeoutDraft(event.target.value)}
            onBlur={() => void commitTimeout()}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
            aria-label={t("settings.advanced.agentBridge.timeoutLabel")}
            disabled={isUpdating("agent_bridge_timeout_secs")}
            variant="compact"
            className="w-28"
          />
        </SettingContainer>
        <SettingContainer
          title={t("settings.advanced.agentBridge.promptFlagLabel")}
          description={t("settings.advanced.agentBridge.promptFlagDescription")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Input
            type="text"
            value={flagDraft}
            onChange={(event) => setFlagDraft(event.target.value)}
            onBlur={() => void commitFlag()}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
            aria-label={t("settings.advanced.agentBridge.promptFlagLabel")}
            disabled={isUpdating("agent_bridge_prompt_flag")}
            variant="compact"
            className="w-28"
          />
        </SettingContainer>
      </>
    );
  });
