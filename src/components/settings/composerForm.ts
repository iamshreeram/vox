import type {
  TtsComposerFallback,
  TtsComposerMode,
  TtsComposerSettings,
  TtsComposerSettingsPatch,
} from "../../bindings";

/** What the user is typing into the command form (saved with ONE patch). */
export interface CommandDraft {
  path: string;
  args: string;
  flag: string;
  acknowledge: boolean;
}

export type CommandReadiness =
  | "needs_setup"
  | "needs_acknowledgement"
  | "ready";

/** Flat AppSettings fields the composer UI reads (all optional in the bindings). */
export interface ComposerAppFields {
  tts_composer_mode?: TtsComposerMode;
  tts_composer_provider_id?: string;
  tts_composer_model?: string;
  tts_composer_command_path?: string;
  tts_composer_command_args?: string;
  tts_composer_prompt_flag?: string;
  tts_composer_command_ack?: string;
  tts_composer_timeout_secs?: number;
  tts_composer_style?: string;
  tts_composer_send_user_utterance?: boolean;
  tts_composer_fallback?: TtsComposerFallback;
}

export const DEFAULT_PROMPT_FLAG = "-p";
export const KNOWN_OUTCOMES = [
  "ok",
  "timeout",
  "not_configured",
  "needs_acknowledgement",
  "backend_error",
  "output_too_large",
  "empty",
  "unparseable",
  "superseded",
] as const;

/** Every key present, all null (the generated patch type has no optional keys). */
export function emptyPatch(): TtsComposerSettingsPatch {
  throw new Error("not implemented");
}

/** `emptyPatch()` overlaid with `partial`; never mutates its inputs. */
export function patchWith(
  partial: Partial<TtsComposerSettingsPatch>,
): TtsComposerSettingsPatch {
  throw new Error("not implemented");
}

/** `path \0 args \0 flag` exactly as the backend normalizes it: trim the path
 * and the flag, collapse the args to single spaces. */
export function ackTuple(path: string, args: string, flag: string): string {
  throw new Error("not implemented");
}

/** True iff the stored acknowledgement is non-empty and equals the tuple of
 * the stored path/args/flag. */
export function isAcknowledged(s: TtsComposerSettings): boolean {
  throw new Error("not implemented");
}

export function draftFromSettings(s: TtsComposerSettings): CommandDraft {
  throw new Error("not implemented");
}

/** ONE patch carrying path, args, flag (trimmed) and acknowledge_command; all
 * other keys null. */
export function commandPatch(d: CommandDraft): TtsComposerSettingsPatch {
  throw new Error("not implemented");
}

/** Compared with whitespace normalization; a blank flag equals "-p". */
export function isCommandDraftDirty(
  d: CommandDraft,
  s: TtsComposerSettings,
): boolean {
  throw new Error("not implemented");
}

export function commandReadiness(s: TtsComposerSettings): CommandReadiness {
  throw new Error("not implemented");
}

/** i18n key for a `speech-composer-status` outcome. */
export function statusLabelKey(outcome: string): string {
  throw new Error("not implemented");
}

/** Build the composer settings from the flat app fields, with the backend
 * defaults for anything missing. */
export function composerFromApp(app: ComposerAppFields): TtsComposerSettings {
  throw new Error("not implemented");
}
