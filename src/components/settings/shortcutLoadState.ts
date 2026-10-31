// Shortcut load failure (T5, U9). Pure: the shortcut rows pass the store flags in and render the
// result. Only one row (the first shortcut) is a live region, so the failure is announced once
// even though every row can show the message and its Retry button.

export const SHORTCUT_IDS: readonly string[] = [
  "transcribe",
  "cancel",
  "transcribe_with_post_process",
  "converse",
  "chat_window",
];

const ANNOUNCER_ID = "transcribe";

export function shortcutAnnouncesLoadFailure(shortcutId: string): boolean {
  return shortcutId === ANNOUNCER_ID;
}

export interface ShortcutLoadInput {
  loadFailed: boolean;
  retrying: boolean;
  shortcutId: string;
}

export interface ShortcutLoadView {
  showError: boolean;
  /** "alert" only on the announcing row; undefined keeps every other row silent. */
  role: "alert" | undefined;
  retryDisabled: boolean;
  busy: boolean;
  retryLabelKey: string;
}

export function shortcutLoadView(input: ShortcutLoadInput): ShortcutLoadView {
  return {
    showError: input.loadFailed,
    role:
      input.loadFailed && shortcutAnnouncesLoadFailure(input.shortcutId)
        ? "alert"
        : undefined,
    retryDisabled: input.retrying,
    busy: input.retrying,
    retryLabelKey: input.retrying
      ? "settings.general.shortcut.retrying"
      : "settings.general.shortcut.retry",
  };
}
