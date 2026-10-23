// Single source of truth for where each formerly-Experimental control lives.
// Pure data and helpers only: no React, no hooks.

export type PlacedSection = "general" | "models" | "advanced" | "agents";

export type ControlId =
  | "postProcessing"
  | "keyboardImplementation"
  | "acceleration"
  | "lazyStreamClose"
  | "vadBackend"
  | "voiceCommands"
  | "voiceShortcuts"
  | "voiceMediaControls"
  | "wakeWord"
  | "ambientMode"
  | "memoryToggle"
  | "memoryFacts"
  | "conversationMemory"
  | "agentBridge"
  | "agentContext"
  | "agentBridgeBinaryPath"
  | "acpAgent"
  | "agentTools"
  | "codingJobs"
  | "tts"
  | "realtime";

export type ControlFlag = "memoryEnabled" | "agentBridgeEnabled";

export interface ControlEntry {
  id: ControlId;
  when?: ControlFlag;
}

export interface PlacedGroup {
  id: string;
  section: PlacedSection;
  titleKey: string;
  descriptionKey?: string;
  defaultOpen: boolean;
  controls: readonly ControlEntry[];
}

// Group ids are persisted verbatim in localStorage open-state keys, so every
// legacy id is kept unchanged. Only "vadEngine" is new.
export const PLACED_GROUPS: readonly PlacedGroup[] = [
  {
    id: "voiceCommands",
    section: "general",
    titleKey: "settings.experimental.groups.voiceCommands",
    defaultOpen: false,
    controls: [
      { id: "voiceCommands" },
      { id: "voiceShortcuts" },
      { id: "voiceMediaControls" },
    ],
  },
  {
    id: "listening",
    section: "general",
    titleKey: "settings.advanced.listeningMode.title",
    descriptionKey: "settings.advanced.listeningMode.description",
    defaultOpen: false,
    controls: [{ id: "wakeWord" }, { id: "ambientMode" }],
  },
  {
    id: "vadEngine",
    section: "models",
    titleKey: "settings.models.groups.vadEngine",
    defaultOpen: false,
    controls: [{ id: "vadBackend" }],
  },
  {
    id: "spokenReplies",
    section: "models",
    titleKey: "settings.advanced.tts.groupTitle",
    defaultOpen: false,
    controls: [{ id: "tts" }],
  },
  {
    id: "realtime",
    section: "models",
    titleKey: "settings.experimental.realtime.title",
    defaultOpen: false,
    controls: [{ id: "realtime" }],
  },
  {
    id: "engine",
    section: "advanced",
    titleKey: "settings.experimental.groups.engine",
    defaultOpen: true,
    controls: [
      { id: "postProcessing" },
      { id: "keyboardImplementation" },
      { id: "acceleration" },
      { id: "lazyStreamClose" },
    ],
  },
  {
    id: "acpAgent",
    section: "agents",
    titleKey: "settings.experimental.groups.acpAgent",
    descriptionKey: "settings.experimental.acpAgent.groupDescription",
    defaultOpen: false,
    controls: [{ id: "acpAgent" }],
  },
  {
    id: "agentBridge",
    section: "agents",
    titleKey: "settings.experimental.groups.agentBridge",
    defaultOpen: false,
    controls: [
      { id: "agentBridge" },
      { id: "agentContext" },
      { id: "agentBridgeBinaryPath", when: "agentBridgeEnabled" },
    ],
  },
  {
    id: "agentTools",
    section: "agents",
    titleKey: "settings.experimental.groups.agentTools",
    descriptionKey: "settings.experimental.agentTools.groupDescription",
    defaultOpen: false,
    controls: [{ id: "agentTools" }],
  },
  {
    id: "codingJobs",
    section: "agents",
    titleKey: "settings.experimental.groups.codingJobs",
    descriptionKey: "settings.experimental.jobs.groupDescription",
    defaultOpen: false,
    controls: [{ id: "codingJobs" }],
  },
  {
    id: "memory",
    section: "agents",
    titleKey: "settings.experimental.groups.memory",
    defaultOpen: false,
    controls: [
      { id: "memoryToggle" },
      { id: "memoryFacts", when: "memoryEnabled" },
    ],
  },
  {
    id: "conversationMemory",
    section: "agents",
    titleKey: "settings.experimental.groups.conversationMemory",
    descriptionKey: "settings.experimental.conversationMemory.groupDescription",
    defaultOpen: false,
    controls: [{ id: "conversationMemory" }],
  },
];

/** Groups rendered on one page, in declaration order. */
export function groupsForSection(section: PlacedSection): PlacedGroup[] {
  return PLACED_GROUPS.filter((group) => group.section === section);
}

/** The section and group that hold a control. */
export function placementOf(id: ControlId): {
  section: PlacedSection;
  groupId: string;
} {
  for (const group of PLACED_GROUPS) {
    if (group.controls.some((entry) => entry.id === id)) {
      return { section: group.section, groupId: group.id };
    }
  }
  throw new Error(`control ${id} is not placed in any group`);
}

/** Control ids of a group that should render for the given feature flags, in order. */
export function visibleControls(
  group: PlacedGroup,
  flags: { memoryEnabled: boolean; agentBridgeEnabled: boolean },
): ControlId[] {
  return group.controls
    .filter((entry) => entry.when === undefined || flags[entry.when])
    .map((entry) => entry.id);
}
