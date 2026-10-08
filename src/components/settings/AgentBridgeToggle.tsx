import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface AgentBridgeToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AgentBridgeToggle: React.FC<AgentBridgeToggleProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const agentBridgeEnabled = getSetting("agent_bridge_enabled") ?? false;

    return (
      <ToggleSwitch
        checked={agentBridgeEnabled}
        onChange={(enabled) => updateSetting("agent_bridge_enabled", enabled)}
        isUpdating={isUpdating("agent_bridge_enabled")}
        label={t("settings.advanced.agentBridge.label")}
        description={t("settings.advanced.agentBridge.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  });
