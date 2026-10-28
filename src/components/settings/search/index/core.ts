import type { DynamicFamily, SearchEntry } from "../types";

// Search index for the pages rendered outside the Experimental page:
// general, history, models, advanced, postprocessing, debug, about.
// Keys are checked by settingsSearchIndex.test.ts; keep labelKey equal to the
// title/label key passed to SettingContainer/ToggleSwitch/Dropdown/Slider.

const SHORTCUT_BINDING_PREFIX = "settings.general.shortcut.bindings";

export const CORE_ENTRIES: readonly SearchEntry[] = [
  // ---------- General ----------
  {
    id: "general.transcribeShortcut",
    section: "general",
    groupKey: "settings.general.title",
    labelKey: `${SHORTCUT_BINDING_PREFIX}.transcribe.name`,
    descriptionKey: `${SHORTCUT_BINDING_PREFIX}.transcribe.description`,
    keywords: ["hotkey", "keybind", "keyboard", "record", "start recording"],
  },
  {
    id: "general.cancelShortcut",
    section: "general",
    groupKey: "settings.general.title",
    labelKey: `${SHORTCUT_BINDING_PREFIX}.cancel.name`,
    descriptionKey: `${SHORTCUT_BINDING_PREFIX}.cancel.description`,
    keywords: ["abort", "stop recording", "hotkey", "escape"],
    // Hidden on Linux (GeneralSettings: !isLinux).
    platforms: ["macos", "windows"],
  },
  {
    id: "general.shortcutBehavior",
    section: "general",
    groupKey: "settings.general.title",
    labelKey: "settings.general.shortcutActivation.title",
    descriptionKey: "settings.general.shortcutActivation.description",
    keywords: ["hold", "toggle", "push to talk", "tap", "activation", "mode"],
  },
  {
    id: "general.theme",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.title",
    descriptionKey: "theme.description",
    keywords: [
      "dark mode",
      "light mode",
      "color scheme",
      "dark",
      "light",
      "system theme",
      "aurora",
    ],
  },
  {
    id: "general.customThemeText",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.text",
    descriptionKey: "theme.custom.description",
    keywords: ["font color", "foreground", "text color", "colour"],
  },
  {
    id: "general.customThemeBackground",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.background",
    descriptionKey: "theme.custom.description",
    keywords: ["window color", "surface", "background color", "colour"],
  },
  {
    id: "general.customThemeAccent",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.accent",
    descriptionKey: "theme.custom.description",
    keywords: ["highlight", "primary color", "accent color", "colour"],
  },
  {
    id: "general.customThemeAccentSecondary",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.accentSecondary",
    descriptionKey: "theme.custom.description",
    keywords: ["second highlight", "secondary color", "colour"],
  },
  {
    id: "general.customThemeWarning",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.warning",
    descriptionKey: "theme.custom.description",
    keywords: ["caution", "alert color", "yellow", "colour"],
  },
  {
    id: "general.customThemeError",
    section: "general",
    groupKey: "settings.appearance.title",
    labelKey: "theme.custom.error",
    descriptionKey: "theme.custom.description",
    keywords: ["failure color", "red", "danger", "colour"],
  },
  {
    id: "models.language",
    section: "models",
    labelKey: "settings.general.language.title",
    descriptionKey: "settings.general.language.description",
    keywords: [
      "speech language",
      "locale",
      "transcription language",
      "dialect",
      "recognition language",
    ],
  },
  {
    id: "models.translateToEnglish",
    section: "models",
    labelKey: "settings.advanced.translateToEnglish.label",
    descriptionKey: "settings.advanced.translateToEnglish.description",
    keywords: ["translation", "english", "translate", "foreign language"],
  },
  {
    id: "general.microphone",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.sound.microphone.title",
    descriptionKey: "settings.sound.microphone.description",
    keywords: ["mic", "input device", "recording device", "audio input"],
  },
  {
    // Rendered only when the selected device has more than one channel.
    id: "general.inputChannel",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.sound.channel.title",
    descriptionKey: "settings.sound.channel.description",
    keywords: ["channel", "mono", "stereo", "audio interface"],
  },
  {
    id: "general.muteWhileRecording",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.debug.muteWhileRecording.label",
    descriptionKey: "settings.debug.muteWhileRecording.description",
    keywords: ["mute", "silence", "system audio", "sound off"],
  },
  {
    id: "general.audioFeedback",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.sound.audioFeedback.label",
    descriptionKey: "settings.sound.audioFeedback.description",
    keywords: ["beep", "chime", "start sound", "stop sound", "sound effects"],
  },
  {
    id: "general.outputDevice",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.sound.outputDevice.title",
    descriptionKey: "settings.sound.outputDevice.description",
    keywords: ["speaker", "headphones", "playback", "sound output"],
  },
  {
    id: "general.volume",
    section: "general",
    groupKey: "settings.sound.title",
    labelKey: "settings.sound.volume.title",
    descriptionKey: "settings.sound.volume.description",
    keywords: ["loudness", "sound level", "feedback volume", "audio level"],
  },

  // ---------- History (page only: the page has no setting rows) ----------
  {
    id: "history.page",
    section: "history",
    labelKey: "sidebar.history",
    keywords: ["transcriptions", "past recordings", "search history", "retry"],
    pageOnly: true,
  },

  // ---------- Models (page only: the page has no setting rows) ----------
  {
    id: "models.page",
    section: "models",
    labelKey: "sidebar.models",
    keywords: [
      "download models",
      "whisper",
      "parakeet",
      "speech model",
      "engine",
    ],
    pageOnly: true,
  },

  // ---------- Advanced ----------
  {
    id: "advanced.startHidden",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.startHidden.label",
    descriptionKey: "settings.advanced.startHidden.description",
    keywords: [
      "tray",
      "background",
      "minimized",
      "silent start",
      "hidden window",
    ],
  },
  {
    id: "advanced.launchOnStartup",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.autostart.label",
    descriptionKey: "settings.advanced.autostart.description",
    keywords: ["login", "autostart", "boot", "start at login", "auto start"],
  },
  {
    id: "advanced.showTrayIcon",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.showTrayIcon.label",
    descriptionKey: "settings.advanced.showTrayIcon.description",
    keywords: ["menu bar", "system tray", "taskbar", "icon"],
  },
  {
    id: "advanced.overlayStyle",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.overlay.style.title",
    descriptionKey: "settings.advanced.overlay.style.description",
    keywords: [
      "recording overlay",
      "indicator",
      "hud",
      "minimal",
      "live preview",
    ],
  },
  {
    // Rendered only when the overlay style is not "None".
    id: "advanced.overlayPosition",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.overlay.position.title",
    descriptionKey: "settings.advanced.overlay.position.description",
    keywords: ["top", "bottom", "screen position", "where"],
  },
  {
    id: "models.modelUnload",
    section: "models",
    groupKey: "settings.models.groups.speechToText",
    labelKey: "settings.advanced.modelUnload.title",
    descriptionKey: "settings.advanced.modelUnload.description",
    keywords: ["memory", "gpu memory", "ram", "idle", "free memory", "timeout"],
  },
  {
    id: "advanced.experimentalToggle",
    section: "advanced",
    groupKey: "settings.advanced.groups.app",
    labelKey: "settings.advanced.experimentalToggle.label",
    descriptionKey: "settings.advanced.experimentalToggle.description",
    keywords: ["beta", "lab", "preview features", "unstable", "early access"],
  },
  {
    id: "advanced.pasteMethod",
    section: "advanced",
    groupKey: "settings.advanced.groups.output",
    labelKey: "settings.advanced.pasteMethod.title",
    descriptionKey: "settings.advanced.pasteMethod.description",
    keywords: ["paste", "insert text", "ctrl+v", "direct typing", "keystroke"],
  },
  {
    id: "advanced.typingTool",
    section: "advanced",
    groupKey: "settings.advanced.groups.output",
    labelKey: "settings.advanced.typingTool.title",
    descriptionKey: "settings.advanced.typingTool.description",
    keywords: ["xdotool", "wtype", "ydotool", "linux typing", "keystrokes"],
    platforms: ["linux"],
  },
  {
    id: "advanced.clipboardHandling",
    section: "advanced",
    groupKey: "settings.advanced.groups.output",
    labelKey: "settings.advanced.clipboardHandling.title",
    descriptionKey: "settings.advanced.clipboardHandling.description",
    keywords: ["clipboard", "copy", "restore clipboard", "preserve clipboard"],
  },
  {
    id: "advanced.autoSubmit",
    section: "advanced",
    groupKey: "settings.advanced.groups.output",
    labelKey: "settings.advanced.autoSubmit.title",
    descriptionKey: "settings.advanced.autoSubmit.description",
    keywords: ["enter key", "send", "submit", "press enter", "auto send"],
  },
  {
    id: "advanced.voiceActivityDetection",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.advanced.voiceActivityDetection.title",
    descriptionKey: "settings.advanced.voiceActivityDetection.description",
    keywords: ["vad", "silence", "silence detection", "speech detection"],
  },
  {
    id: "advanced.fillerWordRemoval",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.advanced.fillerWordRemoval.title",
    descriptionKey: "settings.advanced.fillerWordRemoval.description",
    keywords: ["hesitation", "stutter", "verbal tics", "clean up", "filler"],
  },
  {
    id: "advanced.customFillerWords",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.advanced.fillerWordRemoval.customLabel",
    descriptionKey: "settings.advanced.fillerWordRemoval.customHelp",
    keywords: [
      "custom filler",
      "filler list",
      "filler words",
      "built-in list",
      "keep every word",
      "stop words",
    ],
  },
  {
    id: "advanced.chineseScript",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.advanced.chineseScript.title",
    descriptionKey: "settings.advanced.chineseScript.description",
    keywords: ["simplified", "traditional", "mandarin", "cantonese", "hanzi"],
  },
  {
    id: "advanced.customWords",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.advanced.customWords.title",
    descriptionKey: "settings.advanced.customWords.description",
    keywords: [
      "dictionary",
      "vocabulary",
      "names",
      "jargon",
      "spelling corrections",
    ],
  },
  {
    id: "advanced.appendTrailingSpace",
    section: "advanced",
    groupKey: "settings.advanced.groups.transcription",
    labelKey: "settings.debug.appendTrailingSpace.label",
    descriptionKey: "settings.debug.appendTrailingSpace.description",
    keywords: ["space", "after paste", "spacing", "separator"],
  },
  {
    id: "advanced.historyLimit",
    section: "advanced",
    groupKey: "settings.advanced.groups.history",
    labelKey: "settings.debug.historyLimit.title",
    descriptionKey: "settings.debug.historyLimit.description",
    keywords: ["entries", "how many", "max history", "limit", "history size"],
  },
  {
    id: "advanced.recordingRetention",
    section: "advanced",
    groupKey: "settings.advanced.groups.history",
    labelKey: "settings.debug.recordingRetention.title",
    descriptionKey: "settings.debug.recordingRetention.description",
    keywords: [
      "delete recordings",
      "auto delete",
      "cleanup",
      "disk space",
      "old audio",
    ],
  },

  // ---------- Post Process (unlocked by the experimental toggle) ----------
  {
    id: "postprocessing.hotkey",
    section: "postprocessing",
    groupKey: "settings.postProcessing.hotkey.title",
    labelKey: `${SHORTCUT_BINDING_PREFIX}.transcribe_with_post_process.name`,
    descriptionKey: `${SHORTCUT_BINDING_PREFIX}.transcribe_with_post_process.description`,
    keywords: ["refine hotkey", "ai hotkey", "llm shortcut", "second shortcut"],
    unlockedBy: "experimental.postProcessing",
  },
  {
    id: "postprocessing.provider",
    section: "postprocessing",
    groupKey: "settings.postProcessing.api.title",
    labelKey: "settings.postProcessing.api.provider.title",
    descriptionKey: "settings.postProcessing.api.provider.description",
    keywords: ["openai compatible", "llm", "service", "vendor", "backend"],
    unlockedBy: "experimental.postProcessing",
  },
  {
    // Rendered only when the provider is "custom".
    id: "postprocessing.baseUrl",
    section: "postprocessing",
    groupKey: "settings.postProcessing.api.title",
    labelKey: "settings.postProcessing.api.baseUrl.title",
    descriptionKey: "settings.postProcessing.api.baseUrl.description",
    keywords: [
      "endpoint",
      "url",
      "custom endpoint",
      "server address",
      "api url",
    ],
    unlockedBy: "experimental.postProcessing",
  },
  {
    id: "postprocessing.apiKey",
    section: "postprocessing",
    groupKey: "settings.postProcessing.api.title",
    labelKey: "settings.postProcessing.api.apiKey.title",
    descriptionKey: "settings.postProcessing.api.apiKey.description",
    keywords: ["token", "secret", "password", "authentication", "credentials"],
    unlockedBy: "experimental.postProcessing",
  },
  {
    id: "postprocessing.model",
    section: "postprocessing",
    groupKey: "settings.postProcessing.api.title",
    labelKey: "settings.postProcessing.api.model.title",
    descriptionKey: "settings.postProcessing.api.model.descriptionDefault",
    keywords: ["llm model", "gpt", "model name", "identifier"],
    unlockedBy: "experimental.postProcessing",
  },
  {
    id: "postprocessing.selectedPrompt",
    section: "postprocessing",
    groupKey: "settings.postProcessing.prompts.title",
    labelKey: "settings.postProcessing.prompts.selectedPrompt.title",
    descriptionKey:
      "settings.postProcessing.prompts.selectedPrompt.description",
    keywords: [
      "template",
      "instructions",
      "system prompt",
      "rewrite",
      "prompt template",
    ],
    unlockedBy: "experimental.postProcessing",
  },

  // ---------- Debug ----------
  {
    id: "debug.logLevel",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.logLevel.title",
    descriptionKey: "settings.debug.logLevel.description",
    keywords: ["verbosity", "logging", "verbose", "trace", "debug logs"],
  },
  {
    id: "debug.whatsNewPreview",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.whatsNewPreview.title",
    descriptionKey: "settings.debug.whatsNewPreview.description",
    keywords: ["release notes", "changelog", "preview", "changes"],
  },
  {
    // Rendered only when the page receives onPreviewOnboarding.
    id: "debug.onboardingPreview",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.onboardingPreview.title",
    descriptionKey: "settings.debug.onboardingPreview.description",
    keywords: ["setup", "first run", "welcome", "tutorial", "wizard"],
  },
  {
    id: "debug.updateChecks",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.updateChecks.label",
    descriptionKey: "settings.debug.updateChecks.description",
    keywords: ["updates", "auto update", "version check", "upgrade"],
  },
  {
    // SoundPicker renders a SettingContainer titled with this label and a sound theme dropdown.
    id: "debug.soundTheme",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.soundTheme.label",
    descriptionKey: "settings.debug.soundTheme.description",
    keywords: ["sound pack", "chime", "beep", "sound effects", "marimba"],
  },
  {
    id: "debug.wordCorrectionThreshold",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.wordCorrectionThreshold.title",
    descriptionKey: "settings.debug.wordCorrectionThreshold.description",
    keywords: ["fuzzy", "custom words", "spelling", "correction sensitivity"],
  },
  {
    id: "debug.pasteDelay",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.pasteDelay.title",
    descriptionKey: "settings.debug.pasteDelay.description",
    keywords: ["delay", "paste timing", "latency", "wait before paste"],
  },
  {
    id: "debug.pasteDelayAfter",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.pasteDelayAfter.title",
    descriptionKey: "settings.debug.pasteDelayAfter.description",
    keywords: [
      "delay",
      "paste timing",
      "latency",
      "wait after paste",
      "restore clipboard",
    ],
  },
  {
    id: "debug.reliablePaste",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.reliablePaste.title",
    descriptionKey: "settings.debug.reliablePaste.description",
    keywords: [
      "clipboard restore",
      "paste reliability",
      "beta",
      "clipboard timing",
    ],
    // ReliablePasteToggle renders only on macOS and Windows.
    platforms: ["macos", "windows"],
  },
  {
    id: "debug.holdThreshold",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.holdThreshold.title",
    descriptionKey: "settings.debug.holdThreshold.description",
    keywords: [
      "press and hold",
      "hold time",
      "push to talk threshold",
      "auto mode",
    ],
  },
  {
    id: "debug.recordingBuffer",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.recordingBuffer.title",
    descriptionKey: "settings.debug.recordingBuffer.description",
    keywords: [
      "buffer",
      "tail",
      "trailing audio",
      "cut off",
      "end of recording",
    ],
  },
  {
    id: "debug.alwaysOnMicrophone",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.alwaysOnMicrophone.label",
    descriptionKey: "settings.debug.alwaysOnMicrophone.description",
    keywords: ["mic", "microphone", "standby", "fast start", "response time"],
  },
  {
    // Rendered only on laptops.
    id: "debug.clamshellMicrophone",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.clamshellMicrophone.title",
    descriptionKey: "settings.debug.clamshellMicrophone.description",
    keywords: ["lid closed", "laptop lid", "laptop", "closed lid"],
  },
  {
    id: "debug.liveLogs",
    section: "debug",
    groupKey: "settings.debug.title",
    labelKey: "settings.debug.liveLogs.title",
    descriptionKey: "settings.debug.liveLogs.description",
    keywords: ["logs", "console", "log viewer", "stream logs", "diagnostics"],
  },

  // ---------- About ----------
  {
    id: "about.appLanguage",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "appLanguage.title",
    descriptionKey: "appLanguage.description",
    keywords: ["language", "locale", "interface language", "ui language"],
  },
  {
    id: "about.version",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "settings.about.version.title",
    descriptionKey: "settings.about.version.description",
    keywords: ["build", "release", "app version", "current version"],
  },
  {
    id: "about.whatsNewUpdates",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "settings.about.whatsNewUpdates.label",
    descriptionKey: "settings.about.whatsNewUpdates.description",
    keywords: ["release notes", "changelog", "updates", "changes after update"],
  },
  {
    id: "about.sourceCode",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "settings.about.sourceCode.title",
    descriptionKey: "settings.about.sourceCode.description",
    keywords: ["github", "repository", "code", "open source", "contribute"],
  },
  {
    id: "about.appDataDirectory",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "settings.about.appDataDirectory.title",
    descriptionKey: "settings.about.appDataDirectory.description",
    keywords: [
      "data folder",
      "storage location",
      "settings folder",
      "app folder",
    ],
  },
  {
    id: "about.logDirectory",
    section: "about",
    groupKey: "settings.about.title",
    labelKey: "settings.debug.logDirectory.title",
    descriptionKey: "settings.debug.logDirectory.description",
    keywords: ["logs folder", "log files", "log location", "open logs"],
  },
  {
    id: "about.ggml",
    section: "about",
    groupKey: "settings.about.acknowledgments.title",
    labelKey: "settings.about.acknowledgments.ggml.title",
    descriptionKey: "settings.about.acknowledgments.ggml.description",
    keywords: [
      "credits",
      "thanks",
      "tensor library",
      "attribution",
      "machine learning library",
    ],
  },
];

/**
 * Translation keys rendered as title/label attributes that are not settings rows.
 * Each reason names the control kind so a reviewer can verify it.
 */
export const CORE_EXCLUDED: Readonly<Record<string, string>> = {
  "settings.modelSettings.title":
    'card title is templated with the active model name ("{{model}} Settings"), so it has no stable text to search or show; its rows are indexed directly under the General page',
  "settings.general.shortcut.title":
    "fallback row title shown only while shortcut bindings load or are missing; the searchable rows are the binding names",
  "settings.history.copyToClipboard":
    "tooltip of an icon button on one history entry; a per-entry action, not a setting",
  "settings.history.retranscribe":
    "tooltip of an icon button that re-runs transcription on one history entry; an action, not a setting",
  "settings.history.delete":
    "tooltip of an icon button that deletes one history entry; an action, not a setting",
  "settings.history.openFolder":
    "caption of the button that opens the recordings folder; an action, not a setting",
  "settings.history.searchLabel":
    "accessible name of the history search box; the box is not a setting",
  "settings.models.rescan.tooltip":
    "tooltip and accessible name of the icon button that rescans the models folder; an action, not a setting",
  "settings.models.filters.streaming":
    "tooltip of an icon-only filter toggle in the models toolbar; a list filter, not a setting",
  "settings.models.filters.translation":
    "tooltip of an icon-only filter toggle in the models toolbar; a list filter, not a setting",
  "settings.advanced.customWords.remove":
    "accessible name of the per-word remove button in the custom words list; an action, not a setting",
};

/**
 * Dynamic t() calls in files on these pages. Each family lists the keys it can
 * produce; `ids: []` requires a note explaining why nothing there is a row.
 */
export const CORE_DYNAMIC: readonly DynamicFamily[] = [
  {
    source: "src/components/settings/GlobalShortcutInput.tsx",
    prefix: `${SHORTCUT_BINDING_PREFIX}.`,
    ids: ["transcribe", "cancel", "transcribe_with_post_process"],
    suffix: ".name",
  },
  {
    source: "src/components/settings/HandyKeysShortcutInput.tsx",
    prefix: `${SHORTCUT_BINDING_PREFIX}.`,
    ids: ["transcribe", "cancel", "transcribe_with_post_process"],
    suffix: ".name",
  },
  {
    source: "src/components/settings/ThemeSelector.tsx",
    prefix: "theme.options.",
    ids: [],
    note: "t(`theme.options.${value}`) only labels the entries of the theme Dropdown (System, Light, Dark, Aurora, Custom); the row itself is the literal theme.title.",
  },
  {
    source: "src/components/settings/CustomThemeEditor.tsx",
    prefix: "theme.custom.",
    ids: [
      "text",
      "background",
      "accent",
      "accentSecondary",
      "warning",
      "error",
    ],
  },
  {
    source: "src/components/settings/debug/PasteDelay.tsx",
    prefix: "settings.debug.pasteDelay",
    ids: ["", "After"],
    suffix: ".title",
  },
];
