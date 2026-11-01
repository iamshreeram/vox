import {
  PLACED_GROUPS,
  controlEntryOf,
  type ControlId,
} from "../../settingsPlacement";
import type { DynamicFamily, SearchEntry } from "../types";

/** Entry id of the advanced-page toggle that unlocks the Experimental switch. */
const UNLOCK = "advanced.experimentalToggle";

const GROUP = {
  engine: "settings.experimental.groups.engine",
  vadEngine: "settings.models.groups.voiceDetection",
  voiceCommands: "settings.experimental.groups.voiceCommands",
  listening: "settings.advanced.listeningMode.title",
  memory: "settings.experimental.groups.memory",
  agentBridge: "settings.experimental.groups.agentBridge",
  acpAgent: "settings.experimental.groups.acpAgent",
  spokenReplies: "settings.models.groups.textToSpeech",
  conversation: "settings.general.groups.conversation",
  conversationMemory: "settings.experimental.groups.conversationMemory",
  agentTools: "settings.experimental.groups.agentTools",
  codingJobs: "settings.experimental.groups.codingJobs",
} as const;

/**
 * Rows whose control is gated on its own inside an always-visible group, keyed by row slug.
 * Realtime and the latency panel sit in the ungated Conversation group but stay behind the
 * Experimental switch (`experimental: true` on their ControlEntry).
 */
const REALTIME_ROW_SLUGS = [
  "realtimeEnabled",
  "realtimeDirectEndpoint",
  "realtimeEndpointUrl",
  "realtimeModel",
  "realtimeToken",
  "realtimeStreamReply",
  "realtimeEarlySpeech",
  "realtimeFastVoice",
  "realtimeAck",
  "realtimePrewarm",
  "realtimeAutoProxy",
  "realtimeProxy",
  "realtimeProxyUrl",
  "realtimeTimingLog",
] as const;

const CONTROL_BY_ROW_SLUG: Readonly<Record<string, ControlId>> = {
  ...Object.fromEntries(REALTIME_ROW_SLUGS.map((slug) => [slug, "realtime"])),
  conversationLatency: "latencyPanel",
};

/**
 * One row of a placed control. The page (section), the gating and the id prefix come from
 * the control entry when it has its own `experimental` flag, otherwise from the placed group:
 * gated rows keep "experimental.<slug>" and need the Experimental switch; the rest use
 * "<section>.<slug>".
 */
function row(
  slug: string,
  groupId: keyof typeof GROUP,
  labelKey: string,
  descriptionKey: string | undefined,
  keywords: readonly string[],
): SearchEntry {
  const group = PLACED_GROUPS.find((candidate) => candidate.id === groupId);
  if (group === undefined) {
    throw new Error(`search index: no placed group "${groupId}"`);
  }
  const controlId = CONTROL_BY_ROW_SLUG[slug];
  const ownFlag =
    controlId === undefined
      ? undefined
      : controlEntryOf(controlId)?.experimental;
  const experimental = ownFlag ?? group.experimental;
  const prefix = experimental ? "experimental" : group.section;
  return {
    id: `${prefix}.${slug}`,
    section: group.section,
    requiresExperimental: experimental,
    groupId,
    groupKey: GROUP[groupId],
    labelKey,
    ...(descriptionKey === undefined ? {} : { descriptionKey }),
    keywords,
    ...(experimental ? { unlockedBy: UNLOCK } : {}),
  };
}

/** Maps every key `prefix + id + suffix` to the same reason. */
function excludeFamily(
  prefix: string,
  ids: readonly string[],
  reason: string,
): Record<string, string> {
  return Object.fromEntries(ids.map((id) => [`${prefix}${id}`, reason]));
}

const REALTIME_PREFIX = "settings.experimental.realtime.";

/** Realtime rows: the label family is `${K}.<id>.label` in RealtimeSection.tsx. */
const REALTIME_LABEL_IDS = [
  "enabled",
  "directEndpoint",
  "endpoint",
  "model",
  "token",
  "streamReply",
  "earlySpeech",
  "fastVoice",
  "ack",
  "prewarm",
  "autoProxy",
  "proxy",
  "proxyUrl",
  "timingLog",
] as const;

const REALTIME_STATUS_IDS = [
  "off",
  "ok",
  "missing_endpoint",
  "missing_model",
  "missing_token",
  "invalid_endpoint",
  "needs_https",
] as const;

const REALTIME_ERROR_IDS = ["invalid", "needs_https", "too_long"] as const;

const COMPOSER_PREFIX = "settings.advanced.tts.composer.";
const COMPOSER_MODE_IDS = ["off", "model", "command"] as const;
const COMPOSER_FALLBACK_IDS = ["short_notice", "verbatim", "silent"] as const;
const COMPOSER_READINESS_IDS = [
  "needs_setup",
  "needs_acknowledgement",
  "ready",
] as const;
const COMPOSER_EMOTION_IDS = [
  "neutral",
  "cheerful",
  "excited",
  "calm",
  "concerned",
  "apologetic",
  "serious",
] as const;
const COMPOSER_STATUS_IDS = [
  "ok",
  "timeout",
  "not_configured",
  "needs_acknowledgement",
  "backend_error",
  "output_too_large",
  "empty",
  "unparseable",
  "superseded",
  "unknown",
] as const;

const CONVERSE_STATE_IDS = [
  "listening",
  "thinking",
  "speaking",
  "acting",
  "idle",
] as const;
const CONVERSE_END_IDS = [
  "user_ended",
  "idle",
  "cancelled",
  "not_configured",
  "audio_unavailable",
  "speech_stuck",
  "turn_limit",
] as const;

/** Slider label family in ConversationSection.tsx: `converse.section.<id>Label` (object array, label={item.label}). */
const CONVERSE_SLIDER_IDS = ["silence", "idleTurns", "contextTurns"] as const;

const JOB_STATE_IDS = [
  "idle",
  "running",
  "succeeded",
  "failed",
  "cancelled",
  "timedOut",
  "couldntStart",
] as const;

const SHORTCUT_ACTION_IDS = ["url", "folder", "app"] as const;

const PLACED_GROUPS_SOURCE = "src/components/settings/PlacedGroups.tsx";
const SPOKEN_REPLY_SOURCE = "src/components/settings/SpokenReplyStyle.tsx";
const VOICE_SHORTCUTS_SOURCE =
  "src/components/settings/VoiceShortcutsEditor.tsx";
const REALTIME_SOURCE = "src/components/settings/RealtimeSection.tsx";
const CONVERSATION_SOURCE = "src/components/settings/ConversationSection.tsx";
const JOBS_SOURCE = "src/components/settings/JobsSettings.tsx";
const TTS_SOURCE = "src/components/settings/TtsSettings.tsx";
export const EXPERIMENTAL_ENTRIES: readonly SearchEntry[] = [
  // ----- engine -----
  row(
    "postProcessing",
    "engine",
    "settings.debug.postProcessingToggle.label",
    "settings.debug.postProcessingToggle.description",
    [
      "post processing",
      "llm",
      "cleanup",
      "rewrite",
      "ai cleanup",
      "transcript cleanup",
      "refine text",
    ],
  ),
  row(
    "keyboardImplementation",
    "engine",
    "settings.debug.keyboardImplementation.title",
    "settings.debug.keyboardImplementation.description",
    [
      "keyboard",
      "keys",
      "typing",
      "input method",
      "key simulation",
      "keyboard backend",
      "hotkey",
    ],
  ),
  row(
    "accelerationTranscribe",
    "engine",
    "settings.advanced.acceleration.transcribe.title",
    "settings.advanced.acceleration.transcribe.description",
    [
      "gpu",
      "metal",
      "cuda",
      "whisper",
      "acceleration",
      "transcription device",
      "hardware",
      "cpu",
      "accelerator",
    ],
  ),
  row(
    "accelerationOrt",
    "engine",
    "settings.advanced.acceleration.ort.title",
    "settings.advanced.acceleration.ort.description",
    [
      "gpu",
      "onnx",
      "ort",
      "onnx runtime",
      "acceleration",
      "vad device",
      "coreml",
      "hardware",
      "accelerator",
      "cpu",
    ],
  ),
  row(
    "lazyStreamClose",
    "engine",
    "settings.advanced.lazyStreamClose.label",
    "settings.advanced.lazyStreamClose.description",
    [
      "lazy stream close",
      "keep mic open",
      "keep microphone open",
      "microphone latency",
      "bluetooth",
      "stream",
      "latency",
      "between transcriptions",
      "microphone",
      "mic",
    ],
  ),
  row(
    "vadBackend",
    "vadEngine",
    "settings.advanced.vadBackend.title",
    "settings.advanced.vadBackend.description",
    [
      "vad",
      "silero",
      "earshot",
      "silence",
      "voice activity",
      "voice activity detection",
      "speech detection",
      "silence detection",
    ],
  ),

  // ----- voice commands -----
  row(
    "voiceCommands",
    "voiceCommands",
    "settings.advanced.voiceCommands.label",
    "settings.advanced.voiceCommands.description",
    [
      "voice commands",
      "voice command",
      "commands",
      "voice control",
      "spoken commands",
      "say",
    ],
  ),
  row(
    "voiceShortcuts",
    "voiceCommands",
    "settings.advanced.voiceShortcuts.title",
    "settings.advanced.voiceShortcuts.description",
    [
      "voice shortcuts",
      "shortcut",
      "phrase",
      "open url",
      "open folder",
      "open app",
      "custom phrase",
      "voice macros",
    ],
  ),
  row(
    "voiceMediaControls",
    "voiceCommands",
    "settings.advanced.voiceMediaControls.label",
    "settings.advanced.voiceMediaControls.description",
    [
      "media",
      "media controls",
      "music",
      "play",
      "pause",
      "media keys",
      "playback",
    ],
  ),

  // ----- wake word (ungated; lives with the conversation controls) -----
  row(
    "wakeWord",
    "conversation",
    "settings.advanced.wakeWord.label",
    "settings.advanced.wakeWord.description",
    [
      "wake word",
      "hotword",
      "trigger word",
      "hands free",
      "activation phrase",
      "always listening",
      "voice trigger",
      "hey jarvis",
    ],
  ),
  // ----- listening -----
  row(
    "wakeWordConfidence",
    "conversation",
    "settings.advanced.wakeWord.confidenceLabel",
    "settings.advanced.wakeWord.confidenceDescription",
    [
      "wake word threshold",
      "wake word confidence",
      "detection threshold",
      "sensitivity",
      "false triggers",
      "confidence",
    ],
  ),
  row(
    "wakeWordCooldown",
    "conversation",
    "settings.advanced.wakeWord.cooldownLabel",
    "settings.advanced.wakeWord.cooldownDescription",
    ["wake word cooldown", "cooldown", "wait", "pause", "milliseconds"],
  ),
  row(
    "ambientMode",
    "listening",
    "settings.advanced.ambientMode.label",
    "settings.advanced.ambientMode.description",
    [
      "ambient",
      "ambient mode",
      "always listening",
      "continuous listening",
      "open mic",
      "background listening",
    ],
  ),
  row(
    "ambientWakeName",
    "listening",
    "settings.advanced.ambientMode.wakeNameLabel",
    "settings.advanced.ambientMode.wakeNameDescription",
    ["ambient wake name", "wake name", "address name", "vox name", "name"],
  ),
  row(
    "ambientWindow",
    "listening",
    "settings.advanced.ambientMode.windowLabel",
    "settings.advanced.ambientMode.windowDescription",
    [
      "ambient memory",
      "memory window",
      "context window",
      "recent conversation",
      "history length",
      "seconds",
    ],
  ),

  // ----- memory -----
  row(
    "memory",
    "memory",
    "settings.advanced.memory.label",
    "settings.advanced.memory.description",
    [
      "memory",
      "remember",
      "facts",
      "long term memory",
      "personal facts",
      "recall",
      "remember facts",
    ],
  ),
  // The facts list has no SettingContainer title; the add-field placeholder is the
  // anchor, so reveal falls back to the Memory card header.
  row(
    "memoryFacts",
    "memory",
    "settings.advanced.memory.addPlaceholder",
    undefined,
    [
      "facts",
      "saved facts",
      "forget",
      "remove memory",
      "list memories",
      "remembered facts",
      "delete memory",
    ],
  ),

  // ----- agent bridge -----
  row(
    "agentBridge",
    "agentBridge",
    "settings.advanced.agentBridge.label",
    "settings.advanced.agentBridge.description",
    [
      "agent",
      "agent bridge",
      "coding agent",
      "bridge",
      "assistant",
      "integration",
    ],
  ),
  row(
    "agentContext",
    "agentBridge",
    "settings.advanced.agentContext.title",
    "settings.advanced.agentContext.description",
    [
      "agent context",
      "context",
      "clear context",
      "conversation context",
      "forget context",
      "reset context",
    ],
  ),
  row(
    "agentBridgePromptFlag",
    "agentBridge",
    "settings.advanced.agentBridge.promptFlagLabel",
    "settings.advanced.agentBridge.promptFlagDescription",
    ["prompt flag", "flag", "argument", "-p", "agent flag"],
  ),
  row(
    "agentBridgeBinaryPath",
    "agentBridge",
    "settings.advanced.agentBridge.binaryPathLabel",
    "settings.advanced.agentBridge.binaryPathDescription",
    [
      "binary",
      "binary path",
      "executable",
      "path",
      "agent binary",
      "cli path",
      "location",
    ],
  ),

  // ----- ACP agent (AcpAgentSettings) -----
  row(
    "acpAgentEnabled",
    "acpAgent",
    "settings.advanced.acpAgent.enabled.label",
    "settings.advanced.acpAgent.enabled.description",
    ["agent", "acp", "protocol", "assistant", "escalation"],
  ),
  row(
    "acpAgentCommand",
    "acpAgent",
    "settings.advanced.acpAgent.command.label",
    "settings.advanced.acpAgent.command.description",
    ["agent path", "command", "executable", "program", "acp", "binary"],
  ),
  row(
    "acpAgentArgs",
    "acpAgent",
    "settings.advanced.acpAgent.args.label",
    "settings.advanced.acpAgent.args.description",
    ["arguments", "args", "flags", "agent arguments", "acp"],
  ),
  row(
    "acpAgentTimeout",
    "acpAgent",
    "settings.advanced.acpAgent.timeout.label",
    "settings.advanced.acpAgent.timeout.description",
    ["timeout", "seconds", "wait", "agent timeout", "turn timeout"],
  ),
  row(
    "acpPermissionMode",
    "acpAgent",
    "settings.advanced.acpPermission.mode.label",
    "settings.advanced.acpPermission.mode.description",
    [
      "agent permission",
      "approve",
      "allow",
      "deny",
      "tool approval",
      "ask before",
      "permissions",
    ],
  ),
  row(
    "acpPermissionTimeout",
    "acpAgent",
    "settings.advanced.acpPermission.timeout.label",
    "settings.advanced.acpPermission.timeout.description",
    ["approval timeout", "agent permission", "approve", "timeout"],
  ),
  row(
    "acpConversationHandoff",
    "acpAgent",
    "settings.advanced.acpAgent.conversationHandoff.label",
    "settings.advanced.acpAgent.conversationHandoff.description",
    [
      "conversation",
      "hey jarvis",
      "agent",
      "handoff",
      "hand off",
      "web",
      "files",
      "programs",
      "open-ended",
    ],
  ),
  row(
    "acpDictationToAgent",
    "acpAgent",
    "settings.advanced.acpAgent.dictation.label",
    "settings.advanced.acpAgent.dictation.description",
    ["dictation", "agent", "paste", "voice command", "send to agent", "acp"],
  ),
  row(
    "acpConversationPlanner",
    "acpAgent",
    "settings.advanced.acpAgent.conversationPlanner.label",
    "settings.advanced.acpAgent.conversationPlanner.description",
    ["plan", "planner", "multi-step", "steps", "sequence", "hand off", "agent"],
  ),
  row(
    "acpStepAgentResearch",
    "acpAgent",
    "settings.advanced.acpAgent.stepAgents.research.label",
    "settings.advanced.acpAgent.stepAgents.research.description",
    [
      "specialist agent",
      "research",
      "look up",
      "search",
      "agent path",
      "agent arguments",
    ],
  ),
  row(
    "acpStepAgentBrowser",
    "acpAgent",
    "settings.advanced.acpAgent.stepAgents.browser.label",
    "settings.advanced.acpAgent.stepAgents.browser.description",
    [
      "specialist agent",
      "browser",
      "web",
      "web pages",
      "agent path",
      "agent arguments",
    ],
  ),
  row(
    "acpStepAgentCode",
    "acpAgent",
    "settings.advanced.acpAgent.stepAgents.code.label",
    "settings.advanced.acpAgent.stepAgents.code.description",
    [
      "specialist agent",
      "code",
      "coding",
      "programming",
      "agent path",
      "agent arguments",
    ],
  ),
  row(
    "acpStepAgentFiles",
    "acpAgent",
    "settings.advanced.acpAgent.stepAgents.files.label",
    "settings.advanced.acpAgent.stepAgents.files.description",
    [
      "specialist agent",
      "files",
      "file",
      "read",
      "edit",
      "agent path",
      "agent arguments",
    ],
  ),

  // ----- spoken replies (TtsSettings) -----
  row(
    "tts",
    "spokenReplies",
    "settings.advanced.tts.label",
    "settings.advanced.tts.description",
    [
      "tts",
      "text to speech",
      "voice",
      "speech",
      "speak",
      "spoken replies",
      "read aloud",
      "audio output",
      "speaker",
    ],
  ),
  row(
    "ttsTamil",
    "spokenReplies",
    "settings.advanced.tts.tamil.title",
    "settings.advanced.tts.tamil.description",
    [
      "tamil voice",
      "tamil",
      "tamil text to speech",
      "tamil tts",
      "offline voice",
    ],
  ),
  row(
    "ttsVoice",
    "spokenReplies",
    "settings.advanced.tts.voiceLabel",
    "settings.advanced.tts.voiceDescription",
    [
      "voice",
      "tts voice",
      "voice name",
      "speaker voice",
      "text to speech voice",
      "voice selection",
    ],
  ),
  row(
    "ttsRate",
    "spokenReplies",
    "settings.advanced.tts.rateLabel",
    "settings.advanced.tts.rateDescription",
    [
      "rate",
      "speed",
      "speaking rate",
      "words per minute",
      "wpm",
      "tts speed",
      "pace",
    ],
  ),
  row(
    "ttsTest",
    "spokenReplies",
    "settings.advanced.tts.testLabel",
    "settings.advanced.tts.testDescription",
    [
      "test",
      "audition",
      "try voice",
      "preview voice",
      "sample",
      "test phrase",
      "stop speech",
      "stop",
    ],
  ),
  row(
    "ttsEngine",
    "spokenReplies",
    "settings.advanced.tts.natural.engine.label",
    "settings.advanced.tts.natural.engine.description",
    [
      "engine",
      "voice engine",
      "natural voice",
      "neural voice",
      "system voice",
      "realistic voice",
    ],
  ),
  row(
    "ttsNaturalVoice",
    "spokenReplies",
    "settings.advanced.tts.natural.voice.label",
    "settings.advanced.tts.natural.voice.description",
    [
      "natural voice",
      "neural voice",
      "accent",
      "american",
      "british",
      "preview",
    ],
  ),
  row(
    "ttsNaturalModel",
    "spokenReplies",
    "settings.advanced.tts.natural.model.label",
    undefined,
    [
      "natural voice model",
      "download voice",
      "voice model",
      "delete model",
      "retry",
      "offline speech",
    ],
  ),
  row(
    "ttsPhononModel",
    "spokenReplies",
    "settings.advanced.tts.engines.phonon.model.label",
    "settings.advanced.tts.engines.phonon.model.description",
    [
      "phonon",
      "phonon voice",
      "voice model",
      "install voice",
      "install from folder",
      "download voice",
      "delete voice",
      "offline voice",
      "english voice",
    ],
  ),

  // ----- spoken reply style (SpokenReplyStyle) -----
  row(
    "spokenReplyMode",
    "conversation",
    "settings.advanced.tts.composer.modeLabel",
    "settings.advanced.tts.composer.modeDescription",
    [
      "mode",
      "composer",
      "spoken reply mode",
      "off",
      "model",
      "command",
      "external command",
    ],
  ),
  row(
    "spokenReplyProvider",
    "conversation",
    "settings.advanced.tts.composer.providerLabel",
    "settings.advanced.tts.composer.providerDescription",
    ["provider", "model provider", "llm provider", "backend", "service"],
  ),
  row(
    "spokenReplyModel",
    "conversation",
    "settings.advanced.tts.composer.modelLabel",
    "settings.advanced.tts.composer.modelDescription",
    ["model", "model name", "llm model", "spoken reply model"],
  ),
  row(
    "spokenReplyCommandPath",
    "conversation",
    "settings.advanced.tts.composer.commandPathLabel",
    "settings.advanced.tts.composer.commandPathDescription",
    [
      "command",
      "command path",
      "executable",
      "script",
      "external command",
      "path",
    ],
  ),
  row(
    "spokenReplyCommandArgs",
    "conversation",
    "settings.advanced.tts.composer.commandArgsLabel",
    "settings.advanced.tts.composer.commandArgsDescription",
    ["arguments", "args", "command arguments", "flags", "command line"],
  ),
  row(
    "spokenReplyPromptFlag",
    "conversation",
    "settings.advanced.tts.composer.promptFlagLabel",
    "settings.advanced.tts.composer.promptFlagDescription",
    ["prompt flag", "flag", "prompt argument", "command flag"],
  ),
  row(
    "spokenReplyAcknowledge",
    "conversation",
    "settings.advanced.tts.composer.ackLabel",
    "settings.advanced.tts.composer.ackDescription",
    [
      "acknowledge",
      "acknowledgement",
      "acknowledgment",
      "consent",
      "confirm",
      "warning",
    ],
  ),
  row(
    "spokenReplyStyle",
    "conversation",
    "settings.advanced.tts.composer.styleLabel",
    "settings.advanced.tts.composer.styleDescription",
    [
      "style",
      "tone",
      "speaking style",
      "voice style",
      "instructions",
      "persona",
    ],
  ),
  row(
    "spokenReplyTimeout",
    "conversation",
    "settings.advanced.tts.composer.timeoutLabel",
    "settings.advanced.tts.composer.timeoutDescription",
    ["timeout", "seconds", "wait", "time limit", "deadline"],
  ),
  row(
    "spokenReplySendUtterance",
    "conversation",
    "settings.advanced.tts.composer.sendUtteranceLabel",
    "settings.advanced.tts.composer.sendUtteranceDescription",
    [
      "send utterance",
      "transcript",
      "my words",
      "user speech",
      "privacy",
      "what i said",
    ],
  ),
  row(
    "spokenReplyFallback",
    "conversation",
    "settings.advanced.tts.composer.fallbackLabel",
    "settings.advanced.tts.composer.fallbackDescription",
    [
      "fallback",
      "failure",
      "error",
      "short notice",
      "verbatim",
      "silent",
      "on failure",
    ],
  ),
  row(
    "spokenReplyPreview",
    "conversation",
    "settings.advanced.tts.composer.previewLabel",
    "settings.advanced.tts.composer.previewDescription",
    ["preview", "sample", "try", "compose", "test"],
  ),

  // ----- conversation section (ConversationSection) -----
  row(
    "conversationMode",
    "conversation",
    "converse.section.title",
    "converse.section.description",
    [
      "conversation",
      "converse",
      "talk",
      "voice conversation",
      "continuous conversation",
      "start conversation",
      "chat",
    ],
  ),
  row(
    "conversationSilence",
    "conversation",
    "converse.section.silenceLabel",
    "converse.section.silenceDescription",
    [
      "silence",
      "silence timeout",
      "pause",
      "end of turn",
      "silence duration",
      "milliseconds",
      "wait",
    ],
  ),
  row(
    "conversationIdleTurns",
    "conversation",
    "converse.section.idleTurnsLabel",
    "converse.section.idleTurnsDescription",
    ["idle", "idle turns", "inactivity", "timeout", "turns", "stop when idle"],
  ),
  row(
    "conversationContextTurns",
    "conversation",
    "converse.section.contextTurnsLabel",
    "converse.section.contextTurnsDescription",
    [
      "context",
      "context turns",
      "history",
      "turns",
      "memory",
      "conversation history",
    ],
  ),
  row(
    "conversationShortcut",
    "conversation",
    "converse.section.shortcutLabel",
    "converse.section.shortcutDescription",
    [
      "shortcut",
      "hotkey",
      "keybinding",
      "keyboard shortcut",
      "conversation shortcut",
      "binding",
    ],
  ),
  row(
    "conversationChatLog",
    "conversation",
    "settings.conversation.chatLog.title",
    "settings.conversation.chatLog.description",
    ["chat", "chat log", "transcript", "conversation window", "log", "history"],
  ),
  row(
    "conversationChatOpenOnStart",
    "conversation",
    "settings.conversation.chatOpenOnStart.label",
    "settings.conversation.chatOpenOnStart.description",
    [
      "conversation window",
      "open window",
      "hey jarvis",
      "show chat",
      "auto open",
      "open on start",
    ],
  ),
  row(
    "conversationChatWindow",
    "conversation",
    "settings.conversation.openChatWindow",
    "settings.conversation.chatLog.description",
    [
      "chat window",
      "open chat",
      "conversation window",
      "transcript",
      "show chat",
    ],
  ),
  row(
    "converseBargeIn",
    "conversation",
    "converse.section.bargeIn.label",
    "converse.section.bargeIn.description",
    [
      "interrupt",
      "barge in",
      "talk over",
      "headphones",
      "interrupt assistant",
      "speak over",
    ],
  ),
  row(
    "conversationSmartTurn",
    "vadEngine",
    "converse.voice.smartTurn.label",
    "converse.voice.smartTurn.description",
    [
      "smart turn",
      "turn detection",
      "end of turn",
      "endpoint",
      "faster replies",
      "model",
    ],
  ),
  row(
    "conversationSmartTurnModel",
    "vadEngine",
    "converse.voice.smartTurn.modelLabel",
    undefined,
    ["smart turn model", "download", "delete", "install"],
  ),
  row(
    "conversationLatency",
    "conversation",
    "converse.voice.latency.label",
    "converse.voice.latency.description",
    ["latency", "timing", "speed", "milliseconds", "panel", "median"],
  ),
  row(
    "conversationPersona",
    "conversation",
    "converse.voice.persona.label",
    "converse.voice.persona.description",
    ["persona", "style", "voice preset", "personality", "custom persona"],
  ),
  row(
    "conversationSpeakerFilter",
    "vadEngine",
    "converse.voice.speaker.label",
    "converse.voice.speaker.description",
    [
      "speaker filter",
      "voiceprint",
      "my voice",
      "other voices",
      "convenience filter",
      "ignore others",
    ],
  ),
  row(
    "conversationSpeakerSensitivity",
    "vadEngine",
    "converse.voice.speaker.sensitivityLabel",
    "converse.voice.speaker.sensitivityDescription",
    ["sensitivity", "strict", "lenient", "balanced", "speaker"],
  ),
  row(
    "conversationSpeakerModel",
    "vadEngine",
    "converse.voice.speaker.modelLabel",
    undefined,
    ["speaker model", "download", "delete", "install"],
  ),
  row(
    "conversationSpeakerEnroll",
    "vadEngine",
    "converse.voice.speaker.enroll.label",
    "converse.voice.speaker.enroll.description",
    ["enroll", "enrollment", "voiceprint", "record voice", "train voice"],
  ),
  row(
    "conversationSpeakerDelete",
    "vadEngine",
    "converse.voice.speaker.deleteLabel",
    "converse.voice.speaker.deleteDescription",
    ["delete voiceprint", "forget voice", "voiceprint"],
  ),

  // ----- realtime (RealtimeSection) -----
  row(
    "realtimeEnabled",
    "conversation",
    "settings.experimental.realtime.enabled.label",
    "settings.experimental.realtime.enabled.description",
    [
      "realtime",
      "real time",
      "realtime voice",
      "streaming",
      "live voice",
      "speech to speech",
    ],
  ),
  row(
    "realtimeDirectEndpoint",
    "conversation",
    "settings.experimental.realtime.directEndpoint.label",
    "settings.experimental.realtime.directEndpoint.description",
    [
      "direct endpoint",
      "endpoint",
      "custom endpoint",
      "server",
      "connection",
      "api",
    ],
  ),
  row(
    "realtimeEndpointUrl",
    "conversation",
    "settings.experimental.realtime.endpoint.label",
    "settings.experimental.realtime.endpoint.description",
    ["endpoint", "url", "endpoint url", "server url", "address", "api url"],
  ),
  row(
    "realtimeModel",
    "conversation",
    "settings.experimental.realtime.model.label",
    "settings.experimental.realtime.model.description",
    ["model", "model name", "realtime model"],
  ),
  row(
    "realtimeToken",
    "conversation",
    "settings.experimental.realtime.token.label",
    "settings.experimental.realtime.token.description",
    ["token", "api key", "key", "password", "secret", "authentication", "auth"],
  ),
  row(
    "realtimeStreamReply",
    "conversation",
    "settings.experimental.realtime.streamReply.label",
    "settings.experimental.realtime.streamReply.description",
    ["stream", "streaming reply", "stream reply", "partial", "latency"],
  ),
  row(
    "realtimeEarlySpeech",
    "conversation",
    "settings.experimental.realtime.earlySpeech.label",
    "settings.experimental.realtime.earlySpeech.description",
    [
      "early speech",
      "early",
      "start speaking early",
      "latency",
      "speech early",
    ],
  ),
  row(
    "realtimeFastVoice",
    "conversation",
    "settings.experimental.realtime.fastVoice.label",
    "settings.experimental.realtime.fastVoice.description",
    ["fast voice", "fast", "latency", "quick voice", "speed"],
  ),
  row(
    "realtimeAck",
    "conversation",
    "settings.experimental.realtime.ack.label",
    "settings.experimental.realtime.ack.description",
    ["acknowledge", "acknowledgement", "ack", "filler", "backchannel"],
  ),
  row(
    "realtimePrewarm",
    "conversation",
    "settings.experimental.realtime.prewarm.label",
    "settings.experimental.realtime.prewarm.description",
    ["prewarm", "warm up", "preconnect", "latency", "connection"],
  ),
  row(
    "realtimeAutoProxy",
    "conversation",
    "settings.experimental.realtime.autoProxy.label",
    "settings.experimental.realtime.autoProxy.description",
    ["auto proxy", "proxy", "automatic proxy", "network", "proxy detection"],
  ),
  row(
    "realtimeProxy",
    "conversation",
    "settings.experimental.realtime.proxy.label",
    "settings.experimental.realtime.proxy.description",
    ["proxy", "network proxy", "proxy server", "http proxy"],
  ),
  row(
    "realtimeProxyUrl",
    "conversation",
    "settings.experimental.realtime.proxyUrl.label",
    "settings.experimental.realtime.proxyUrl.description",
    ["proxy url", "proxy address", "proxy", "url", "server address"],
  ),
  row(
    "realtimeTimingLog",
    "conversation",
    "settings.experimental.realtime.timingLog.label",
    "settings.experimental.realtime.timingLog.description",
    ["timing", "timing log", "logs", "latency log", "debug log", "diagnostics"],
  ),

  // ----- conversation memory -----
  row(
    "conversationRecords",
    "conversationMemory",
    "settings.experimental.conversationMemory.recordsLabel",
    "settings.experimental.conversationMemory.recordsDescription",
    [
      "records",
      "conversation records",
      "save conversations",
      "transcripts",
      "history",
      "log",
      "conversation log",
    ],
  ),
  row(
    "conversationVault",
    "conversationMemory",
    "settings.experimental.conversationMemory.vaultToggleLabel",
    "settings.experimental.conversationMemory.vaultToggleDescription",
    [
      "vault",
      "obsidian",
      "notes",
      "notes vault",
      "save to notes",
      "markdown",
      "obsidian vault",
    ],
  ),
  row(
    "conversationVaultPath",
    "conversationMemory",
    "settings.experimental.conversationMemory.vaultPathLabel",
    "settings.experimental.conversationMemory.vaultPathDescription",
    [
      "obsidian",
      "notes",
      "vault",
      "vault path",
      "folder",
      "path",
      "notes folder",
      "markdown folder",
    ],
  ),
  row(
    "conversationVaultContext",
    "conversationMemory",
    "settings.experimental.conversationMemory.contextLabel",
    "settings.experimental.conversationMemory.contextDescription",
    [
      "vault context",
      "obsidian",
      "notes",
      "context",
      "notes context",
      "use notes",
    ],
  ),
  row(
    "rememberConversations",
    "conversationMemory",
    "settings.experimental.rememberConversations.label",
    "settings.experimental.rememberConversations.description",
    [
      "remember conversations",
      "remember",
      "memory",
      "past conversations",
      "recall",
      "conversation memory",
    ],
  ),
  row(
    "rememberConversationsClear",
    "conversationMemory",
    "settings.experimental.rememberConversations.clearTitle",
    "settings.experimental.rememberConversations.clearDescription",
    [
      "clear conversations",
      "forget conversations",
      "delete remembered conversations",
      "erase memory",
      "remembered",
    ],
  ),
  row(
    "vaultRagEnable",
    "conversationMemory",
    "settings.experimental.vaultRag.enable.label",
    "settings.experimental.vaultRag.enable.description",
    ["vault", "obsidian", "notes", "search", "retrieval", "rag", "markdown"],
  ),
  row(
    "vaultRagFolders",
    "conversationMemory",
    "settings.experimental.vaultRag.folders.label",
    "settings.experimental.vaultRag.folders.description",
    ["vault folders", "folders", "obsidian", "notes", "index", "markdown"],
  ),
  row(
    "vaultRagStatus",
    "conversationMemory",
    "settings.experimental.vaultRag.title",
    "settings.experimental.vaultRag.description",
    ["vault status", "reindex", "re-index", "tool support", "retrieval", "rag"],
  ),

  // ----- agent tools -----
  row(
    "agentTools",
    "agentTools",
    "settings.experimental.agentTools.enabledLabel",
    "settings.experimental.agentTools.enabledDescription",
    [
      "agent tools",
      "tools",
      "function calling",
      "file access",
      "actions",
      "tool use",
    ],
  ),
  row(
    "agentToolsGuard",
    "agentTools",
    "settings.experimental.agentTools.guardLabel",
    "settings.experimental.agentTools.guardDescription",
    ["guard", "safety", "confirm", "approval", "protection", "ask before"],
  ),
  row(
    "agentToolsProjectRoots",
    "agentTools",
    "settings.experimental.agentTools.projectRootsLabel",
    "settings.experimental.agentTools.projectRootsDescription",
    [
      "project roots",
      "project folders",
      "folders",
      "directories",
      "workspace",
      "allowed folders",
      "repositories",
    ],
  ),

  // ----- coding jobs -----
  row(
    "codingJobsEnabled",
    "codingJobs",
    "settings.experimental.jobs.enabledLabel",
    "settings.experimental.jobs.enabledDescription",
    [
      "coding jobs",
      "jobs",
      "agent jobs",
      "background jobs",
      "run tasks",
      "coding agent",
    ],
  ),
  row(
    "codingJobsTimeout",
    "codingJobs",
    "settings.experimental.jobs.timeoutLabel",
    "settings.experimental.jobs.timeoutDescription",
    ["timeout", "job timeout", "minutes", "time limit", "timeout minutes"],
  ),
  row(
    "codingJobsConfirm",
    "codingJobs",
    "settings.experimental.jobs.confirmLabel",
    "settings.experimental.jobs.confirmDescription",
    ["confirm", "confirmation", "approval", "approve", "ask before running"],
  ),
  row(
    "codingJobsAsk",
    "codingJobs",
    "settings.experimental.jobs.ask.label",
    "settings.experimental.jobs.ask.description",
    ["ask", "ask before", "question", "prompt", "approval"],
  ),
  row(
    "codingJobsAgents",
    "codingJobs",
    "settings.experimental.jobs.agents.label",
    "settings.experimental.jobs.agents.description",
    ["agents", "agent names", "allowlist", "allowed agents", "agent list"],
  ),
  row(
    "codingJobsAgentFlag",
    "codingJobs",
    "settings.experimental.jobs.flag.label",
    "settings.experimental.jobs.flag.description",
    ["flag", "agent flag", "feature flag", "enable agents", "jobs flag"],
  ),
];

/** Non-row translation keys on this page, each with its reason. */
export const EXPERIMENTAL_EXCLUDED: Readonly<Record<string, string>> = {
  "settings.advanced.voiceShortcuts.phraseLabel":
    "aria-label of the phrase text box inside a voice shortcut row; the row itself is voiceShortcuts.title",
  "settings.advanced.voiceShortcuts.typeLabel":
    "aria-label of the action type select inside a voice shortcut row, not a separate setting",
  "settings.advanced.voiceShortcuts.targetLabel":
    "aria-label of the target text box inside a voice shortcut row, not a separate setting",
  "settings.advanced.wakeWord.disabledByAmbientMode":
    "tooltip on the disabled wake word toggle explaining the conflict with ambient mode",
  "settings.advanced.ambientMode.disabledByWakeWord":
    "tooltip on the disabled ambient mode toggle explaining the conflict with wake word",
  ...excludeFamily(
    "settings.advanced.voiceShortcuts.actionType.",
    SHORTCUT_ACTION_IDS,
    "caption of an action type option in the voice shortcut select",
  ),
  ...excludeFamily(
    "settings.advanced.voiceShortcuts.placeholder.",
    SHORTCUT_ACTION_IDS,
    "placeholder text of the voice shortcut target box, not a row title",
  ),
  ...excludeFamily(
    `${COMPOSER_PREFIX}mode.`,
    COMPOSER_MODE_IDS,
    "caption of a mode option in the spoken reply mode select",
  ),
  ...excludeFamily(
    `${COMPOSER_PREFIX}fallback.`,
    COMPOSER_FALLBACK_IDS,
    "caption of a fallback option in the spoken reply fallback select",
  ),
  ...excludeFamily(
    `${COMPOSER_PREFIX}readiness.`,
    COMPOSER_READINESS_IDS,
    "readiness status text shown next to the save command button",
  ),
  ...excludeFamily(
    `${COMPOSER_PREFIX}emotion.`,
    COMPOSER_EMOTION_IDS,
    "emotion name shown in the spoken reply preview result",
  ),
  ...excludeFamily(
    `${COMPOSER_PREFIX}status.`,
    COMPOSER_STATUS_IDS,
    "outcome status text of the most recent spoken reply, not a setting",
  ),
  ...excludeFamily(
    "converse.state.",
    CONVERSE_STATE_IDS,
    "conversation state text in the conversation status line, not a setting",
  ),
  ...excludeFamily(
    "converse.end.",
    CONVERSE_END_IDS,
    "reason text shown under the conversation button after a session ends",
  ),
  ...excludeFamily(
    "settings.experimental.jobs.state.",
    JOB_STATE_IDS,
    "job state text in the coding job status panel, not a setting",
  ),
  ...excludeFamily(
    `${REALTIME_PREFIX}status.`,
    REALTIME_STATUS_IDS,
    "endpoint readiness message under the realtime fields, not a setting",
  ),
  ...excludeFamily(
    `${REALTIME_PREFIX}errors.`,
    REALTIME_ERROR_IDS,
    "validation message shown under a realtime text field, not a setting",
  ),
};

export const EXPERIMENTAL_DYNAMIC: readonly DynamicFamily[] = [
  {
    source: TTS_SOURCE,
    prefix: "settings.advanced.tts.natural.",
    ids: [],
    note: "Natural voice option labels (accent./gender. parts), the failure.<code> detail text from failureI18nKey and the system voice description key from buildVoiceOptions are inline text inside the indexed ttsVoice / ttsNaturalVoice / ttsNaturalModel rows, not separate settings.",
  },
  {
    source: PLACED_GROUPS_SOURCE,
    prefix: "settings.experimental.groups.",
    ids: [],
    note: "Card titles and descriptions come from settingsPlacement.ts (PLACED_GROUPS titleKey, descriptionKey); each card title is indexed as the groupKey of its rows, so no row label is built here.",
  },
  {
    source: SPOKEN_REPLY_SOURCE,
    prefix: `${COMPOSER_PREFIX}mode.`,
    ids: COMPOSER_MODE_IDS,
  },
  {
    source: SPOKEN_REPLY_SOURCE,
    prefix: `${COMPOSER_PREFIX}fallback.`,
    ids: COMPOSER_FALLBACK_IDS,
  },
  {
    source: SPOKEN_REPLY_SOURCE,
    prefix: `${COMPOSER_PREFIX}readiness.`,
    ids: COMPOSER_READINESS_IDS,
  },
  {
    source: SPOKEN_REPLY_SOURCE,
    prefix: `${COMPOSER_PREFIX}emotion.`,
    ids: COMPOSER_EMOTION_IDS,
  },
  {
    source: SPOKEN_REPLY_SOURCE,
    prefix: `${COMPOSER_PREFIX}status.`,
    ids: COMPOSER_STATUS_IDS,
  },
  {
    source: VOICE_SHORTCUTS_SOURCE,
    prefix: "settings.advanced.voiceShortcuts.actionType.",
    ids: SHORTCUT_ACTION_IDS,
  },
  {
    source: VOICE_SHORTCUTS_SOURCE,
    prefix: "settings.advanced.voiceShortcuts.placeholder.",
    ids: SHORTCUT_ACTION_IDS,
  },
  {
    source: REALTIME_SOURCE,
    prefix: REALTIME_PREFIX,
    suffix: ".label",
    ids: REALTIME_LABEL_IDS,
  },
  {
    source: REALTIME_SOURCE,
    prefix: `${REALTIME_PREFIX}status.`,
    ids: REALTIME_STATUS_IDS,
  },
  {
    source: REALTIME_SOURCE,
    prefix: `${REALTIME_PREFIX}errors.`,
    ids: REALTIME_ERROR_IDS,
  },
  {
    source: CONVERSATION_SOURCE,
    prefix: "converse.state.",
    ids: CONVERSE_STATE_IDS,
  },
  {
    source: CONVERSATION_SOURCE,
    prefix: "converse.end.",
    ids: CONVERSE_END_IDS,
  },
  {
    source: CONVERSATION_SOURCE,
    prefix: "converse.section.",
    suffix: "Label",
    ids: CONVERSE_SLIDER_IDS,
  },
  {
    source: JOBS_SOURCE,
    prefix: "settings.experimental.jobs.state.",
    ids: JOB_STATE_IDS,
  },
];
