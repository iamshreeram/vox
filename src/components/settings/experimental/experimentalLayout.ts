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
  | "agentBridge"
  | "agentContext"
  | "agentBridgeBinaryPath"
  | "tts"
  | "realtime";

export type ControlFlag = "memoryEnabled" | "agentBridgeEnabled";

export interface ControlEntry {
  id: ControlId;
  when?: ControlFlag;
}

export interface ExperimentalGroup {
  id: string;
  titleKey: string;
  descriptionKey?: string;
  defaultOpen: boolean;
  controls: readonly ControlEntry[];
}

export const EXPERIMENTAL_GROUPS: readonly ExperimentalGroup[] = [
  {
    id: "engine",
    titleKey: "settings.experimental.groups.engine",
    defaultOpen: true,
    controls: [
      { id: "postProcessing" },
      { id: "keyboardImplementation" },
      { id: "acceleration" },
      { id: "lazyStreamClose" },
      { id: "vadBackend" },
    ],
  },
  {
    id: "voiceCommands",
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
    titleKey: "settings.advanced.listeningMode.title",
    descriptionKey: "settings.advanced.listeningMode.description",
    defaultOpen: false,
    controls: [{ id: "wakeWord" }, { id: "ambientMode" }],
  },
  {
    id: "memory",
    titleKey: "settings.experimental.groups.memory",
    defaultOpen: false,
    controls: [
      { id: "memoryToggle" },
      { id: "memoryFacts", when: "memoryEnabled" },
    ],
  },
  {
    id: "agentBridge",
    titleKey: "settings.experimental.groups.agentBridge",
    defaultOpen: false,
    controls: [
      { id: "agentBridge" },
      { id: "agentContext" },
      { id: "agentBridgeBinaryPath", when: "agentBridgeEnabled" },
    ],
  },
  {
    id: "spokenReplies",
    titleKey: "settings.advanced.tts.groupTitle",
    defaultOpen: false,
    controls: [{ id: "tts" }],
  },
  {
    id: "realtime",
    titleKey: "settings.experimental.realtime.title",
    defaultOpen: false,
    controls: [{ id: "realtime" }],
  },
];

/** Control ids of a group that should render for the given feature flags, in order. */
export function visibleControls(
  group: ExperimentalGroup,
  flags: { memoryEnabled: boolean; agentBridgeEnabled: boolean },
): ControlId[] {
  return group.controls
    .filter((c) => c.when === undefined || flags[c.when])
    .map((c) => c.id);
}
