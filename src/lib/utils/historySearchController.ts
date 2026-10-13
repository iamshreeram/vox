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
  /**
   * Applies a pure mapper to the entries currently shown (list OR search
   * results) and emits one state. Used for optimistic edits (toggle saved,
   * delete) and incremental live updates. Does NOT invalidate in-flight
   * requests and is a no-op after `dispose`.
   */
  patchEntries: (mapper: (entries: T[]) => T[]) => void;
  dispose: () => void;
  getState: () => HistorySearchState<T>;
}

function errorMessage(value: unknown): string {
  if (value instanceof Error) return value.message;
  if (typeof value === "string") return value;
  return String(value);
}

export function createHistorySearchController<T extends { id: number }>(
  deps: HistorySearchDeps<T>,
): HistorySearchController<T> {
  const debounceMs = deps.debounceMs ?? 250;
  let state: HistorySearchState<T> = {
    mode: "list",
    entries: [],
    loading: false,
    error: null,
    hasMore: false,
    query: "",
  };
  let generation = 0;
  let disposed = false;
  let pendingTimer: { handle: unknown } | null = null;

  const isCurrent = (gen: number): boolean => !disposed && gen === generation;

  const update = (patch: Partial<HistorySearchState<T>>): void => {
    if (disposed) return;
    state = { ...state, ...patch };
    deps.onState(state);
  };

  const clearPendingTimer = (): void => {
    if (pendingTimer !== null) {
      deps.clearTimer(pendingTimer.handle);
      pendingTimer = null;
    }
  };

  const runSearch = async (query: string, gen: number): Promise<void> => {
    try {
      const results = await deps.search(query);
      if (isCurrent(gen)) {
        update({ entries: results, loading: false, error: null });
      }
    } catch (error) {
      if (isCurrent(gen)) {
        update({ error: errorMessage(error), entries: [], loading: false });
      }
    }
  };

  const refresh = async (): Promise<void> => {
    const gen = ++generation;
    clearPendingTimer();
    update({ mode: "list", query: "", loading: true, error: null });
    try {
      const page = await deps.fetchPage(null);
      if (isCurrent(gen)) {
        update({
          entries: page.entries,
          hasMore: page.hasMore,
          loading: false,
          error: null,
        });
      }
    } catch (error) {
      if (isCurrent(gen)) {
        update({ error: errorMessage(error), loading: false });
      }
    }
  };

  const setQuery = (raw: string): void => {
    const gen = ++generation;
    clearPendingTimer();
    const query = raw.trim();
    if (query === "") {
      // Same as refresh(); fire-and-forget so the caller never awaits it.
      void refresh();
      return;
    }
    update({ mode: "search", query, loading: true, error: null });
    const handle = deps.setTimer(() => {
      if (disposed || gen !== generation) return;
      pendingTimer = null;
      void runSearch(query, gen);
    }, debounceMs);
    pendingTimer = { handle };
  };

  const loadMore = async (): Promise<void> => {
    if (disposed || state.mode !== "list" || !state.hasMore || state.loading) {
      return;
    }
    // Captures the current generation without bumping it, so any later
    // setQuery/refresh/dispose makes this page response stale.
    const gen = generation;
    const last = state.entries[state.entries.length - 1];
    const cursor = last === undefined ? null : last.id;
    update({ loading: true });
    try {
      const page = await deps.fetchPage(cursor);
      if (isCurrent(gen)) {
        update({
          entries: [...state.entries, ...page.entries],
          hasMore: page.hasMore,
          loading: false,
        });
      }
    } catch (error) {
      if (isCurrent(gen)) {
        update({ error: errorMessage(error), loading: false });
      }
    }
  };

  const onHistoryUpdated = (): void => {
    if (disposed) return;
    if (state.mode === "search" && state.query !== "") {
      const gen = ++generation;
      clearPendingTimer();
      update({ loading: true });
      void runSearch(state.query, gen);
      return;
    }
    void refresh();
  };

  // Does not bump `generation`: optimistic edits and live patches must not
  // invalidate an in-flight search or page. `update` already no-ops after
  // dispose.
  const patchEntries = (mapper: (entries: T[]) => T[]): void => {
    update({ entries: mapper(state.entries) });
  };

  const dispose = (): void => {
    disposed = true;
    generation += 1;
    clearPendingTimer();
  };

  return {
    refresh,
    setQuery,
    loadMore,
    onHistoryUpdated,
    patchEntries,
    dispose,
    getState: () => state,
  };
}
