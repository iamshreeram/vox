// Pure action matrix for the unified model row (DESIGN.md Rev 5, item 4).
// Spec = modelRow.test.tsx (authoritative). No React, no i18n, no Tauri imports.

/** Lifecycle status of one model-like thing (STT model, TTS engine, later user models). */
export type RowStatus =
  | "notInstalled"
  | "downloading"
  | "verifying"
  | "extracting"
  | "switching"
  | "ready"
  | "active"
  | "error";

/** Which actions a row offers in a given status. */
export interface RowActions {
  select: boolean;
  download: boolean;
  cancel: boolean;
  delete: boolean;
  retry: boolean;
}

/**
 * The action matrix:
 * - notInstalled: download
 * - downloading: cancel
 * - verifying / extracting / switching: none (busy)
 * - ready: select, delete
 * - active: delete (already in use, so not selectable)
 * - error: retry, delete
 * Delete is removed entirely when the row is not deletable.
 */
export function rowActions(
  status: RowStatus,
  options: { deletable: boolean },
): RowActions {
  const actions: RowActions = {
    select: false,
    download: false,
    cancel: false,
    delete: false,
    retry: false,
  };
  switch (status) {
    case "notInstalled":
      actions.download = true;
      break;
    case "downloading":
      actions.cancel = true;
      break;
    case "verifying":
    case "extracting":
    case "switching":
      break;
    case "ready":
      actions.select = true;
      actions.delete = true;
      break;
    case "active":
      actions.delete = true;
      break;
    case "error":
      actions.retry = true;
      actions.delete = true;
      break;
  }
  if (!options.deletable) {
    actions.delete = false;
  }
  return actions;
}
