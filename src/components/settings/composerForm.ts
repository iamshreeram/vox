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
  return {
    mode: null,
    provider_id: null,
    model: null,
    command_path: null,
    command_args: null,
    prompt_flag: null,
    timeout_secs: null,
    style: null,
    send_user_utterance: null,
    fallback: null,
    acknowledge_command: null,
  };
}

/** Collapse runs of whitespace to single spaces and drop leading/trailing ones. */
function normalizeArgs(args: string): string {
  return args.split(/\s+/).filter(Boolean).join(" ");
}

/** The flag as the backend stores it: trimmed, blank means the default. */
function effectiveFlag(flag: string): string {
  return flag.trim() || DEFAULT_PROMPT_FLAG;
}

/** `emptyPatch()` overlaid with `partial`; never mutates its inputs. */
export function patchWith(
  partial: Partial<TtsComposerSettingsPatch>,
): TtsComposerSettingsPatch {
  const out = emptyPatch();
  for (const key of Object.keys(
    partial,
  ) as (keyof TtsComposerSettingsPatch)[]) {
    const value = partial[key];
    if (value !== undefined) {
      Object.assign(out, { [key]: value });
    }
  }
  return out;
}

/** `path \0 args \0 flag` exactly as the backend normalizes it: trim the path
 * and the flag, collapse the args to single spaces. */
export function ackTuple(path: string, args: string, flag: string): string {
  return `${path.trim()}\0${normalizeArgs(args)}\0${flag.trim()}`;
}

/** True iff the stored acknowledgement is non-empty and equals the tuple of
 * the stored path/args/flag. */
export function isAcknowledged(s: TtsComposerSettings): boolean {
  return (
    s.command_ack !== "" &&
    s.command_ack === ackTuple(s.command_path, s.command_args, s.prompt_flag)
  );
}

export function draftFromSettings(s: TtsComposerSettings): CommandDraft {
  return {
    path: s.command_path,
    args: s.command_args,
    flag: s.prompt_flag,
    acknowledge: isAcknowledged(s),
  };
}

/** ONE patch carrying path, args, flag (trimmed) and acknowledge_command; all
 * other keys null. */
export function commandPatch(d: CommandDraft): TtsComposerSettingsPatch {
  return {
    ...emptyPatch(),
    command_path: d.path.trim(),
    command_args: normalizeArgs(d.args),
    prompt_flag: d.flag.trim(),
    acknowledge_command: d.acknowledge,
  };
}

/** Compared with whitespace normalization; a blank flag equals "-p". */
export function isCommandDraftDirty(
  d: CommandDraft,
  s: TtsComposerSettings,
): boolean {
  return (
    d.path.trim() !== s.command_path.trim() ||
    normalizeArgs(d.args) !== normalizeArgs(s.command_args) ||
    effectiveFlag(d.flag) !== effectiveFlag(s.prompt_flag) ||
    d.acknowledge !== isAcknowledged(s)
  );
}

export function commandReadiness(s: TtsComposerSettings): CommandReadiness {
  if (s.command_path.trim() === "") {
    return "needs_setup";
  }
  if (!isAcknowledged(s)) {
    return "needs_acknowledgement";
  }
  return "ready";
}

/** i18n key for a `speech-composer-status` outcome. */
export function statusLabelKey(outcome: string): string {
  const known: readonly string[] = KNOWN_OUTCOMES;
  const key = known.includes(outcome) ? outcome : "unknown";
  return `settings.advanced.tts.composer.status.${key}`;
}

/** Build the composer settings from the flat app fields, with the backend
 * defaults for anything missing. */
export function composerFromApp(app: ComposerAppFields): TtsComposerSettings {
  return {
    mode: app.tts_composer_mode ?? "off",
    provider_id: app.tts_composer_provider_id ?? "custom",
    model: app.tts_composer_model ?? "",
    command_path: app.tts_composer_command_path ?? "",
    command_args: app.tts_composer_command_args ?? "",
    prompt_flag: app.tts_composer_prompt_flag ?? DEFAULT_PROMPT_FLAG,
    command_ack: app.tts_composer_command_ack ?? "",
    timeout_secs: app.tts_composer_timeout_secs ?? 45,
    style: app.tts_composer_style ?? "warm, upbeat and concise",
    send_user_utterance: app.tts_composer_send_user_utterance ?? true,
    fallback: app.tts_composer_fallback ?? "short_notice",
  };
}
