// Pure helper that builds the spoken-reply voice dropdown.

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
 * @param voices installed voices in display order
 * @param current saved voice name, or null when unset
 * @param labels labels for the default and not-installed entries
 */
export function buildVoiceOptions(
  voices: TtsVoiceInfo[],
  current: string | null,
  labels: VoiceLabels,
): VoiceOptionsResult {
  const useTextFallback = voices.length === 0;
  const selected = (current ?? "").trim();

  const defaultOption: VoiceOption = { value: "", label: labels.defaultLabel };
  if (useTextFallback) {
    return {
      options: [defaultOption],
      selected,
      useTextFallback,
      descriptionKey: "settings.advanced.tts.voiceDescription",
    };
  }

  const options: VoiceOption[] = [defaultOption];
  const seen = new Set<string>();
  for (const voice of voices) {
    if (seen.has(voice.name)) continue;
    seen.add(voice.name);
    options.push({
      value: voice.name,
      label: `${voice.name} (${voice.locale})`,
    });
  }

  if (selected !== "" && !seen.has(selected)) {
    options.push({ value: selected, label: labels.notInstalled(selected) });
  }

  return {
    options,
    selected,
    useTextFallback,
    descriptionKey: "settings.advanced.tts.voiceListDescription",
  };
}

/**
 * Maps a dropdown value to the setting value: blank means system default (null).
 *
 * @param value dropdown value
 */
export function voiceValueToSetting(value: string): string | null {
  const trimmed = value.trim();
  return trimmed === "" ? null : trimmed;
}
