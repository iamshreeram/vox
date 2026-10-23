import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";
import { useSettingsStore } from "../../stores/settingsStore";

interface AgentBridgeAdvancedSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AgentBridgeAdvancedSettings: React.FC<AgentBridgeAdvancedSettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const agentBridgeEnabled = getSetting("agent_bridge_enabled") ?? false;
    const promptFlag = getSetting("agent_bridge_prompt_flag") ?? "-p";

    const [flagDraft, setFlagDraft] = useState(promptFlag);

    useEffect(() => {
      setFlagDraft(promptFlag);
    }, [promptFlag]);

    if (!agentBridgeEnabled) return null;

    // updateSetting reverts the store on a backend rejection. Re-read the stored
    // value afterwards so the field shows what was actually kept.
    const storedNow = (key: "agent_bridge_prompt_flag") =>
      useSettingsStore.getState().settings?.[key];

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
