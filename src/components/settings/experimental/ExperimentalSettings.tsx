import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { PostProcessingToggle } from "../PostProcessingToggle";
import { KeyboardImplementationSelector } from "../debug/KeyboardImplementationSelector";
import { AccelerationSelector } from "../AccelerationSelector";
import { LazyStreamClose } from "../LazyStreamClose";
import { VadBackendSelector } from "../VadBackendSelector";
import { MemoryToggle } from "../MemoryToggle";
import { MemoryFactsList } from "../MemoryFactsList";
import { VoiceCommandsToggle } from "../VoiceCommandsToggle";
import { AgentBridgeToggle } from "../AgentBridgeToggle";
import { AgentContextToggle } from "../AgentContextToggle";
import { AgentBridgeBinaryPathField } from "../AgentBridgeBinaryPathField";
import { ListeningModeSection } from "../ListeningModeSection";
import { useSettings } from "../../../hooks/useSettings";

export const ExperimentalSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const memoryEnabled = getSetting("memory_enabled") || false;
  const agentBridgeEnabled = getSetting("agent_bridge_enabled") || false;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.advanced.groups.experimental")}>
        <PostProcessingToggle descriptionMode="tooltip" grouped={true} />
        <KeyboardImplementationSelector
          descriptionMode="tooltip"
          grouped={true}
        />
        <AccelerationSelector descriptionMode="tooltip" grouped={true} />
        <LazyStreamClose descriptionMode="tooltip" grouped={true} />
        <VadBackendSelector descriptionMode="tooltip" grouped={true} />
        <MemoryToggle descriptionMode="tooltip" grouped={true} />
        {memoryEnabled && <MemoryFactsList />}
        <VoiceCommandsToggle descriptionMode="tooltip" grouped={true} />
        <AgentBridgeToggle descriptionMode="tooltip" grouped={true} />
        <AgentContextToggle descriptionMode="tooltip" grouped={true} />
        {agentBridgeEnabled && (
          <AgentBridgeBinaryPathField
            descriptionMode="tooltip"
            grouped={true}
          />
        )}
        <ListeningModeSection />
      </SettingsGroup>
    </div>
  );
};
