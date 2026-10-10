import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { Button } from "../ui/Button";
import { useSettings } from "../../hooks/useSettings";

interface AgentContextToggleProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AgentContextToggle: React.FC<AgentContextToggleProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const [isClearing, setIsClearing] = useState(false);

    const agentContextEnabled = getSetting("agent_context_enabled") ?? false;
    const agentBridgeEnabled = getSetting("agent_bridge_enabled") ?? false;

    const handleClear = async () => {
      setIsClearing(true);
      try {
        const result = await commands.agentClearConversation();
        if (result.status === "ok") {
          toast.success(t("settings.advanced.agentContext.clearedToast"));
        } else {
          toast.error(t("settings.advanced.agentContext.clearFailedToast"));
        }
      } catch (error) {
        toast.error(String(error));
      } finally {
        setIsClearing(false);
      }
    };

    return (
      <>
        <div
          title={
            agentBridgeEnabled
              ? undefined
              : t("settings.advanced.agentBridge.label")
          }
        >
          <ToggleSwitch
            checked={agentContextEnabled}
            onChange={(enabled) =>
              updateSetting("agent_context_enabled", enabled)
            }
            isUpdating={isUpdating("agent_context_enabled")}
            disabled={!agentBridgeEnabled}
            label={t("settings.advanced.agentContext.title")}
            description={t("settings.advanced.agentContext.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            tooltipPosition="bottom"
          />
        </div>
        {agentContextEnabled && (
          <div className="px-4 pb-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={handleClear}
              disabled={isClearing}
            >
              {t("settings.advanced.agentContext.clearButton")}
            </Button>
          </div>
        )}
      </>
    );
  },
);
