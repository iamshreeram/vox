import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettings } from "../../hooks/useSettings";

interface AgentBridgeBinaryPathFieldProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AgentBridgeBinaryPathField: React.FC<AgentBridgeBinaryPathFieldProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const agentBridgeEnabled = getSetting("agent_bridge_enabled") ?? false;
    const binaryPath = getSetting("agent_bridge_binary_path") ?? null;
    const [localValue, setLocalValue] = useState(binaryPath ?? "");

    React.useEffect(() => {
      setLocalValue(binaryPath ?? "");
    }, [binaryPath]);

    if (!agentBridgeEnabled) return null;

    return (
      <SettingContainer
        title={t("settings.advanced.agentBridge.binaryPathLabel")}
        description={t("settings.advanced.agentBridge.binaryPathDescription")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Input
          type="text"
          value={localValue}
          onChange={(event) => setLocalValue(event.target.value)}
          onBlur={() =>
            updateSetting("agent_bridge_binary_path", localValue === "" ? null : localValue)
          }
          placeholder=""
          variant="compact"
          disabled={isUpdating("agent_bridge_binary_path")}
          className="flex-1 min-w-[360px]"
        />
      </SettingContainer>
    );
  });
