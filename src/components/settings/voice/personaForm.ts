/**
 * Pure validation for the custom persona editor. No React, no Tauri.
 * Spec = personaForm.test.ts. The limits mirror src-tauri/src/managers/persona.rs
 * (MAX_CUSTOM, NAME_MAX_CHARS) and composer_rules.rs (MAX_STYLE_CHARS).
 */

export const PERSONA_LIMITS = {
  maxCustom: 12,
  nameMaxChars: 40,
  styleMaxChars: 300,
} as const;

/** Built-in persona ids start with this prefix and are read-only. */
const BUILTIN_PREFIX = "builtin-";

export interface PersonaDraft {
  name: string;
  style: string;
  /** A natural voice id, or null / empty for no voice change. */
  voice: string | null;
}

export interface PersonaFormContext {
  /** Number of custom personas already saved. */
  customCount: number;
  /** Id of the persona being edited, or null for a new one. */
  editingId: string | null;
  /** True when the persona being edited is a built-in. */
  isBuiltin: boolean;
  /** Voice ids the app offers. Empty means the voice list is unavailable. */
  knownVoices: readonly string[];
}

export type PersonaIssue =
  | "name_empty"
  | "name_too_long"
  | "style_empty"
  | "style_too_long"
  | "control_chars"
  | "unknown_voice"
  | "limit_reached"
  | "builtin_immutable";

export type PersonaValidation =
  | { ok: true; persona: { name: string; style: string; voice: string | null } }
  | { ok: false; issues: PersonaIssue[] };

const CONTROL_CHAR = /\p{Cc}/u;

/** Length in code points, the same count the backend uses (chars()). */
function codePoints(text: string): number {
  return [...text].length;
}

export function isBuiltinId(id: string): boolean {
  return id.startsWith(BUILTIN_PREFIX);
}

/**
 * Checks a draft against every rule the backend enforces, collecting all
 * issues. On success the name and style are trimmed, as the backend does.
 */
export function validatePersonaDraft(
  draft: PersonaDraft,
  ctx: PersonaFormContext,
): PersonaValidation {
  const issues: PersonaIssue[] = [];

  if (ctx.isBuiltin) issues.push("builtin_immutable");
  if (ctx.editingId === null && ctx.customCount >= PERSONA_LIMITS.maxCustom) {
    issues.push("limit_reached");
  }

  const name = draft.name.trim();
  if (name === "") {
    issues.push("name_empty");
  } else if (CONTROL_CHAR.test(name)) {
    issues.push("control_chars");
  } else if (codePoints(name) > PERSONA_LIMITS.nameMaxChars) {
    issues.push("name_too_long");
  }

  const style = draft.style.trim();
  if (style === "") {
    issues.push("style_empty");
  } else if (CONTROL_CHAR.test(style)) {
    issues.push("control_chars");
  } else if (codePoints(style) > PERSONA_LIMITS.styleMaxChars) {
    issues.push("style_too_long");
  }

  const voice = draft.voice === null || draft.voice === "" ? null : draft.voice;
  if (voice !== null && !ctx.knownVoices.includes(voice)) {
    issues.push("unknown_voice");
  }

  if (issues.length > 0) return { ok: false, issues };
  return { ok: true, persona: { name, style, voice } };
}

/** Maps a backend `PersonaError` variant to an editor issue. Anything unknown is generic. */
export function personaErrorIssue(error: string): PersonaIssue | "generic" {
  switch (error) {
    case "NameEmpty":
      return "name_empty";
    case "NameTooLong":
      return "name_too_long";
    case "StyleEmpty":
      return "style_empty";
    case "StyleTooLong":
      return "style_too_long";
    case "ControlChars":
      return "control_chars";
    case "UnknownVoice":
      return "unknown_voice";
    case "BuiltinImmutable":
      return "builtin_immutable";
    case "LimitReached":
      return "limit_reached";
    default:
      return "generic";
  }
}
