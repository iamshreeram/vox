import React from "react";
import { useTranslation } from "react-i18next";
import { CollapsibleSettingsGroup } from "../../ui/CollapsibleSettingsGroup";
import { PostProcessingToggle } from "../PostProcessingToggle";
import { KeyboardImplementationSelector } from "../debug/KeyboardImplementationSelector";
import { AccelerationSelector } from "../AccelerationSelector";
import { LazyStreamClose } from "../LazyStreamClose";
import { VadBackendSelector } from "../VadBackendSelector";
import { MemoryToggle } from "../MemoryToggle";
import { MemoryFactsList } from "../MemoryFactsList";
import { VoiceCommandsToggle } from "../VoiceCommandsToggle";
import { VoiceShortcutsEditor } from "../VoiceShortcutsEditor";
import { VoiceMediaControlsToggle } from "../VoiceMediaControlsToggle";
import { AgentBridgeToggle } from "../AgentBridgeToggle";
import { AgentContextToggle } from "../AgentContextToggle";
import { AgentBridgeBinaryPathField } from "../AgentBridgeBinaryPathField";
import { AgentBridgeAdvancedSettings } from "../AgentBridgeAdvancedSettings";
import { AcpAgentSettings } from "../AcpAgentSettings";
import {
  ConversationRecordsToggle,
  ConversationVaultContextToggle,
  ConversationVaultPathField,
  ConversationVaultToggle,
} from "../ConversationMemorySettings";
import { VaultRagSettings } from "../VaultRagSettings";
import { AgentToolsSettings } from "../AgentToolsSettings";
import { JobsSettings } from "../JobsSettings";
import { TtsSettings } from "../TtsSettings";
import { RealtimeSection } from "../RealtimeSection";
import { WakeWordToggle } from "../WakeWordToggle";
import { AmbientModeToggle } from "../AmbientModeToggle";
import { useSettings } from "../../../hooks/useSettings";
import {
  ControlId,
  EXPERIMENTAL_GROUPS,
  visibleControls,
} from "./experimentalLayout";

export const ExperimentalSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const memoryEnabled = getSetting("memory_enabled") || false;
  const agentBridgeEnabled = getSetting("agent_bridge_enabled") || false;
  const flags = { memoryEnabled, agentBridgeEnabled };

  const RENDER: Record<ControlId, () => React.ReactNode> = {
    postProcessing: () => (
      <PostProcessingToggle descriptionMode="tooltip" grouped={true} />
    ),
    keyboardImplementation: () => (
      <KeyboardImplementationSelector
        descriptionMode="tooltip"
        grouped={true}
      />
    ),
    acceleration: () => (
      <AccelerationSelector descriptionMode="tooltip" grouped={true} />
    ),
    lazyStreamClose: () => (
      <LazyStreamClose descriptionMode="tooltip" grouped={true} />
    ),
    vadBackend: () => (
      <VadBackendSelector descriptionMode="tooltip" grouped={true} />
    ),
    voiceCommands: () => (
      <VoiceCommandsToggle descriptionMode="tooltip" grouped={true} />
    ),
    voiceShortcuts: () => (
      <VoiceShortcutsEditor descriptionMode="tooltip" grouped={true} />
    ),
    voiceMediaControls: () => (
      <VoiceMediaControlsToggle descriptionMode="tooltip" grouped={true} />
    ),
    wakeWord: () => <WakeWordToggle descriptionMode="tooltip" grouped={true} />,
    ambientMode: () => (
      <AmbientModeToggle descriptionMode="tooltip" grouped={true} />
    ),
    memoryToggle: () => (
      <MemoryToggle descriptionMode="tooltip" grouped={true} />
    ),
    memoryFacts: () => <MemoryFactsList />,
    agentBridge: () => (
      <AgentBridgeToggle descriptionMode="tooltip" grouped={true} />
    ),
    agentContext: () => (
      <AgentContextToggle descriptionMode="tooltip" grouped={true} />
    ),
    agentBridgeBinaryPath: () => (
      <>
        <AgentBridgeBinaryPathField descriptionMode="tooltip" grouped={true} />
        <AgentBridgeAdvancedSettings descriptionMode="tooltip" grouped={true} />
      </>
    ),
    acpAgent: () => (
      <AcpAgentSettings descriptionMode="tooltip" grouped={true} />
    ),
    tts: () => <TtsSettings descriptionMode="tooltip" grouped={true} />,
    realtime: () => (
      <RealtimeSection descriptionMode="tooltip" grouped={true} />
    ),
  };

  return (
    <div className="max-w-3xl w-full mx-auto space-y-3">
      <h2 className="px-1 text-xs font-medium text-mid-gray uppercase tracking-wide">
        {t("settings.advanced.groups.experimental")}
      </h2>
      {EXPERIMENTAL_GROUPS.map((group) => (
        <CollapsibleSettingsGroup
          key={group.id}
          id={group.id}
          title={t(group.titleKey)}
          description={
            group.descriptionKey ? t(group.descriptionKey) : undefined
          }
          defaultOpen={group.defaultOpen}
        >
          {visibleControls(group, flags).map((id) => (
            <React.Fragment key={id}>{RENDER[id]()}</React.Fragment>
          ))}
        </CollapsibleSettingsGroup>
      ))}
      <CollapsibleSettingsGroup
        id="conversationMemory"
        title={t("settings.experimental.groups.conversationMemory")}
        description={t(
          "settings.experimental.conversationMemory.groupDescription",
        )}
        defaultOpen={false}
      >
        <ConversationRecordsToggle descriptionMode="tooltip" grouped={true} />
        <ConversationVaultToggle descriptionMode="tooltip" grouped={true} />
        <ConversationVaultPathField descriptionMode="tooltip" grouped={true} />
        <ConversationVaultContextToggle
          descriptionMode="tooltip"
          grouped={true}
        />
        <VaultRagSettings descriptionMode="tooltip" grouped={true} />
      </CollapsibleSettingsGroup>
      <CollapsibleSettingsGroup
        id="agentTools"
        title={t("settings.experimental.groups.agentTools")}
        description={t("settings.experimental.agentTools.groupDescription")}
        defaultOpen={false}
      >
        <AgentToolsSettings descriptionMode="tooltip" grouped={true} />
      </CollapsibleSettingsGroup>
      <CollapsibleSettingsGroup
        id="codingJobs"
        title={t("settings.experimental.groups.codingJobs")}
        description={t("settings.experimental.jobs.groupDescription")}
        defaultOpen={false}
      >
        <JobsSettings descriptionMode="tooltip" grouped={true} />
      </CollapsibleSettingsGroup>
    </div>
  );
};
