// Fake-DOM tests for settingsReveal (revealSetting / cancelReveal).
// Written before the implementation (failing-first): ./settingsReveal does not
// exist yet, so this file is expected to fail until it is added.
//
// Harness: a manual rAF queue stepped one frame at a time, a fake clock that
// advances 16 ms per frame, a fake setTimeout registry that fires when the
// clock reaches its due time, and fake elements implementing RevealElement.
import assert from "node:assert/strict";
import {
  HIGHLIGHT_CLASS,
  HIGHLIGHT_MS,
  REVEAL_TIMEOUT_MS,
  cancelReveal,
  revealSetting,
  type RevealDeps,
  type RevealElement,
} from "./settingsReveal";

const FRAME_MS = 16;
const ROOT_SELECTOR = "[data-settings-section]";
const SECTION = "experimental";

interface ScrollOptions {
  block: "center" | "nearest";
  behavior: "auto" | "smooth";
}

/** Every selector string the code under test passed to any query method. */
const selectorLog: string[] = [];

// Only the simple forms the reveal code may use: [name] and [name="value"].
const SIMPLE_SELECTOR = /^\[([a-z-]+)(?:="([^"]*)")?\]$/;

class FakeEl implements RevealElement {
  readonly attrs = new Map<string, string>();
  readonly children: FakeEl[] = [];
  readonly scrolls: ScrollOptions[] = [];
  readonly classes = new Set<string>();
  visible = true;
  throwOnScroll = false;

  constructor(attrs: Record<string, string> = {}) {
    for (const [name, value] of Object.entries(attrs)) {
      this.attrs.set(name, value);
    }
  }

  append(...children: FakeEl[]): this {
    this.children.push(...children);
    return this;
  }

  setAttr(name: string, value: string): void {
    this.attrs.set(name, value);
  }

  getAttribute(name: string): string | null {
    return this.attrs.get(name) ?? null;
  }

  querySelector(sel: string): FakeEl | null {
    selectorLog.push(sel);
    return this.descendants().find((el) => matchesSelector(el, sel)) ?? null;
  }

  querySelectorAll(sel: string): FakeEl[] {
    selectorLog.push(sel);
    return this.descendants().filter((el) => matchesSelector(el, sel));
  }

  getClientRects(): { length: number } {
    return { length: this.visible ? 1 : 0 };
  }

  scrollIntoView(opts: ScrollOptions): void {
    this.scrolls.push(opts);
    if (this.throwOnScroll) {
      throw new Error("scrollIntoView failed");
    }
  }

  classList = {
    add: (name: string): void => {
      this.classes.add(name);
    },
    remove: (name: string): void => {
      this.classes.delete(name);
    },
  };

  private descendants(): FakeEl[] {
    const out: FakeEl[] = [];
    const walk = (el: FakeEl): void => {
      for (const child of el.children) {
        out.push(child);
        walk(child);
      }
    };
    walk(this);
    return out;
  }
}

function matchesSelector(el: FakeEl, sel: string): boolean {
  const match = SIMPLE_SELECTOR.exec(sel);
  if (!match) return false;
  const actual = el.getAttribute(match[1]);
  if (actual === null) return false;
  return match[2] === undefined || actual === match[2];
}

const makeRoot = (section: string, children: FakeEl[] = []): FakeEl =>
  new FakeEl({ "data-settings-section": section }).append(...children);

const makeRow = (title: string): FakeEl =>
  new FakeEl({ "data-setting-title": title });

const makeGroup = (id: string, children: FakeEl[] = []): FakeEl =>
  new FakeEl({ "data-group-id": id }).append(...children);

class Harness {
  now = 0;
  reduced = false;
  docRoot: FakeEl | null;
  readonly rafs = new Map<number, (t: number) => void>();
  readonly timers = new Map<number, { due: number; fn: () => void }>();
  readonly openGroupCalls: string[] = [];
  readonly deps: RevealDeps;
  private nextId = 1;

  constructor(root: FakeEl | null) {
    this.docRoot = root;
    this.deps = {
      doc: {
        querySelector: (sel: string): FakeEl | null => {
          selectorLog.push(sel);
          return sel === ROOT_SELECTOR ? this.docRoot : null;
        },
      },
      raf: (cb: (t: number) => void): number => {
        const id = this.nextId++;
        this.rafs.set(id, cb);
        return id;
      },
      caf: (id: number): void => {
        this.rafs.delete(id);
      },
      now: (): number => this.now,
      setTimeout: (fn: () => void, ms: number): number => {
        const id = this.nextId++;
        this.timers.set(id, { due: this.now + ms, fn });
        return id;
      },
      clearTimeout: (id: number): void => {
        this.timers.delete(id);
      },
      reducedMotion: (): boolean => this.reduced,
      openGroup: (id: string): void => {
        this.openGroupCalls.push(id);
      },
    };
  }

  /** One frame: the clock moves on, due timers fire, then queued rAF callbacks run. */
  frame(): void {
    this.now += FRAME_MS;
    const due = [...this.timers.entries()]
      .filter(([, timer]) => timer.due <= this.now)
      .sort((a, b) => a[1].due - b[1].due || a[0] - b[0]);
    for (const [id, timer] of due) {
      if (this.timers.delete(id)) timer.fn();
    }
    for (const [id, cb] of [...this.rafs.entries()]) {
      if (this.rafs.delete(id)) cb(this.now);
    }
  }

  runUntil(ms: number): void {
    while (this.now < ms) this.frame();
  }
}

const smooth = (block: "center" | "nearest" = "center"): ScrollOptions => ({
  block,
  behavior: "smooth",
});

const scenarios: Record<string, () => void> = {
  "found on the first frame: one smooth center scroll, highlight added then removed after HIGHLIGHT_MS":
    () => {
      cancelReveal();
      const row = makeRow("Same");
      const h = new Harness(makeRoot(SECTION, [row]));
      revealSetting({ section: SECTION, labelText: "Same" }, h.deps);
      h.frame();
      assert.deepEqual(row.scrolls, [smooth("center")]);
      assert.ok(row.classes.has(HIGHLIGHT_CLASS), "highlight class added");
      const foundAt = h.now;
      h.runUntil(foundAt + HIGHLIGHT_MS - 100);
      assert.ok(
        row.classes.has(HIGHLIGHT_CLASS),
        "still highlighted before 1800 ms",
      );
      h.runUntil(foundAt + HIGHLIGHT_MS + 100);
      assert.equal(
        row.classes.has(HIGHLIGHT_CLASS),
        false,
        "removed after 1800 ms",
      );
      assert.equal(row.scrolls.length, 1, "scrolled exactly once");
    },

  "reducedMotion: behavior is auto instead of smooth": () => {
    cancelReveal();
    const row = makeRow("Same");
    const h = new Harness(makeRoot(SECTION, [row]));
    h.reduced = true;
    revealSetting({ section: SECTION, labelText: "Same" }, h.deps);
    h.frame();
    assert.deepEqual(row.scrolls, [{ block: "center", behavior: "auto" }]);
  },

  "occurrence 1 picks the second row with the same title": () => {
    cancelReveal();
    const first = makeRow("Same");
    const second = makeRow("Same");
    const h = new Harness(makeRoot(SECTION, [first, second]));
    revealSetting(
      { section: SECTION, labelText: "Same", occurrence: 1 },
      h.deps,
    );
    h.frame();
    assert.deepEqual(first.scrolls, []);
    assert.equal(first.classes.has(HIGHLIGHT_CLASS), false);
    assert.deepEqual(second.scrolls, [smooth("center")]);
  },

  "a hidden row (no client rects) is skipped until it becomes visible": () => {
    cancelReveal();
    const row = makeRow("Same");
    row.visible = false;
    const h = new Harness(makeRoot(SECTION, [row]));
    revealSetting({ section: SECTION, labelText: "Same" }, h.deps);
    h.frame();
    h.frame();
    assert.deepEqual(row.scrolls, [], "no scroll while hidden");
    row.visible = true;
    h.frame();
    assert.deepEqual(row.scrolls, [smooth("center")], "scrolled once visible");
  },

  "late destination: outgoing root is ignored for frames 1-4, only the destination row is acted on":
    () => {
      cancelReveal();
      const outgoingRow = makeRow("Same");
      const destinationRow = makeRow("Same");
      const h = new Harness(makeRoot("general", [outgoingRow]));
      revealSetting({ section: SECTION, labelText: "Same" }, h.deps);

      const outgoingUntouched = (): void => {
        assert.deepEqual(
          outgoingRow.scrolls,
          [],
          "outgoing row must never scroll",
        );
        assert.equal(
          outgoingRow.classes.has(HIGHLIGHT_CLASS),
          false,
          "outgoing row must never be highlighted",
        );
      };
      for (let frame = 1; frame <= 4; frame++) {
        h.frame();
        outgoingUntouched();
      }
      assert.deepEqual(destinationRow.scrolls, [], "no scroll while waiting");

      h.docRoot = makeRoot(SECTION, [destinationRow]);
      h.frame();
      assert.deepEqual(destinationRow.scrolls, [smooth("center")]);
      assert.ok(destinationRow.classes.has(HIGHLIGHT_CLASS));
      outgoingUntouched();

      h.runUntil(h.now + HIGHLIGHT_MS + 200);
      outgoingUntouched();
      assert.equal(destinationRow.scrolls.length, 1);
    },

  "timeout with groupId: falls back to the group element with block nearest":
    () => {
      cancelReveal();
      const group = makeGroup("engine");
      const root = makeRoot(SECTION, [group]);
      const h = new Harness(root);
      revealSetting(
        { section: SECTION, groupId: "engine", labelText: "Never shown" },
        h.deps,
      );
      h.runUntil(REVEAL_TIMEOUT_MS - FRAME_MS);
      assert.equal(group.scrolls.length, 0, "no fallback before the timeout");
      assert.equal(root.scrolls.length, 0);
      h.runUntil(REVEAL_TIMEOUT_MS + 2 * FRAME_MS);
      assert.equal(group.scrolls.length, 1, "group element scrolled once");
      assert.equal(group.scrolls[0].block, "nearest");
      assert.equal(
        root.scrolls.length,
        0,
        "root not scrolled when group exists",
      );
    },

  "timeout without groupId: falls back to the section root with block nearest":
    () => {
      cancelReveal();
      const root = makeRoot(SECTION, []);
      const h = new Harness(root);
      revealSetting({ section: SECTION, labelText: "Never shown" }, h.deps);
      h.runUntil(REVEAL_TIMEOUT_MS - FRAME_MS);
      assert.equal(root.scrolls.length, 0, "no fallback before the timeout");
      h.runUntil(REVEAL_TIMEOUT_MS + 2 * FRAME_MS);
      assert.equal(root.scrolls.length, 1, "root scrolled once");
      assert.equal(root.scrolls[0].block, "nearest");
    },

  "after ARRIVED, the root changing to another section cancels: no action, registries empty":
    () => {
      cancelReveal();
      const root = makeRoot(SECTION, []);
      const h = new Harness(root);
      revealSetting({ section: SECTION, labelText: "Late" }, h.deps);
      h.frame();
      h.frame();
      root.setAttr("data-settings-section", "general");
      h.frame();
      assert.equal(h.rafs.size, 0, "rAF registry empty after navigation");
      assert.equal(h.timers.size, 0, "timer registry empty after navigation");

      const late = makeRow("Late");
      root.append(late);
      h.runUntil(REVEAL_TIMEOUT_MS + 200);
      assert.deepEqual(late.scrolls, [], "no scroll after cancel");
      assert.equal(
        late.classes.has(HIGHLIGHT_CLASS),
        false,
        "no highlight after cancel",
      );
      assert.deepEqual(root.scrolls, [], "no timeout fallback after cancel");
    },

  "a second revealSetting cancels the first (highlighted) reveal: class removed, first never acts again":
    () => {
      cancelReveal();
      const first = makeRow("First");
      const second = makeRow("Second");
      const h = new Harness(makeRoot(SECTION, [first, second]));
      revealSetting({ section: SECTION, labelText: "First" }, h.deps);
      h.frame();
      assert.ok(first.classes.has(HIGHLIGHT_CLASS));

      revealSetting({ section: SECTION, labelText: "Second" }, h.deps);
      h.frame();
      assert.equal(
        first.classes.has(HIGHLIGHT_CLASS),
        false,
        "first highlight removed",
      );
      assert.deepEqual(second.scrolls, [smooth("center")]);

      h.runUntil(h.now + HIGHLIGHT_MS + 200);
      assert.equal(first.scrolls.length, 1, "first never scrolls again");
      assert.equal(first.classes.has(HIGHLIGHT_CLASS), false);
    },

  "a second revealSetting cancels a still-waiting first reveal (no timeout fallback, no late scroll)":
    () => {
      cancelReveal();
      const second = makeRow("Second");
      const root = makeRoot(SECTION, [second]);
      const h = new Harness(root);
      revealSetting({ section: SECTION, labelText: "Late" }, h.deps);
      h.frame();
      revealSetting({ section: SECTION, labelText: "Second" }, h.deps);
      h.frame();
      assert.deepEqual(second.scrolls, [smooth("center")]);

      const late = makeRow("Late");
      root.append(late);
      h.runUntil(REVEAL_TIMEOUT_MS + 200);
      assert.deepEqual(late.scrolls, [], "first reveal never finds its row");
      assert.deepEqual(root.scrolls, [], "first reveal's timeout never fires");
    },

  "the cancel function of an OLDER reveal does not cancel a newer one": () => {
    cancelReveal();
    const b = makeRow("B");
    const h = new Harness(makeRoot(SECTION, [b]));
    const cancelOlder = revealSetting(
      { section: SECTION, labelText: "A" },
      h.deps,
    );
    revealSetting({ section: SECTION, labelText: "B" }, h.deps);
    cancelOlder();
    h.frame();
    assert.deepEqual(b.scrolls, [smooth("center")], "newer reveal still acts");
    assert.ok(b.classes.has(HIGHLIGHT_CLASS), "newer reveal still highlights");
  },

  "cancelReveal() after a found reveal clears the highlight and the highlight timer":
    () => {
      cancelReveal();
      const row = makeRow("Same");
      const h = new Harness(makeRoot(SECTION, [row]));
      revealSetting({ section: SECTION, labelText: "Same" }, h.deps);
      h.frame();
      assert.ok(row.classes.has(HIGHLIGHT_CLASS));
      cancelReveal();
      assert.equal(
        row.classes.has(HIGHLIGHT_CLASS),
        false,
        "highlight removed",
      );
      assert.equal(h.timers.size, 0, "timers cleared");
      assert.equal(h.rafs.size, 0, "rAF cleared");
      h.runUntil(h.now + HIGHLIGHT_MS + 200);
      assert.equal(row.scrolls.length, 1, "nothing acts after cancel");
    },

  "cancelReveal() while waiting clears the rAF and the timeout": () => {
    cancelReveal();
    const root = makeRoot(SECTION, []);
    const h = new Harness(root);
    revealSetting({ section: SECTION, labelText: "Missing" }, h.deps);
    h.frame();
    cancelReveal();
    assert.equal(h.rafs.size, 0, "rAF cleared");
    assert.equal(h.timers.size, 0, "timeout timer cleared");
    h.runUntil(REVEAL_TIMEOUT_MS + 200);
    assert.deepEqual(root.scrolls, [], "no timeout fallback after cancel");
  },

  "openGroup is called on the first frame and again on each later frame while waiting":
    () => {
      cancelReveal();
      const root = makeRoot(SECTION, []);
      const h = new Harness(root);
      revealSetting(
        { section: SECTION, groupId: "engine", labelText: "Same" },
        h.deps,
      );
      h.frame();
      assert.deepEqual(h.openGroupCalls, ["engine"]);
      h.frame();
      assert.deepEqual(h.openGroupCalls, ["engine", "engine"]);

      // The group card mounts late and contains the row on frame 3.
      const row = makeRow("Same");
      root.append(makeGroup("engine", [row]));
      h.frame();
      assert.deepEqual(h.openGroupCalls, ["engine", "engine", "engine"]);
      assert.deepEqual(row.scrolls, [smooth("center")]);
    },

  "openGroup is never called when groupId is absent": () => {
    cancelReveal();
    const h = new Harness(makeRoot(SECTION, []));
    revealSetting({ section: SECTION, labelText: "Same" }, h.deps);
    h.runUntil(REVEAL_TIMEOUT_MS / 2);
    assert.deepEqual(h.openGroupCalls, []);
  },

  "a groupId failing /^[A-Za-z0-9_-]+$/ is treated as absent and never reaches a selector":
    () => {
      cancelReveal();
      const row = makeRow("Title");
      const h = new Harness(makeRoot(SECTION, [row]));
      selectorLog.length = 0;
      revealSetting(
        { section: SECTION, groupId: 'bad id"]', labelText: "Title" },
        h.deps,
      );
      h.frame();
      assert.deepEqual(
        row.scrolls,
        [smooth("center")],
        "row found without a group scope",
      );
      assert.deepEqual(h.openGroupCalls, [], "bad group id must not be opened");
      assert.ok(
        selectorLog.every((sel) => !sel.includes("bad id")),
        `group id leaked into a selector: ${JSON.stringify(selectorLog)}`,
      );
    },

  "scrollIntoView throwing does not throw out of revealSetting": () => {
    cancelReveal();
    const row = makeRow("Same");
    row.throwOnScroll = true;
    const h = new Harness(makeRoot(SECTION, [row]));
    assert.doesNotThrow(() =>
      revealSetting({ section: SECTION, labelText: "Same" }, h.deps),
    );
    assert.doesNotThrow(() => h.runUntil(REVEAL_TIMEOUT_MS + 200));
    assert.ok(row.scrolls.length >= 1, "scroll was attempted");
  },

  "nothing throws when the section root is null for the whole timeout": () => {
    cancelReveal();
    const h = new Harness(null);
    assert.doesNotThrow(() =>
      revealSetting({ section: SECTION, labelText: "Same" }, h.deps),
    );
    assert.doesNotThrow(() => h.runUntil(REVEAL_TIMEOUT_MS + 200));
  },

  "timing constants: timeout 1000 ms, highlight 1800 ms, class name": () => {
    assert.equal(REVEAL_TIMEOUT_MS, 1000);
    assert.equal(HIGHLIGHT_MS, 1800);
    assert.equal(HIGHLIGHT_CLASS, "settings-search-highlight");
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  } finally {
    cancelReveal();
  }
}
if (failed > 0) {
  throw new Error(`${failed} settings reveal scenarios failed`);
}
console.log(
  `settings reveal: ${Object.keys(scenarios).length} scenarios passed`,
);
