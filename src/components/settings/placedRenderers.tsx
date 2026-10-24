import React from "react";
import { PostProcessingToggle } from "./PostProcessingToggle";
import { KeyboardImplementationSelector } from "./debug/KeyboardImplementationSelector";
import { AccelerationSelector } from "./AccelerationSelector";
import { LazyStreamClose } from "./LazyStreamClose";
import { VadBackendSelector } from "./VadBackendSelector";
import { MemoryToggle } from "./MemoryToggle";
import { MemoryFactsList } from "./MemoryFactsList";
import { VoiceCommandsToggle } from "./VoiceCommandsToggle";
import { VoiceShortcutsEditor } from "./VoiceShortcutsEditor";
import { VoiceMediaControlsToggle } from "./VoiceMediaControlsToggle";
import { AgentBridgeToggle } from "./AgentBridgeToggle";
import { AgentContextToggle } from "./AgentContextToggle";
import { AgentBridgeBinaryPathField } from "./AgentBridgeBinaryPathField";
import { AgentBridgeAdvancedSettings } from "./AgentBridgeAdvancedSettings";
import { AcpAgentSettings } from "./AcpAgentSettings";
import {
  ConversationRecordsToggle,
  ConversationVaultContextToggle,
  ConversationVaultPathField,
  ConversationVaultToggle,
} from "./ConversationMemorySettings";
import { VaultRagSettings } from "./VaultRagSettings";
import { AgentToolsSettings } from "./AgentToolsSettings";
import { JobsSettings } from "./JobsSettings";
import { TtsSettings } from "./TtsSettings";
import { RealtimeSection } from "./RealtimeSection";
import { WakeWordToggle } from "./WakeWordToggle";
import { AmbientModeToggle } from "./AmbientModeToggle";
import type { ControlId } from "./settingsPlacement";

// Renders each placed control. Moved verbatim from the former Experimental page,
// plus the three groups that used to be hard-coded there.
export const PLACED_RENDERERS: Record<ControlId, () => React.ReactNode> = {
  postProcessing: () => (
    <PostProcessingToggle descriptionMode="tooltip" grouped={true} />
  ),
  keyboardImplementation: () => (
    <KeyboardImplementationSelector descriptionMode="tooltip" grouped={true} />
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
  memoryToggle: () => <MemoryToggle descriptionMode="tooltip" grouped={true} />,
  memoryFacts: () => <MemoryFactsList />,
  conversationMemory: () => (
    <>
      <ConversationRecordsToggle descriptionMode="tooltip" grouped={true} />
      <ConversationVaultToggle descriptionMode="tooltip" grouped={true} />
      <ConversationVaultPathField descriptionMode="tooltip" grouped={true} />
      <ConversationVaultContextToggle
        descriptionMode="tooltip"
        grouped={true}
      />
      <VaultRagSettings descriptionMode="tooltip" grouped={true} />
    </>
  ),
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
  acpAgent: () => <AcpAgentSettings descriptionMode="tooltip" grouped={true} />,
  agentTools: () => (
    <AgentToolsSettings descriptionMode="tooltip" grouped={true} />
  ),
  codingJobs: () => <JobsSettings descriptionMode="tooltip" grouped={true} />,
  tts: () => <TtsSettings descriptionMode="tooltip" grouped={true} />,
  realtime: () => <RealtimeSection descriptionMode="tooltip" grouped={true} />,
};
