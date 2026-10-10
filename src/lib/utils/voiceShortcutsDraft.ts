import type { VoiceShortcut, VoiceShortcutAction } from "@/bindings";

export type VoiceShortcutActionType = VoiceShortcutAction["type"];

/** Maximum number of shortcuts; mirrors the backend `MAX_SHORTCUTS`. */
export const MAX_VOICE_SHORTCUTS = 50;

/** A new, blank shortcut that opens a URL. */
export const emptyShortcut = (): VoiceShortcut => ({
  phrase: "",
  action: { type: "open_url", url: "" },
});

/** Returns a new list with a blank shortcut appended. */
export const addRow = (list: VoiceShortcut[]): VoiceShortcut[] => [
  ...list,
  emptyShortcut(),
];

/**
 * Returns a new list with the row at `index` shallow-merged with `patch`.
 * An out-of-range index returns an unchanged copy.
 */
export const updateRow = (
  list: VoiceShortcut[],
  index: number,
  patch: Partial<VoiceShortcut>,
): VoiceShortcut[] => {
  if (!isValidIndex(list, index)) return [...list];
  return list.map((row, i) => (i === index ? { ...row, ...patch } : row));
};

/**
 * Returns a new list without the row at `index`. An out-of-range index
 * returns an unchanged copy.
 */
export const removeRow = (
  list: VoiceShortcut[],
  index: number,
): VoiceShortcut[] => {
  if (!isValidIndex(list, index)) return [...list];
  return list.filter((_, i) => i !== index);
};

/** True when the draft differs from the saved list (deep, order-sensitive). */
export const isDirty = (
  saved: VoiceShortcut[],
  draft: VoiceShortcut[],
): boolean => JSON.stringify(saved) !== JSON.stringify(draft);

/**
 * Returns a shortcut whose action is switched to `type`. The target is reset
 * to an empty string because a URL, folder, and app name are not
 * interchangeable.
 */
export const changeActionType = (
  shortcut: VoiceShortcut,
  type: VoiceShortcutActionType,
): VoiceShortcut => {
  switch (type) {
    case "open_url":
      return { ...shortcut, action: { type: "open_url", url: "" } };
    case "open_path":
      return { ...shortcut, action: { type: "open_path", path: "" } };
    case "open_app":
      return { ...shortcut, action: { type: "open_app", name: "" } };
  }
};

/** Returns the target string of a shortcut, whichever action it uses. */
export const actionTarget = (shortcut: VoiceShortcut): string => {
  const action = shortcut.action;
  switch (action.type) {
    case "open_url":
      return action.url;
    case "open_path":
      return action.path;
    case "open_app":
      return action.name;
  }
};

/** Returns a shortcut with its target replaced, keeping the action type. */
export const setActionTarget = (
  shortcut: VoiceShortcut,
  target: string,
): VoiceShortcut => {
  const action = shortcut.action;
  switch (action.type) {
    case "open_url":
      return { ...shortcut, action: { type: "open_url", url: target } };
    case "open_path":
      return { ...shortcut, action: { type: "open_path", path: target } };
    case "open_app":
      return { ...shortcut, action: { type: "open_app", name: target } };
  }
};

const isValidIndex = (list: unknown[], index: number): boolean =>
  Number.isInteger(index) && index >= 0 && index < list.length;
