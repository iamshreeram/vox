// How the user can interrupt a spoken reply by talking over it. Off by default.
// Keep in sync with the Rust `ConverseBargeIn` enum (settings.rs) and the
// generated `ConverseBargeIn` type in bindings.ts.
export const CONVERSE_BARGE_IN_VALUES = ["off", "headphones"] as const;

export type ConverseBargeIn = (typeof CONVERSE_BARGE_IN_VALUES)[number];

export const DEFAULT_CONVERSE_BARGE_IN: ConverseBargeIn = "off";

export function normalizeConverseBargeIn(value: unknown): ConverseBargeIn {
  return (CONVERSE_BARGE_IN_VALUES as readonly unknown[]).includes(value)
    ? (value as ConverseBargeIn)
    : DEFAULT_CONVERSE_BARGE_IN;
}
