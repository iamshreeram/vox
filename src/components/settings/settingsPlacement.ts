// Single source of truth for where each formerly-Experimental control lives.
// Pure data and helpers only: no React, no hooks.

export type PlacedSection =
  | "general"
  | "appearance"
  | "models"
  | "advanced"
  | "agents";

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
  | "realtime"
  | "replyStyle"
  | "conversation"
  | "floatingOrb"
  | "chatOpacity"
  | "persona"
  | "latencyPanel"
  | "smartTurn"
  | "speakerFilter";

export type ControlFlag = "memoryEnabled" | "agentBridgeEnabled";

export interface ControlEntry {
  id: ControlId;
  when?: ControlFlag;
  /** Sub-heading shown above the control when its group holds several controls. */
  subTitleKey?: string;
  /** True when this control alone renders only while the Experimental switch is on (default false). */
  experimental?: boolean;
}

export interface PlacedGroup {
  id: string;
  section: PlacedSection;
  titleKey: string;
  descriptionKey?: string;
  defaultOpen: boolean;
  /** True when the group only renders while the Experimental switch is on. */
  experimental: boolean;
  controls: readonly ControlEntry[];
}

// Group ids are persisted verbatim in localStorage open-state keys, so the ids
// that survive keep their old names. "smartTurn", "speakerFilter", "replyStyle"
// and "realtime" are no longer groups; their controls moved into the groups below.
export const PLACED_GROUPS: readonly PlacedGroup[] = [
  {
    id: "voiceCommands",
    section: "general",
    titleKey: "settings.experimental.groups.voiceCommands",
    defaultOpen: false,
    experimental: true,
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
    experimental: true,
    controls: [{ id: "ambientMode" }],
  },
  {
    id: "conversation",
    section: "general",
    titleKey: "settings.general.groups.conversation",
    defaultOpen: false,
    experimental: false,
    controls: [
      { id: "conversation" },
      { id: "wakeWord" },
      { id: "persona" },
      { id: "replyStyle", subTitleKey: "settings.models.groups.replyStyle" },
      { id: "floatingOrb", subTitleKey: "settings.general.groups.floatingOrb" },
      {
        id: "realtime",
        subTitleKey: "settings.experimental.realtime.title",
        experimental: true,
      },
      { id: "latencyPanel", experimental: true },
    ],
  },
  {
    // Moved from Conversation: the Appearance page holds the conversation window's transparency.
    id: "chatWindow",
    section: "appearance",
    titleKey: "settings.appearance.chatWindow.title",
    defaultOpen: true,
    experimental: false,
    controls: [{ id: "chatOpacity" }],
  },
  {
    id: "spokenReplies",
    section: "models",
    titleKey: "settings.models.groups.textToSpeech",
    defaultOpen: true,
    experimental: false,
    controls: [{ id: "tts" }],
  },
  {
    id: "vadEngine",
    section: "models",
    titleKey: "settings.models.groups.voiceDetection",
    defaultOpen: false,
    experimental: false,
    controls: [
      { id: "vadBackend" },
      { id: "smartTurn" },
      { id: "speakerFilter" },
    ],
  },
  {
    id: "engine",
    section: "advanced",
    titleKey: "settings.experimental.groups.engine",
    defaultOpen: true,
    experimental: true,
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
    defaultOpen: false,
    experimental: true,
    controls: [{ id: "acpAgent" }],
  },
  {
    id: "agentBridge",
    section: "agents",
    titleKey: "settings.experimental.groups.agentBridge",
    defaultOpen: false,
    experimental: true,
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
    experimental: true,
    controls: [{ id: "agentTools" }],
  },
  {
    id: "codingJobs",
    section: "agents",
    titleKey: "settings.experimental.groups.codingJobs",
    descriptionKey: "settings.experimental.jobs.groupDescription",
    defaultOpen: false,
    experimental: true,
    controls: [{ id: "codingJobs" }],
  },
  {
    id: "memory",
    section: "agents",
    titleKey: "settings.experimental.groups.memory",
    defaultOpen: false,
    experimental: true,
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
    experimental: true,
    controls: [{ id: "conversationMemory" }],
  },
];

/** Groups rendered on one page, in declaration order. */
export function groupsForSection(section: PlacedSection): PlacedGroup[] {
  return PLACED_GROUPS.filter((group) => group.section === section);
}

/** Groups of one page that render now: experimental groups only while the switch is on. */
export function visibleGroups(
  section: PlacedSection,
  experimentalEnabled: boolean,
): PlacedGroup[] {
  return groupsForSection(section).filter(
    (group) => !group.experimental || experimentalEnabled,
  );
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

/**
 * Control ids of a group that should render for the given feature flags, in order.
 * Controls marked `experimental` render only while `experimentalEnabled` is true.
 */
export function visibleControls(
  group: PlacedGroup,
  flags: { memoryEnabled: boolean; agentBridgeEnabled: boolean },
  experimentalEnabled = true,
): ControlId[] {
  return group.controls
    .filter((entry) => entry.when === undefined || flags[entry.when])
    .filter((entry) => experimentalEnabled || entry.experimental !== true)
    .map((entry) => entry.id);
}

/** The placed control entry for an id, or undefined when it is not placed. */
export function controlEntryOf(id: ControlId): ControlEntry | undefined {
  for (const group of PLACED_GROUPS) {
    const entry = group.controls.find((candidate) => candidate.id === id);
    if (entry !== undefined) return entry;
  }
  return undefined;
}
