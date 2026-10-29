import assert from "node:assert/strict";
import { wakeToggleDisabledByAmbient } from "./wakeWordModel";

let passed = 0;
function scenario(name: string, body: () => void) {
  try {
    body();
    passed += 1;
  } catch (err) {
    console.error(`FAILED: ${name}`);
    throw err;
  }
}

scenario(
  "Experimental off + ambient flag on leaves the wake toggle enabled",
  () => {
    assert.equal(
      wakeToggleDisabledByAmbient({
        experimental_enabled: false,
        ambient_mode_enabled: true,
      }),
      false,
    );
  },
);

scenario("Experimental on + ambient flag on disables the wake toggle", () => {
  assert.equal(
    wakeToggleDisabledByAmbient({
      experimental_enabled: true,
      ambient_mode_enabled: true,
    }),
    true,
  );
});

scenario("ambient flag off leaves the wake toggle enabled", () => {
  assert.equal(
    wakeToggleDisabledByAmbient({
      experimental_enabled: true,
      ambient_mode_enabled: false,
    }),
    false,
  );
});

scenario("missing settings read as off, so the toggle stays enabled", () => {
  assert.equal(wakeToggleDisabledByAmbient({}), false);
});

console.log(`wakeWordModel.test.ts: ${passed} scenarios passed`);
