// Models page empty states (T5, U9). Pure: the component passes counts and flags in and renders
// the matching copy. "No models downloaded yet" and "No models match this filter" are decided
// here so they can never be shown for the wrong situation.

export type ModelsEmptyState =
  | "none"
  | "downloading"
  | "noneDownloaded"
  | "noMatch";

export interface ModelsEmptyInput {
  /** Catalog still loading: nothing is decided yet. */
  loading: boolean;
  /** Models on disk or user-added, before any filter (the Downloaded section's population). */
  downloadedCount: number;
  /**
   * Models being downloaded or extracted. The Your models list shows them while they run, so the
   * "No models downloaded yet" text must not appear beside them.
   */
  downloadingCount?: number;
  /** Models left after every filter and the search. */
  filteredCount: number;
  /** A search, toggle or language filter is narrowing the list. */
  filterActive: boolean;
}

export function modelsEmptyState(input: ModelsEmptyInput): ModelsEmptyState {
  if (input.loading) return "none";
  if (input.filterActive && input.filteredCount === 0) return "noMatch";
  if ((input.downloadingCount ?? 0) > 0) return "downloading";
  if (input.downloadedCount === 0) return "noneDownloaded";
  return "none";
}

export interface ModelsFilterFields {
  searchQuery: string;
  filterStreaming: boolean;
  filterTranslation: boolean;
  languageFilter: string;
}

export function modelsFilterActive(fields: ModelsFilterFields): boolean {
  return (
    fields.searchQuery.trim() !== "" ||
    fields.filterStreaming ||
    fields.filterTranslation ||
    fields.languageFilter !== "all"
  );
}

export function isDownloadedForEmptyState(model: {
  is_downloaded: boolean;
  is_custom: boolean;
}): boolean {
  return model.is_downloaded || model.is_custom;
}
