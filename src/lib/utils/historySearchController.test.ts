import assert from "node:assert/strict";
import {
  createHistorySearchController,
  type HistoryPage,
  type HistorySearchState,
} from "./historySearchController";

// Authoritative spec for the History search controller. Plain async script in
// the same style as the other *.test.ts files here; a failed assertion throws
// and fails `bun test` / `bun <file>`.

interface Entry {
  id: number;
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

function entries(...ids: number[]): Entry[] {
  return ids.map((id) => ({ id }));
}

function harness() {
  const searchCalls: { query: string; d: Deferred<Entry[]> }[] = [];
  const pageCalls: { cursor: number | null; d: Deferred<HistoryPage<Entry>> }[] =
    [];
  const states: HistorySearchState<Entry>[] = [];
  const timers = new Map<number, { cb: () => void; ms: number }>();
  let nextTimer = 1;

  const controller = createHistorySearchController<Entry>({
    search: (query) => {
      const d = deferred<Entry[]>();
      searchCalls.push({ query, d });
      return d.promise;
    },
    fetchPage: (cursor) => {
      const d = deferred<HistoryPage<Entry>>();
      pageCalls.push({ cursor, d });
      return d.promise;
    },
    onState: (state) => states.push(state),
    setTimer: (cb, ms) => {
      const id = nextTimer++;
      timers.set(id, { cb, ms });
      return id;
    },
    clearTimer: (handle) => {
      timers.delete(handle as number);
    },
  });

  return {
    controller,
    searchCalls,
    pageCalls,
    states,
    timers,
    last: () => states[states.length - 1],
    /** Fire every pending timer (like time passing) and return how many fired. */
    fireTimers: () => {
      const pending = [...timers.entries()];
      timers.clear();
      for (const [, timer] of pending) timer.cb();
      return pending.length;
    },
  };
}

type Harness = ReturnType<typeof harness>;

/** Load page 1 with the given ids. */
async function loaded(h: Harness, ids: number[], hasMore: boolean) {
  const p = h.controller.refresh();
  h.pageCalls[h.pageCalls.length - 1].d.resolve({
    entries: entries(...ids),
    hasMore,
  });
  await p;
}

const scenarios: Record<string, () => Promise<void>> = {
  async "refresh loads page 1 into list mode"() {
    const h = harness();
    const p = h.controller.refresh();
    assert.equal(h.pageCalls.length, 1);
    assert.equal(h.pageCalls[0].cursor, null);
    assert.equal(h.last().loading, true);
    h.pageCalls[0].d.resolve({ entries: entries(3, 2), hasMore: true });
    await p;
    const state = h.controller.getState();
    assert.equal(state.mode, "list");
    assert.deepEqual(state.entries, entries(3, 2));
    assert.equal(state.hasMore, true);
    assert.equal(state.loading, false);
    assert.equal(state.error, null);
  },

  async "typing coalesces into one debounced search with the final text"() {
    const h = harness();
    h.controller.setQuery("a");
    assert.equal(h.timers.size, 1);
    assert.equal([...h.timers.values()][0].ms, 250, "default debounce is 250ms");
    h.controller.setQuery("ab");
    assert.equal(h.timers.size, 1, "previous timer was cleared");
    assert.equal(h.searchCalls.length, 0, "nothing searched before the debounce fires");
    assert.equal(h.last().mode, "search");
    assert.equal(h.last().loading, true, "loading shows immediately");
    h.fireTimers();
    assert.equal(h.searchCalls.length, 1);
    assert.equal(h.searchCalls[0].query, "ab");
    h.searchCalls[0].d.resolve(entries(7));
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(7));
    assert.equal(h.controller.getState().loading, false);
  },

  async "the query is trimmed before searching"() {
    const h = harness();
    h.controller.setQuery("  hi there  ");
    h.fireTimers();
    assert.equal(h.searchCalls[0].query, "hi there");
  },

  async "a stale search SUCCESS is discarded after the input changed"() {
    const h = harness();
    h.controller.setQuery("a");
    h.fireTimers(); // search "a" in flight
    h.controller.setQuery("ab"); // invalidates it immediately
    h.searchCalls[0].d.resolve(entries(1));
    await flush();
    assert.deepEqual(h.controller.getState().entries, [], "stale results never show");
    assert.equal(h.controller.getState().loading, true, "still waiting for 'ab'");
    h.fireTimers();
    h.searchCalls[1].d.resolve(entries(2));
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(2));
    assert.equal(h.controller.getState().query, "ab");
  },

  async "a stale search ERROR is discarded after the input changed"() {
    const h = harness();
    h.controller.setQuery("a");
    h.fireTimers();
    h.controller.setQuery("ab");
    h.searchCalls[0].d.reject(new Error("old failure"));
    await flush();
    assert.equal(h.controller.getState().error, null, "stale error never shows");
    h.fireTimers();
    h.searchCalls[1].d.resolve(entries(2));
    await flush();
    assert.equal(h.controller.getState().error, null);
  },

  async "out-of-order search responses: the older one finishing last is ignored"() {
    const h = harness();
    h.controller.setQuery("a");
    h.fireTimers();
    h.controller.setQuery("ab");
    h.fireTimers();
    h.searchCalls[1].d.resolve(entries(2)); // newer finishes first
    await flush();
    h.searchCalls[0].d.resolve(entries(1)); // older finishes last
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(2));
  },

  async "clearing during the debounce cancels the search and restores the list"() {
    const h = harness();
    h.controller.setQuery("abc");
    assert.equal(h.timers.size, 1);
    h.controller.setQuery("");
    assert.equal(h.timers.size, 0, "debounce timer cleared");
    assert.equal(h.searchCalls.length, 0, "search never ran");
    assert.equal(h.pageCalls.length, 1, "list restored via page 1");
    assert.equal(h.pageCalls[0].cursor, null);
    assert.equal(h.last().mode, "list");
    h.pageCalls[0].d.resolve({ entries: entries(9), hasMore: false });
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(9));
  },

  async "a whitespace-only query behaves like an empty one"() {
    const h = harness();
    h.controller.setQuery("   ");
    assert.equal(h.timers.size, 0);
    assert.equal(h.searchCalls.length, 0);
    assert.equal(h.pageCalls.length, 1);
    assert.equal(h.last().mode, "list");
  },

  async "clearing while a search is in flight discards its results"() {
    const h = harness();
    h.controller.setQuery("abc");
    h.fireTimers();
    h.controller.setQuery("");
    h.searchCalls[0].d.resolve(entries(1, 2, 3));
    await flush();
    assert.deepEqual(h.controller.getState().entries, [], "search results not shown in list mode");
    assert.equal(h.controller.getState().mode, "list");
  },

  async "a pagination response pending when a search starts is dropped"() {
    const h = harness();
    await loaded(h, [3, 2], true);
    const more = h.controller.loadMore();
    assert.equal(h.pageCalls[1].cursor, 2, "cursor is the last loaded id");
    h.controller.setQuery("x"); // search starts while the page is pending
    h.pageCalls[1].d.resolve({ entries: entries(1), hasMore: false });
    await more;
    const state = h.controller.getState();
    assert.equal(state.mode, "search");
    assert.ok(!state.entries.some((e) => e.id === 1), "late page never lands in search mode");
  },

  async "a pagination ERROR pending when a search starts is dropped"() {
    const h = harness();
    await loaded(h, [3, 2], true);
    const more = h.controller.loadMore();
    h.controller.setQuery("x");
    h.pageCalls[1].d.reject(new Error("page failed"));
    await more;
    assert.equal(h.controller.getState().error, null);
  },

  async "loadMore appends the next page and updates hasMore"() {
    const h = harness();
    await loaded(h, [3, 2], true);
    const more = h.controller.loadMore();
    assert.equal(h.last().loading, true);
    h.pageCalls[1].d.resolve({ entries: entries(1), hasMore: false });
    await more;
    assert.deepEqual(h.controller.getState().entries, entries(3, 2, 1));
    assert.equal(h.controller.getState().hasMore, false);
  },

  async "loadMore is ignored in search mode, without hasMore, and while loading"() {
    const noMore = harness();
    await loaded(noMore, [2, 1], false);
    await noMore.controller.loadMore();
    assert.equal(noMore.pageCalls.length, 1, "no hasMore => no fetch");

    const searching = harness();
    await loaded(searching, [2, 1], true);
    searching.controller.setQuery("x");
    await searching.controller.loadMore();
    assert.equal(searching.pageCalls.length, 1, "search mode => no fetch");

    const busy = harness();
    await loaded(busy, [2, 1], true);
    void busy.controller.loadMore();
    await busy.controller.loadMore();
    assert.equal(busy.pageCalls.length, 2, "second call while loading is ignored");
  },

  async "a current loadMore error keeps the entries and shows the error"() {
    const h = harness();
    await loaded(h, [3, 2], true);
    const more = h.controller.loadMore();
    h.pageCalls[1].d.reject(new Error("page failed"));
    await more;
    const state = h.controller.getState();
    assert.deepEqual(state.entries, entries(3, 2));
    assert.equal(state.error, "page failed");
    assert.equal(state.loading, false);
  },

  async "a history update during search re-runs the search, not the list"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers();
    h.searchCalls[0].d.resolve(entries(5));
    await flush();
    const pagesBefore = h.pageCalls.length;
    h.controller.onHistoryUpdated();
    assert.equal(h.searchCalls.length, 2, "search re-ran immediately, no debounce");
    assert.equal(h.searchCalls[1].query, "x");
    assert.equal(h.pageCalls.length, pagesBefore, "the list was not touched");
    h.searchCalls[1].d.resolve(entries(6, 5));
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(6, 5));
    assert.equal(h.controller.getState().mode, "search");
  },

  async "a history update in list mode refreshes page 1"() {
    const h = harness();
    await loaded(h, [2, 1], false);
    h.controller.onHistoryUpdated();
    assert.equal(h.pageCalls.length, 2);
    assert.equal(h.pageCalls[1].cursor, null);
    assert.equal(h.searchCalls.length, 0);
  },

  async "a history update invalidates the search it replaces"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers();
    h.controller.onHistoryUpdated(); // second search supersedes the first
    h.searchCalls[1].d.resolve(entries(2));
    await flush();
    h.searchCalls[0].d.resolve(entries(1));
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(2));
  },

  async "a current search failure shows an error distinct from an empty result"() {
    const failing = harness();
    failing.controller.setQuery("x");
    failing.fireTimers();
    failing.searchCalls[0].d.reject(new Error("boom"));
    await flush();
    let state = failing.controller.getState();
    assert.equal(state.error, "boom");
    assert.deepEqual(state.entries, []);
    assert.equal(state.loading, false);
    assert.equal(state.mode, "search");

    const empty = harness();
    empty.controller.setQuery("x");
    empty.fireTimers();
    empty.searchCalls[0].d.resolve([]);
    await flush();
    state = empty.controller.getState();
    assert.equal(state.error, null, "no matches is not an error");
    assert.deepEqual(state.entries, []);
    assert.equal(state.loading, false);
  },

  async "non-Error rejections become readable messages"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers();
    h.searchCalls[0].d.reject("plain string failure");
    await flush();
    assert.equal(h.controller.getState().error, "plain string failure");
  },

  async "a new input after an error clears the error"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers();
    h.searchCalls[0].d.reject(new Error("boom"));
    await flush();
    h.controller.setQuery("xy");
    assert.equal(h.controller.getState().error, null);
  },

  async "dispose clears timers and silences every later response"() {
    const h = harness();
    h.controller.setQuery("x");
    assert.equal(h.timers.size, 1);
    h.controller.dispose();
    assert.equal(h.timers.size, 0, "debounce timer cleared on dispose");

    const inFlight = harness();
    inFlight.controller.setQuery("x");
    inFlight.fireTimers();
    inFlight.controller.dispose();
    const before = inFlight.states.length;
    inFlight.searchCalls[0].d.resolve(entries(1));
    await flush();
    assert.equal(inFlight.states.length, before, "no state emitted after dispose");

    const pageInFlight = harness();
    await loaded(pageInFlight, [2, 1], true);
    const more = pageInFlight.controller.loadMore();
    pageInFlight.controller.dispose();
    const beforePage = pageInFlight.states.length;
    pageInFlight.pageCalls[1].d.reject(new Error("late"));
    await more;
    assert.equal(pageInFlight.states.length, beforePage);
  },

  async "dispose also neutralizes an already-captured timer callback"() {
    const h = harness();
    h.controller.setQuery("x");
    const captured = [...h.timers.values()][0].cb;
    h.controller.dispose();
    captured(); // a timer that raced the clear
    assert.equal(h.searchCalls.length, 0, "no search after dispose");
  },

  async "patchEntries applies a pure mapper to the current list entries and emits"() {
    const h = harness();
    await loaded(h, [3, 2, 1], true);
    const emitted = h.states.length;
    h.controller.patchEntries((prev) => prev.filter((e) => e.id !== 2));
    const state = h.controller.getState();
    assert.deepEqual(state.entries, entries(3, 1));
    assert.equal(state.hasMore, true, "other state untouched");
    assert.equal(state.loading, false);
    assert.equal(state.mode, "list");
    assert.equal(h.states.length, emitted + 1, "exactly one state emitted");
    assert.deepEqual(h.last().entries, entries(3, 1));
  },

  async "patchEntries also patches search results"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers();
    h.searchCalls[0].d.resolve(entries(5, 4));
    await flush();
    h.controller.patchEntries((prev) => [{ id: 9 }, ...prev]);
    const state = h.controller.getState();
    assert.deepEqual(state.entries, entries(9, 5, 4));
    assert.equal(state.mode, "search");
    assert.equal(state.query, "x");
  },

  async "loadMore uses the last PATCHED entry as its cursor"() {
    const h = harness();
    await loaded(h, [3, 2], true);
    h.controller.patchEntries((prev) => prev.filter((e) => e.id !== 2));
    const more = h.controller.loadMore();
    assert.equal(h.pageCalls[1].cursor, 3);
    h.pageCalls[1].d.resolve({ entries: entries(1), hasMore: false });
    await more;
    assert.deepEqual(h.controller.getState().entries, entries(3, 1));
  },

  async "patchEntries does not invalidate an in-flight search"() {
    const h = harness();
    h.controller.setQuery("x");
    h.fireTimers(); // search in flight
    h.controller.patchEntries((prev) => prev);
    h.searchCalls[0].d.resolve(entries(7));
    await flush();
    assert.deepEqual(h.controller.getState().entries, entries(7), "result still applied");
  },

  async "patchEntries after dispose is silent"() {
    const h = harness();
    await loaded(h, [2, 1], false);
    h.controller.dispose();
    const before = h.states.length;
    h.controller.patchEntries((prev) => prev.slice(1));
    assert.equal(h.states.length, before);
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    await run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(`${failed} of ${Object.keys(scenarios).length} controller scenarios failed`);
}
console.log(`history search controller: ${Object.keys(scenarios).length} scenarios passed`);
