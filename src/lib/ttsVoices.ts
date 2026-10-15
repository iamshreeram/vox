// Pure helper that builds the spoken-reply voice dropdown.
// STUB (S2-A RED): every function throws until the S2-B implementation lands.

/** A voice reported by the backend (name and locale as listed by the system). */
export interface TtsVoiceInfo {
  name: string;
  locale: string;
}

/** One entry of the voice dropdown. */
export interface VoiceOption {
  value: string;
  label: string;
}

/** Strings used to label the default and not-installed entries. */
export interface VoiceLabels {
  defaultLabel: string;
  notInstalled: (name: string) => string;
}

/** Translation key that describes the dropdown or the text fallback. */
export type VoiceDescriptionKey =
  | "settings.advanced.tts.voiceListDescription"
  | "settings.advanced.tts.voiceDescription";

/** Everything the settings component needs to render the voice control. */
export interface VoiceOptionsResult {
  options: VoiceOption[];
  selected: string;
  useTextFallback: boolean;
  descriptionKey: VoiceDescriptionKey;
}

/**
 * Builds the dropdown options for the voice picker.
 *
 * @param _voices installed voices in display order
 * @param _current saved voice name, or null when unset
 * @param _labels labels for the default and not-installed entries
 */
export function buildVoiceOptions(
  _voices: TtsVoiceInfo[],
  _current: string | null,
  _labels: VoiceLabels,
): VoiceOptionsResult {
  throw new Error("todo");
}

/**
 * Maps a dropdown value to the setting value: blank means system default (null).
 *
 * @param _value dropdown value
 */
export function voiceValueToSetting(_value: string): string | null {
  throw new Error("todo");
}
