// G3b-A spec (DESIGN.md Rev 5, 'Frontend'): pure model of the orb page. Authoritative: do not
// weaken an assertion; if a test is wrong, report it. Run with `bun run test:orb`.
//
// Contract (src/orb/orbModel.ts):
//   GAZE_MAX_UNITS = 4, GLINT_MIN_MS = 2500, GLINT_MAX_MS = 7000, GLINT_MS = 130, POUNCE_MS = 450
//   gazeOffset({x, y}) -> {dx, dy}: each axis clamped to [-1, 1] (non-finite -> 0) then x GAZE_MAX_UNITS
//   nextGlintDelay(rng) -> ms in [GLINT_MIN_MS, GLINT_MAX_MS] (rng clamped into [0, 1))
//   OrbState {hover, grabbing, pouncing, glint, reducedMotion: boolean; gestureId: number | null; face: AvatarFace}
//   initialOrbState; orbReducer(state, action); orbClasses(state) -> string[]; keyCommand(event)
//   faceFromEvent(prev, event) / initialFaceView(): the orb reads ONLY `state` events from converse-chat
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import {
  GAZE_MAX_UNITS,
  GLINT_MAX_MS,
  GLINT_MIN_MS,
  faceFromEvent,
  gazeOffset,
  initialFaceView,
  initialOrbState,
  keyCommand,
  nextGlintDelay,
  orbClasses,
  orbReducer,
  type OrbAction,
  type OrbState,
} from "./orbModel";

const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");

const run = (...actions: OrbAction[]): OrbState =>
  actions.reduce(orbReducer, initialOrbState);

const scenarios: Record<string, () => void> = {
  "gaze offsets are clamped and capped at four units": () => {
    assert.deepEqual(gazeOffset({ x: 0, y: 0 }), { dx: 0, dy: 0 });
    assert.deepEqual(gazeOffset({ x: 1, y: -1 }), {
      dx: GAZE_MAX_UNITS,
      dy: -GAZE_MAX_UNITS,
    });
    assert.deepEqual(gazeOffset({ x: 5, y: -9 }), { dx: 4, dy: -4 });
    assert.deepEqual(gazeOffset({ x: 0.5, y: 0.25 }), { dx: 2, dy: 1 });
    assert.deepEqual(gazeOffset({ x: Number.NaN, y: Infinity }), {
      dx: 0,
      dy: 0,
    });
  },

  "glint delays stay inside the 2.5 to 7 second window": () => {
    assert.equal(
      nextGlintDelay(() => 0),
      GLINT_MIN_MS,
    );
    assert.ok(nextGlintDelay(() => 0.999999) <= GLINT_MAX_MS);
    assert.ok(nextGlintDelay(() => 1) <= GLINT_MAX_MS, "rng 1 is clamped");
    assert.ok(
      nextGlintDelay(() => -3) >= GLINT_MIN_MS,
      "rng below 0 is clamped",
    );
    assert.equal(
      nextGlintDelay(() => 0.5),
      (GLINT_MIN_MS + GLINT_MAX_MS) / 2,
    );
    assert.equal(GLINT_MIN_MS, 2500);
    assert.equal(GLINT_MAX_MS, 7000);
  },

  "a still press followed by a click result pounces and releases the grab":
    () => {
      let s = run({ type: "down", id: 1 });
      assert.equal(s.grabbing, true);
      assert.equal(s.gestureId, 1);
      s = orbReducer(s, { type: "gesture", id: 1, result: "click" });
      assert.equal(s.grabbing, false);
      assert.equal(s.pouncing, true);
      assert.equal(s.gestureId, null);
      s = orbReducer(s, { type: "pounceEnd" });
      assert.equal(s.pouncing, false);
    },

  "drag and cancel results never pounce": () => {
    for (const result of ["drag", "cancel"] as const) {
      const s = run(
        { type: "down", id: 3 },
        { type: "gesture", id: 3, result },
      );
      assert.equal(s.pouncing, false, result);
      assert.equal(s.grabbing, false, result);
      assert.equal(s.gestureId, null, result);
    }
  },

  "a result for an old gesture id is ignored": () => {
    const s = run({ type: "down", id: 2 });
    const same = orbReducer(s, { type: "gesture", id: 1, result: "click" });
    assert.equal(same, s, "state object is unchanged");
    const idle = orbReducer(initialOrbState, {
      type: "gesture",
      id: 9,
      result: "click",
    });
    assert.equal(idle, initialOrbState, "a result with no gesture in flight");
  },

  "a new press replaces the old one and clears a pounce in progress": () => {
    const s = run(
      { type: "down", id: 1 },
      { type: "gesture", id: 1, result: "click" },
      { type: "down", id: 2 },
    );
    assert.equal(s.pouncing, false);
    assert.equal(s.gestureId, 2);
    assert.equal(s.grabbing, true);
  },

  "reduced motion disables the pounce and the glint": () => {
    let s = run({ type: "reducedMotion", on: true });
    s = orbReducer(s, { type: "down", id: 1 });
    s = orbReducer(s, { type: "gesture", id: 1, result: "click" });
    assert.equal(s.pouncing, false);
    s = orbReducer(s, { type: "glint", on: true });
    assert.equal(s.glint, false);
    s = run({ type: "glint", on: true }, { type: "reducedMotion", on: true });
    assert.equal(s.glint, false, "turning it on clears a running glint");
  },

  "hover and face are independent of the gesture": () => {
    let s = run(
      { type: "hover", on: true },
      { type: "face", face: "speaking" },
    );
    assert.equal(s.hover, true);
    assert.equal(s.face, "speaking");
    s = orbReducer(s, { type: "down", id: 1 });
    assert.equal(s.hover, true);
    assert.equal(s.face, "speaking");
    s = orbReducer(s, { type: "hover", on: false });
    assert.equal(s.hover, false);
  },

  "class names list only the active states in a fixed order": () => {
    assert.deepEqual(orbClasses(initialOrbState), []);
    const s: OrbState = {
      ...initialOrbState,
      hover: true,
      grabbing: true,
      pouncing: true,
      glint: true,
      reducedMotion: true,
    };
    assert.deepEqual(orbClasses(s), [
      "is-hover",
      "is-grabbing",
      "is-pouncing",
      "is-glint",
      "is-reduced",
    ]);
    assert.deepEqual(orbClasses({ ...initialOrbState, pouncing: true }), [
      "is-pouncing",
    ]);
  },

  "keyboard map: Enter and Space activate, ContextMenu and Shift+F10 open the menu":
    () => {
      assert.equal(keyCommand({ key: "Enter", shiftKey: false }), "activate");
      assert.equal(keyCommand({ key: " ", shiftKey: false }), "activate");
      assert.equal(keyCommand({ key: "ContextMenu", shiftKey: false }), "menu");
      assert.equal(keyCommand({ key: "F10", shiftKey: true }), "menu");
      assert.equal(keyCommand({ key: "F10", shiftKey: false }), null);
      assert.equal(keyCommand({ key: "Escape", shiftKey: false }), null);
      assert.equal(keyCommand({ key: "a", shiftKey: false }), null);
    },

  "the face follows state events only and ignores stale or foreign events":
    () => {
      let v = initialFaceView();
      assert.deepEqual(v, { seq: 0, state: "idle", end: null });
      v = faceFromEvent(v, {
        seq: 1,
        kind: { type: "state", state: "listening", end: null },
      });
      assert.equal(v.state, "listening");
      const stale = faceFromEvent(v, {
        seq: 1,
        kind: { type: "state", state: "speaking", end: null },
      });
      assert.deepEqual(stale, v, "equal or lower seq is ignored");
      const entry = faceFromEvent(v, {
        seq: 2,
        kind: {
          type: "entry",
          entry: { id: 1, tsMs: 1, role: "user", text: "TOP SECRET" },
        },
      });
      assert.deepEqual(entry, v, "entries never reach the orb view");
      assert.ok(!JSON.stringify(entry).includes("SECRET"));
      for (const junk of [
        null,
        undefined,
        5,
        "x",
        {},
        { seq: "a" },
        { seq: 3, kind: 7 },
        { seq: 3, kind: { type: "state", state: 5 } },
      ]) {
        assert.deepEqual(faceFromEvent(v, junk), v);
      }
      const ended = faceFromEvent(v, {
        seq: 5,
        kind: { type: "state", state: "idle", end: "user_ended" },
      });
      assert.deepEqual(ended, { seq: 5, state: "idle", end: "user_ended" });
    },

  "the orb entry exists, shares the avatar styles and never touches the network":
    () => {
      for (const f of [
        "./index.html",
        "./main.tsx",
        "./OrbApp.tsx",
        "./orb.css",
        "../chat/avatar.css",
      ]) {
        assert.ok(existsSync(new URL(f, import.meta.url)), `${f} exists`);
      }
      const chatCss = read("../chat/chat.css");
      assert.match(
        chatCss,
        /avatar\.css/,
        "chat.css imports the shared avatar styles",
      );
      assert.ok(
        !/@keyframes vox-breathe/.test(chatCss),
        "keyframes moved out of chat.css",
      );
      assert.match(read("../chat/avatar.css"), /@keyframes vox-breathe/);
      const vite = read("../../vite.config.ts");
      assert.match(
        vite,
        /orb:\s*resolve\(__dirname,\s*"src\/orb\/index\.html"\)/,
      );
      const app = read("./OrbApp.tsx") + read("./main.tsx");
      for (const banned of [
        "fetch(",
        "XMLHttpRequest",
        "WebSocket",
        "eval(",
        "innerHTML",
        "dangerouslySetInnerHTML",
        "getChatSnapshot",
        "sendChatMessage",
      ]) {
        assert.ok(!app.includes(banned), `orb page must not use ${banned}`);
      }
      const css = read("./orb.css");
      assert.match(css, /prefers-reduced-motion:\s*reduce/);
      assert.match(css, /--gaze-x/);
    },

  "every animation is switched off under reduced motion and strings are translated":
    () => {
      const css = read("./orb.css");
      const block = css.slice(css.indexOf("prefers-reduced-motion"));
      assert.match(block, /animation:\s*none\s*!important/);
      assert.match(block, /transition:\s*none\s*!important/);
      const en = JSON.parse(read("../i18n/locales/en/translation.json")) as {
        orb?: { label?: string };
      };
      assert.equal(typeof en.orb?.label, "string");
    },
};

let failed = 0;
for (const [name, fn] of Object.entries(scenarios)) {
  try {
    fn();
    console.log(`ok - ${name}`);
  } catch (error) {
    failed += 1;
    console.error(`FAIL - ${name}`);
    console.error(error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log("orbModel ok");
