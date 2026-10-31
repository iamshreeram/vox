// Unit tests for the orb's dismissal helpers (T3). Run with bun.
import assert from "node:assert/strict";
import { createSingleFlight, isDismissKey } from "./orbDismiss";

assert.equal(isDismissKey({ key: "Escape" }), true, "Escape dismisses");
assert.equal(
  isDismissKey({ key: "Escape", repeat: true }),
  false,
  "an auto-repeated Escape does not dismiss again",
);
for (const key of ["Enter", " ", "a", "Tab", "F10"]) {
  assert.equal(isDismissKey({ key }), false, `${key} does not dismiss`);
}

async function main(): Promise<void> {
  let calls = 0;
  let release: () => void = () => undefined;
  const once = createSingleFlight(
    () =>
      new Promise<void>((resolve) => {
        calls += 1;
        release = resolve;
      }),
  );
  once();
  once();
  once();
  assert.equal(calls, 1, "calls while one is pending are ignored");
  release();
  await new Promise((resolve) => setTimeout(resolve, 0));
  once();
  assert.equal(calls, 2, "a later press runs again once the first settled");

  let rejected = 0;
  const failing = createSingleFlight(() => {
    rejected += 1;
    return Promise.reject(new Error("refused"));
  });
  failing();
  await new Promise((resolve) => setTimeout(resolve, 0));
  failing();
  assert.equal(
    rejected,
    2,
    "a rejected call re-arms the guard without throwing",
  );
  console.log("orbDismiss ok");
}

void main();
