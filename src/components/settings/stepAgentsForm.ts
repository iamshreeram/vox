/**
 * Pure input rules for the specialist agent rows (Stage 4). No React, no Tauri imports.
 * A blank command clears the slot, so that step type runs on the main ACP agent. The
 * command and argument rules are the main agent's rules (acpAgentForm.ts), which mirror
 * the Rust validators in settings.rs; the backend stays authoritative.
 */
import type { StepAgentProfile } from "@/bindings";
import {
  type AcpFormResult,
  parseAcpArgs,
  parseAcpCommand,
} from "./acpAgentForm";

/** The step types that own a specialist slot. `general` has no slot and always uses the main agent. */
export const STEP_AGENT_TYPES = [
  "research",
  "browser",
  "code",
  "files",
] as const;

export type StepAgentType = (typeof STEP_AGENT_TYPES)[number];

export function isStepAgentType(value: unknown): value is StepAgentType {
  return (
    typeof value === "string" &&
    (STEP_AGENT_TYPES as readonly string[]).includes(value)
  );
}

/** The text in a row's two inputs. */
export interface StepAgentDraft {
  command: string;
  args: string;
}

/** Stored profile to row text. An empty slot shows as two blank inputs. */
export function formatStepAgentProfile(
  profile: StepAgentProfile | null | undefined,
): StepAgentDraft {
  return { command: profile?.command ?? "", args: profile?.args ?? "" };
}

/**
 * Row text to a value to store. `value: null` clears the slot. The arguments are checked
 * even when the command is blank, as the backend does, and are stored trimmed.
 */
export function parseStepAgentDraft(
  draft: StepAgentDraft,
): AcpFormResult<StepAgentProfile | null> {
  const command = parseAcpCommand(draft.command);
  if (!command.ok) return command;
  const args = parseAcpArgs(draft.args);
  if (!args.ok) return args;
  if (command.value === null) return { ok: true, value: null };
  return {
    ok: true,
    value: { command: command.value, args: args.value.trim() },
  };
}

/** True when two stored slots hold the same profile (both empty counts as the same). */
export function sameStepAgentProfile(
  a: StepAgentProfile | null | undefined,
  b: StepAgentProfile | null | undefined,
): boolean {
  const left = a ?? null;
  const right = b ?? null;
  if (left === null || right === null) return left === right;
  return left.command === right.command && left.args === right.args;
}
