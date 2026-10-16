// Unit tests for the in-memory "open group" request channel of collapsibleState.
// Written before the implementation (failing-first): the exports below do not
// exist yet, so this file is expected to fail until they are added.
import assert from "node:assert/strict";
import {
  GROUP_OPEN_EVENT,
  PENDING_OPEN_TTL_MS,
  consumePendingOpen,
  openGroup,
  parseGroupOpenEvent,
} from "./collapsibleState";

type SetItemStore = Pick<Storage, "setItem">;

const recordingStore = (): SetItemStore & { data: Record<string, string> } => {
  const data: Record<string, string> = {};
  return {
    data,
    setItem: (key: string, value: string) => {
      data[key] = value;
    },
  };
};

const throwingStore: SetItemStore = {
  setItem: () => {
    throw new Error("quota exceeded");
  },
};

const noop = (): void => {};

const scenarios: Record<string, () => void> = {
  "pending request is consumed once: true first, then false": () => {
    openGroup("once-group", { now: () => 1000, storage: null, dispatch: noop });
    assert.equal(
      consumePendingOpen("once-group", () => 1000),
      true,
    );
    assert.equal(
      consumePendingOpen("once-group", () => 1000),
      false,
    );
  },

  "TTL boundary: consume at +2999ms is true": () => {
    openGroup("ttl-inside", {
      now: () => 1000,
      storage: null,
      dispatch: noop,
    });
    assert.equal(
      consumePendingOpen("ttl-inside", () => 1000 + 2999),
      true,
    );
  },

  "TTL boundary: consume at +3001ms is false": () => {
    openGroup("ttl-outside", {
      now: () => 1000,
      storage: null,
      dispatch: noop,
    });
    assert.equal(
      consumePendingOpen("ttl-outside", () => 1000 + 3001),
      false,
    );
  },

  "TTL constant is 3000 ms": () => {
    assert.equal(PENDING_OPEN_TTL_MS, 3000);
  },

  "consuming one id never consumes another id": () => {
    openGroup("ids-a", { now: () => 0, storage: null, dispatch: noop });
    assert.equal(
      consumePendingOpen("ids-b", () => 0),
      false,
    );
    assert.equal(
      consumePendingOpen("ids-a", () => 0),
      true,
    );
  },

  "two openGroup calls yield exactly one consume": () => {
    const deps = { now: () => 0, storage: null, dispatch: noop };
    openGroup("twice-group", deps);
    openGroup("twice-group", deps);
    assert.equal(
      consumePendingOpen("twice-group", () => 0),
      true,
    );
    assert.equal(
      consumePendingOpen("twice-group", () => 0),
      false,
    );
  },

  "storage.setItem throwing: openGroup does not throw and pending is still consumable":
    () => {
      assert.doesNotThrow(() =>
        openGroup("broken-store", {
          now: () => 500,
          storage: throwingStore,
          dispatch: noop,
        }),
      );
      assert.equal(
        consumePendingOpen("broken-store", () => 500),
        true,
      );
    },

  "storage null: openGroup does not throw": () => {
    assert.doesNotThrow(() =>
      openGroup("null-store", { now: () => 0, storage: null, dispatch: noop }),
    );
  },

  'storage ok: writes key vox.settings.group.open.<id> = "1"': () => {
    const store = recordingStore();
    openGroup("stored-group", {
      now: () => 0,
      storage: store,
      dispatch: noop,
    });
    assert.equal(store.data["vox.settings.group.open.stored-group"], "1");
  },

  "dispatch is called with the group id": () => {
    const calls: string[] = [];
    openGroup("dispatched-group", {
      now: () => 0,
      storage: null,
      dispatch: (id: string) => {
        calls.push(id);
      },
    });
    assert.deepEqual(calls, ["dispatched-group"]);
  },

  "dispatch throwing: openGroup does not throw": () => {
    assert.doesNotThrow(() =>
      openGroup("throwing-dispatch", {
        now: () => 0,
        storage: null,
        dispatch: () => {
          throw new Error("listener failed");
        },
      }),
    );
  },

  "default deps without a window object (bun): openGroup does not throw":
    () => {
      assert.equal(
        typeof (globalThis as { window?: unknown }).window,
        "undefined",
        "this scenario assumes the bun runtime has no window global",
      );
      assert.doesNotThrow(() => openGroup("default-deps-group"));
    },

  "parseGroupOpenEvent: valid detail.id is returned": () => {
    assert.equal(parseGroupOpenEvent({ detail: { id: "x" } }), "x");
  },

  "parseGroupOpenEvent: undefined and null yield null": () => {
    assert.equal(parseGroupOpenEvent(undefined), null);
    assert.equal(parseGroupOpenEvent(null), null);
  },

  "parseGroupOpenEvent: primitives yield null": () => {
    assert.equal(parseGroupOpenEvent("x"), null);
    assert.equal(parseGroupOpenEvent(42), null);
    assert.equal(parseGroupOpenEvent(true), null);
  },

  "parseGroupOpenEvent: missing detail yields null": () => {
    assert.equal(parseGroupOpenEvent({}), null);
    assert.equal(parseGroupOpenEvent({ type: GROUP_OPEN_EVENT }), null);
  },

  "parseGroupOpenEvent: empty id yields null": () => {
    assert.equal(parseGroupOpenEvent({ detail: { id: "" } }), null);
  },

  "parseGroupOpenEvent: non-string id yields null": () => {
    assert.equal(parseGroupOpenEvent({ detail: { id: 5 } }), null);
    assert.equal(parseGroupOpenEvent({ detail: { id: null } }), null);
    assert.equal(
      parseGroupOpenEvent({ detail: { id: { nested: "x" } } }),
      null,
    );
  },

  "event name is vox:settings-group-open": () => {
    assert.equal(GROUP_OPEN_EVENT, "vox:settings-group-open");
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(`${failed} collapsible open-request scenarios failed`);
}
console.log(
  `collapsible open requests: ${Object.keys(scenarios).length} scenarios passed`,
);
