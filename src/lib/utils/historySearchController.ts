/**
 * Framework-free controller for the History tab's search + pagination.
 *
 * Why it exists: the interesting bugs in a debounced search box are races
 * (stale responses, pagination landing after a search started, history
 * events during a search). Keeping the orchestration out of the React
 * component makes those races testable with deferred promises and a fake
 * timer; the component is just a thin binding that renders `onState`.
 *
 * Contract (see historySearchController.test.ts, which is authoritative):
 * - Every user input (`setQuery`), `refresh`, and `dispose` IMMEDIATELY
 *   invalidates all in-flight search/page responses. A response is applied
 *   only if it is still current -- applies to successes AND errors.
 * - A trimmed-empty query means list mode: the pending debounce is cleared and
 *   page 1 is fetched. A non-empty query means search mode: state flips to
 *   loading at once and the (trimmed) search runs after `debounceMs`.
 * - `loadMore` only acts in list mode, when `hasMore`, and not while loading.
 * - `onHistoryUpdated` in search mode re-runs the current search at once (no
 *   debounce) and never touches the list; in list mode it refreshes page 1.
 * - After `dispose`, `onState` is never called again and timers are cleared.
 */

export interface HistorySearchState<T> {
  mode: "list" | "search";
  entries: T[];
  loading: boolean;
  error: string | null;
  hasMore: boolean;
  query: string;
}

export interface HistoryPage<T> {
  entries: T[];
  hasMore: boolean;
}

export interface HistorySearchDeps<T extends { id: number }> {
  /** Rejects on failure. */
  search: (query: string) => Promise<T[]>;
  /** `cursor` is the id of the last loaded entry, or null for page 1. */
  fetchPage: (cursor: number | null) => Promise<HistoryPage<T>>;
  onState: (state: HistorySearchState<T>) => void;
  setTimer: (callback: () => void, ms: number) => unknown;
  clearTimer: (handle: unknown) => void;
  /** Default 250. */
  debounceMs?: number;
}

export interface HistorySearchController<T extends { id: number }> {
  refresh: () => Promise<void>;
  setQuery: (query: string) => void;
  loadMore: () => Promise<void>;
  onHistoryUpdated: () => void;
  dispose: () => void;
  getState: () => HistorySearchState<T>;
}

export function createHistorySearchController<T extends { id: number }>(
  _deps: HistorySearchDeps<T>,
): HistorySearchController<T> {
  throw new Error("F5: not implemented");
}
