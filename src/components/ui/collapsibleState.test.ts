import assert from "node:assert/strict";
import { openStorageKey, readOpen, writeOpen } from "./collapsibleState";

type Store = Pick<Storage, "getItem" | "setItem">;
const memory = (
  initial: Record<string, string> = {},
): Store & {
  data: Record<string, string>;
} => {
  const data = { ...initial };
  return {
    data,
    getItem: (k) => (k in data ? data[k] : null),
    setItem: (k, v) => {
      data[k] = v;
    },
  };
};
const throwing: Store = {
  getItem: () => {
    throw new Error("denied");
  },
  setItem: () => {
    throw new Error("denied");
  },
};

const scenarios: Record<string, () => void> = {
  "storage key is namespaced per group id": () => {
    assert.equal(openStorageKey("engine"), "vox.settings.group.open.engine");
  },
  "stored 1 and 0 override the default": () => {
    const s = memory({
      [openStorageKey("a")]: "1",
      [openStorageKey("b")]: "0",
    });
    assert.equal(readOpen("a", false, s), true);
    assert.equal(readOpen("b", true, s), false);
  },
  "missing, garbage and empty values fall back to the default": () => {
    const s = memory({
      [openStorageKey("g")]: "banana",
      [openStorageKey("e")]: "",
    });
    assert.equal(readOpen("none", true, s), true);
    assert.equal(readOpen("none", false, s), false);
    assert.equal(readOpen("g", true, s), true);
    assert.equal(readOpen("e", false, s), false);
  },
  "unavailable or throwing storage falls back to the default": () => {
    assert.equal(readOpen("x", true, undefined), true);
    assert.equal(readOpen("x", false, undefined), false);
    assert.equal(readOpen("x", true, throwing), true);
    assert.equal(readOpen("x", false, throwing), false);
  },
  "write stores 1/0 and round-trips": () => {
    const s = memory();
    writeOpen("w", true, s);
    assert.equal(s.data[openStorageKey("w")], "1");
    writeOpen("w", false, s);
    assert.equal(s.data[openStorageKey("w")], "0");
    assert.equal(readOpen("w", true, s), false);
  },
  "write never throws on missing or throwing storage": () => {
    assert.doesNotThrow(() => writeOpen("w", true, undefined));
    assert.doesNotThrow(() => writeOpen("w", true, throwing));
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
if (failed > 0) throw new Error(`${failed} collapsible state scenarios failed`);
console.log(
  `collapsible state: ${Object.keys(scenarios).length} scenarios passed`,
);
