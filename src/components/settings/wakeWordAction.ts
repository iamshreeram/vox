// What a wake-word detection does while no conversation session is running.
// Keep in sync with the Rust `WakeWordAction` enum (settings.rs) and the
// generated `WakeWordAction` type in bindings.ts.
export const WAKE_WORD_ACTIONS = ["voice_request", "conversation"] as const;

export type WakeWordAction = (typeof WAKE_WORD_ACTIONS)[number];

// Today's behaviour: say the wake word, speak one request.
export const DEFAULT_WAKE_WORD_ACTION: WakeWordAction = "voice_request";

export function normalizeWakeWordAction(value: unknown): WakeWordAction {
  return (WAKE_WORD_ACTIONS as readonly unknown[]).includes(value)
    ? (value as WakeWordAction)
    : DEFAULT_WAKE_WORD_ACTION;
}

export function wakeWordActionLabelKey(action: WakeWordAction): string {
  return action === "conversation"
    ? "settings.advanced.wakeWord.action.conversation"
    : "settings.advanced.wakeWord.action.voiceRequest";
}
