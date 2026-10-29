// What a wake-word detection does: it always starts a conversation (one trigger).
// Keep in sync with the Rust `WakeWordAction` enum (settings.rs), which reads any stored
// value as "conversation" and always writes "conversation". The generated bindings.ts type
// still lists the retired variant, which nothing in the UI reads or writes.
export const WAKE_WORD_ACTION = "conversation" as const;

export type WakeWordAction = typeof WAKE_WORD_ACTION;
