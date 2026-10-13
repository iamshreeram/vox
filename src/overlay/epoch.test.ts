import { describe, expect, it } from "vitest";
import { createEpoch } from "./epoch";

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe("createEpoch", () => {
  it("next() returns strictly increasing numbers", () => {
    const epoch = createEpoch();
    const a = epoch.next();
    const b = epoch.next();
    const c = epoch.next();
    expect(b).toBeGreaterThan(a);
    expect(c).toBeGreaterThan(b);
  });

  it("a token is current right after next()", () => {
    const epoch = createEpoch();
    const token = epoch.next();
    expect(epoch.isCurrent(token)).toBe(true);
  });

  it("a token is NOT current after another next()", () => {
    const epoch = createEpoch();
    const first = epoch.next();
    epoch.next();
    expect(epoch.isCurrent(first)).toBe(false);
  });

  it("only the latest token is current", () => {
    const epoch = createEpoch();
    const tokens = [epoch.next(), epoch.next(), epoch.next(), epoch.next()];
    const latest = tokens[tokens.length - 1];
    tokens.slice(0, -1).forEach((t) => {
      expect(epoch.isCurrent(t)).toBe(false);
    });
    expect(epoch.isCurrent(latest)).toBe(true);
  });

  it("isCurrent of a never-issued or future number is false", () => {
    const epoch = createEpoch();
    const issued = epoch.next();
    expect(epoch.isCurrent(issued + 1)).toBe(false);
    expect(epoch.isCurrent(issued + 1000)).toBe(false);
    expect(epoch.isCurrent(-1)).toBe(false);
  });

  it("isCurrent(0) is false before any next()", () => {
    const epoch = createEpoch();
    expect(epoch.isCurrent(0)).toBe(false);
  });

  it("stale async handler sees isCurrent false and does not mutate shared state", async () => {
    const epoch = createEpoch();
    const shared = { visible: false, label: "idle" };
    const gate = deferred<void>();
    let aSawCurrent: boolean | undefined;

    // Handler A: show. Captures its token, then awaits an external event.
    const showA = async (): Promise<void> => {
      const my = epoch.next();
      await gate.promise;
      aSawCurrent = epoch.isCurrent(my);
      if (!aSawCurrent) return;
      shared.visible = true;
      shared.label = "show-A";
    };

    const pendingA = showA();

    // Hide handler runs while A is still awaiting, and issues the newest token.
    const hideToken = epoch.next();
    shared.visible = false;
    shared.label = "hide";

    gate.resolve();
    await pendingA;

    expect(aSawCurrent).toBe(false);
    expect(epoch.isCurrent(hideToken)).toBe(true);
    expect(shared).toEqual({ visible: false, label: "hide" });
  });

  it("show-then-show: only the second show's continuation applies", async () => {
    const epoch = createEpoch();
    const shared = { label: "idle" };
    const gate1 = deferred<void>();
    const gate2 = deferred<void>();

    const show = async (gate: Deferred<void>, name: string): Promise<void> => {
      const my = epoch.next();
      await gate.promise;
      if (!epoch.isCurrent(my)) return;
      shared.label = name;
    };

    const first = show(gate1, "show-1");
    const second = show(gate2, "show-2");

    // Resolve the second show first, then the first one.
    gate2.resolve();
    await second;
    gate1.resolve();
    await first;

    expect(shared.label).toBe("show-2");
  });

  it("show-hide-show with out-of-order resolution: only the last show applies", async () => {
    const epoch = createEpoch();
    const shared = { label: "idle" };
    const gateShow1 = deferred<void>();
    const gateHide = deferred<void>();
    const gateShow2 = deferred<void>();

    const run = async (gate: Deferred<void>, name: string): Promise<void> => {
      const my = epoch.next();
      await gate.promise;
      if (!epoch.isCurrent(my)) return;
      shared.label = name;
    };

    const show1 = run(gateShow1, "show-1");
    const hide = run(gateHide, "hide");
    const show2 = run(gateShow2, "show-2");

    // Resolve in an order that differs from issue order.
    gateShow2.resolve();
    await show2;
    gateShow1.resolve();
    await show1;
    gateHide.resolve();
    await hide;

    expect(shared.label).toBe("show-2");
  });

  it("two independent createEpoch() instances do not affect each other", () => {
    const a = createEpoch();
    const b = createEpoch();
    const tokenA = a.next();
    const tokenB = b.next();
    expect(a.isCurrent(tokenA)).toBe(true);
    expect(b.isCurrent(tokenB)).toBe(true);

    b.next();
    b.next();
    expect(a.isCurrent(tokenA)).toBe(true);
    expect(b.isCurrent(tokenB)).toBe(false);
  });

  it("a large number of next() calls stays monotonic", () => {
    const epoch = createEpoch();
    let prev = epoch.next();
    for (let i = 0; i < 10_000; i += 1) {
      const current = epoch.next();
      expect(current).toBeGreaterThan(prev);
      prev = current;
    }
    expect(epoch.isCurrent(prev)).toBe(true);
    expect(epoch.isCurrent(prev - 1)).toBe(false);
  });
});
