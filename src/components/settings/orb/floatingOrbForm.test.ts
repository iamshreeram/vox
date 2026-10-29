// G3c SPEC (DESIGN.md 'Settings (G1)' and 'Frontend' -> Settings UI). Written first; workers must
// not edit this file. Run: bun src/components/settings/orb/floatingOrbForm.test.ts
//
// Contract of the pure module ./floatingOrbForm.ts:
//   ORB_SIZES: readonly ["small", "medium", "large"]
//   supportReasonKey(block) -> reason i18n key | null
//   toggleState(support | null, orbVisible) -> { checked, disabled, reasonKey }
//   sizeValue(size) -> "small" | "medium" | "large"  (unknown or missing means "medium")
// Plus source-level contracts for FloatingOrbSettings.tsx, the placement list, the search index
// and the i18n keys in every locale.
import assert from "node:assert/strict";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PLACED_GROUPS } from "../settingsPlacement";
import { SEARCH_ENTRIES } from "../search/settingsSearchIndex";
import {
  clampOrbPx,
  effectiveOrbPx,
  ORB_PRESETS,
  ORB_PX_MAX,
  ORB_PX_MIN,
  ORB_PX_STEP,
  ORB_SIZES,
  presetFor,
  sizeValue,
  SURFACE_OPTIONS,
  surfaceHints,
  surfaceToPresentation,
  surfaceValue,
  supportReasonKey,
  toggleState,
} from "./floatingOrbForm";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SRC = path.resolve(HERE, "../../..");
const COMPONENT = path.join(HERE, "FloatingOrbSettings.tsx");
const EN_FILE = path.join(SRC, "i18n", "locales", "en", "translation.json");
const LOCALES_DIR = path.join(SRC, "i18n", "locales");

const REASON_PREFIX = "settings.floatingOrb.reason.";

// ---------- sizes ----------
assert.deepEqual([...ORB_SIZES], ["small", "medium", "large"]);
assert.equal(sizeValue("small"), "small");
assert.equal(sizeValue("medium"), "medium");
assert.equal(sizeValue("large"), "large");
assert.equal(sizeValue(undefined), "medium", "missing size means medium");
assert.equal(sizeValue(null), "medium", "null size means medium");
assert.equal(sizeValue(""), "medium", "empty size means medium");
assert.equal(sizeValue("huge"), "medium", "unknown size means medium");

// ---------- free size (Revision 6): px range, presets, clamp, effective px ----------
assert.equal(ORB_PX_MIN, 48);
assert.equal(ORB_PX_MAX, 192);
assert.equal(ORB_PX_STEP, 4);
assert.deepEqual({ ...ORB_PRESETS }, { small: 72, medium: 96, large: 128 });

assert.equal(clampOrbPx(0), 48, "below the range clamps to 48");
assert.equal(clampOrbPx(-5), 48, "negative clamps to 48");
assert.equal(clampOrbPx(10), 48, "10 clamps to 48");
assert.equal(clampOrbPx(1000), 192, "above the range clamps to 192");
assert.equal(clampOrbPx(192), 192);
assert.equal(clampOrbPx(137), 136, "rounds to the 4 px step");
assert.equal(clampOrbPx(139), 140, "rounds to the 4 px step");
assert.equal(clampOrbPx(96), 96);
assert.equal(clampOrbPx(140), 140);

assert.equal(
  effectiveOrbPx("small", 140),
  140,
  "a finite px wins over the preset",
);
assert.equal(effectiveOrbPx("small", null), 72, "no px: small preset");
assert.equal(effectiveOrbPx("small", undefined), 72, "no px: small preset");
assert.equal(effectiveOrbPx("large", undefined), 128, "no px: large preset");
assert.equal(effectiveOrbPx("medium", null), 96, "no px: medium preset");
assert.equal(effectiveOrbPx("large", NaN), 128, "a non-finite px is ignored");
assert.equal(effectiveOrbPx("huge", null), 96, "unknown size reads as medium");
assert.equal(
  effectiveOrbPx(null, undefined),
  96,
  "missing size reads as medium",
);
assert.equal(
  effectiveOrbPx(undefined, null),
  96,
  "missing size reads as medium",
);

assert.equal(presetFor(72), "small");
assert.equal(presetFor(96), "medium");
assert.equal(presetFor(128), "large");
for (const px of [48, 76, 100, 140, 192]) {
  assert.equal(
    presetFor(px),
    null,
    `${px} px is not a preset (exact match only)`,
  );
}

// ---------- reason keys ----------
assert.equal(supportReasonKey(null), null);
assert.equal(supportReasonKey(undefined), null);
assert.equal(supportReasonKey("wayland"), `${REASON_PREFIX}wayland`);
assert.equal(
  supportReasonKey("unknown_backend"),
  `${REASON_PREFIX}unknown_backend`,
);
assert.equal(
  supportReasonKey("no_compositor"),
  `${REASON_PREFIX}no_compositor`,
);
assert.equal(
  supportReasonKey("unsupported_os"),
  `${REASON_PREFIX}unsupported_os`,
);
for (const other of ["", "x11", "something_new", "WAYLAND"]) {
  assert.equal(
    supportReasonKey(other),
    `${REASON_PREFIX}unknown`,
    `unrecognised block "${other}" falls back to reason.unknown`,
  );
}

// ---------- toggle state ----------
{
  // Still loading: nothing can be decided yet.
  const loading = toggleState(null, true);
  assert.equal(loading.disabled, true, "loading disables the toggle");
  assert.equal(loading.reasonKey, null, "loading shows no reason");
}

{
  // Unsupported: disabled and unchecked even when the orb is recorded as visible.
  const s = toggleState({ supported: false, block: "wayland" }, true);
  assert.equal(s.disabled, true);
  assert.equal(s.checked, false, "unsupported is never checked");
  assert.equal(s.reasonKey, `${REASON_PREFIX}wayland`);
}

for (const block of [
  "wayland",
  "unknown_backend",
  "no_compositor",
  "unsupported_os",
]) {
  const s = toggleState({ supported: false, block }, true);
  assert.equal(s.disabled, true, block);
  assert.equal(s.checked, false, block);
  assert.equal(s.reasonKey, `${REASON_PREFIX}${block}`, block);
}

{
  const s = toggleState({ supported: false, block: null }, true);
  assert.equal(s.disabled, true);
  assert.equal(s.checked, false);
  assert.equal(s.reasonKey, null, "no block means no reason text");
}

for (const orbVisible of [true, false, null, undefined] as const) {
  const s = toggleState({ supported: true, block: null }, orbVisible);
  assert.equal(
    s.disabled,
    false,
    `supported is enabled (${String(orbVisible)})`,
  );
  assert.equal(
    s.checked,
    orbVisible === true,
    `checked for ${String(orbVisible)}`,
  );
  assert.equal(s.reasonKey, null, "supported shows no reason");
}

// ---------- source contracts: component ----------
assert.ok(existsSync(COMPONENT), "FloatingOrbSettings.tsx exists");
const component = readFileSync(COMPONENT, "utf8");

for (const call of [
  /commands\.changeOrbVisibleSetting\(/,
  /commands\.changeOrbSizePxSetting\(/,
  /commands\.resetOrbPosition\(/,
  /commands\.getOrbSupport\(/,
]) {
  assert.match(component, call, `component calls ${call.source}`);
}

// A whole-object write would be overwritten by the backend's preserve rules: orb keys go only
// through their dedicated commands.
assert.doesNotMatch(
  component,
  /updateSetting\(\s*["'`]orb_/,
  "orb_* keys must never be written through updateSetting",
);

// ---------- free size: slider, live label, presets, reset (Revision 6) ----------
assert.match(
  component,
  /commands\.changeOrbSizePxSetting\(/,
  "the size control commits through the px setter",
);
assert.doesNotMatch(
  component,
  /commands\.changeOrbSizeSetting\(/,
  "the legacy preset-only setter is not used by the UI",
);
assert.match(component, /type="range"/, "the size control is a range input");
assert.match(
  component,
  /data-testid="floating-orb-size-slider"/,
  "the slider carries data-testid floating-orb-size-slider",
);
assert.doesNotMatch(
  component,
  /data-testid="floating-orb-size"/,
  "the old size select is gone from the UI",
);

// Returns the text of every useEffect(...) call (paren matched; strings and comments are not
// parsed, which is enough for this component).
function effectBodies(src: string): string[] {
  const bodies: string[] = [];
  let at = src.indexOf("useEffect(");
  while (at !== -1) {
    let depth = 0;
    let i = at + "useEffect".length;
    for (; i < src.length; i += 1) {
      if (src[i] === "(") depth += 1;
      else if (src[i] === ")") {
        depth -= 1;
        if (depth === 0) break;
      }
    }
    bodies.push(src.slice(at, i + 1));
    at = src.indexOf("useEffect(", i + 1);
  }
  return bodies;
}

const effects = effectBodies(component);
assert.ok(
  effects.some((body) => /commands\.getOrbSupport\(/.test(body)),
  "getOrbSupport is requested from an effect (on mount)",
);
for (const body of effects) {
  for (const match of body.matchAll(/commands\.(\w+)\(/g)) {
    assert.equal(
      match[1],
      "getOrbSupport",
      `an effect may call only getOrbSupport on mount, found ${match[1]}`,
    );
  }
  assert.doesNotMatch(body, /\binvoke\(/, "effects never call invoke directly");
}

// ---------- placement: the control lives in General > Conversation ----------
const conversation = PLACED_GROUPS.find((group) => group.id === "conversation");
assert.ok(conversation, "conversation group exists");
assert.equal(conversation.section, "general", "conversation is on General");
assert.ok(
  conversation.controls.some((entry) => (entry.id as string) === "floatingOrb"),
  "conversation group lists the floatingOrb control",
);

// ---------- search: orb rows are findable and open on General ----------
const orbEntries = SEARCH_ENTRIES.filter((entry) =>
  entry.labelKey.startsWith("settings.floatingOrb."),
);
assert.ok(
  orbEntries.length >= 3,
  `expected orb search entries, got ${orbEntries.length}`,
);
for (const key of [
  "settings.floatingOrb.show.title",
  "settings.floatingOrb.size.title",
  "settings.floatingOrb.resetPosition.title",
]) {
  assert.ok(
    orbEntries.some((entry) => entry.labelKey === key),
    `search index has an entry for ${key}`,
  );
}
for (const entry of orbEntries) {
  assert.equal(entry.section, "general", `${entry.id} opens on General`);
}

// ---------- i18n: every key in English and in every locale file ----------
const REQUIRED_KEYS = [
  "settings.floatingOrb.title",
  "settings.floatingOrb.description",
  "settings.floatingOrb.show.title",
  "settings.floatingOrb.show.description",
  "settings.floatingOrb.size.title",
  "settings.floatingOrb.size.description",
  "settings.floatingOrb.size.small",
  "settings.floatingOrb.size.medium",
  "settings.floatingOrb.size.large",
  "settings.floatingOrb.resetPosition.title",
  "settings.floatingOrb.resetPosition.description",
  "settings.floatingOrb.resetPosition.button",
  "settings.floatingOrb.reason.wayland",
  "settings.floatingOrb.reason.unknown_backend",
  "settings.floatingOrb.reason.no_compositor",
  "settings.floatingOrb.reason.unsupported_os",
  "settings.floatingOrb.reason.unknown",
  "settings.general.groups.floatingOrb",
];

function lookup(root: unknown, dotted: string): unknown {
  let node: unknown = root;
  for (const part of dotted.split(".")) {
    if (node === null || typeof node !== "object") return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

const localeNames = readdirSync(LOCALES_DIR).filter((name) =>
  statSync(path.join(LOCALES_DIR, name)).isDirectory(),
);
assert.ok(localeNames.includes("en"), "English locale exists");
assert.ok(localeNames.length >= 26, `only ${localeNames.length} locales`);
for (const name of localeNames) {
  const file = path.join(LOCALES_DIR, name, "translation.json");
  const json: unknown = JSON.parse(readFileSync(file, "utf8"));
  for (const key of REQUIRED_KEYS) {
    const value = lookup(json, key);
    assert.equal(typeof value, "string", `${name}: ${key} must be a string`);
    assert.ok(
      (value as string).trim().length > 0,
      `${name}: ${key} must not be empty`,
    );
  }
}
assert.ok(existsSync(EN_FILE), "English translation file exists");

// ---------- G4b: "When I talk to Vox" surface choice ----------
// Contract: SURFACE_OPTIONS = ["auto", "bubbles", "window"]; surfaceValue maps the stored
// presentation (null / missing / unknown = auto, "floating" = bubbles, "window" = window);
// surfaceToPresentation is its inverse (auto = null); surfaceHints decides the two hints.
assert.deepEqual([...SURFACE_OPTIONS], ["auto", "bubbles", "window"]);

assert.equal(surfaceValue(null), "auto", "null presentation is Automatic");
assert.equal(
  surfaceValue(undefined),
  "auto",
  "missing presentation is Automatic",
);
assert.equal(surfaceValue("floating"), "bubbles", "floating shows as bubbles");
assert.equal(surfaceValue("window"), "window", "window shows as window");
for (const other of ["", "classic", "FLOATING", "bubbles", "auto"]) {
  assert.equal(surfaceValue(other), "auto", `unknown "${other}" reads as auto`);
}

assert.equal(surfaceToPresentation("auto"), null, "auto stores null");
assert.equal(
  surfaceToPresentation("bubbles"),
  "floating",
  "bubbles stores floating",
);
assert.equal(surfaceToPresentation("window"), "window", "window stores window");
for (const option of SURFACE_OPTIONS) {
  assert.equal(
    surfaceValue(surfaceToPresentation(option)),
    option,
    `round trip ${option}`,
  );
}

{
  const base = {
    converseChatLog: true,
    wakeWordEnabled: false,
    wakeWordAction: "voice_request",
    surface: "auto" as const,
    supported: true,
  };
  const none = { chatLogOff: false };

  assert.deepEqual(surfaceHints(base), none, "healthy defaults show no hint");

  // Chat log hint.
  assert.equal(
    surfaceHints({ ...base, converseChatLog: false }).chatLogOff,
    true,
  );
  assert.equal(
    surfaceHints({ ...base, converseChatLog: undefined }).chatLogOff,
    false,
  );
  assert.equal(
    surfaceHints({ ...base, converseChatLog: false, surface: "bubbles" })
      .chatLogOff,
    true,
  );
  assert.equal(
    surfaceHints({ ...base, converseChatLog: false, surface: "window" })
      .chatLogOff,
    false,
    "window hides it",
  );
  assert.equal(
    surfaceHints({ ...base, converseChatLog: false, supported: false })
      .chatLogOff,
    false,
    "unsupported hides it",
  );
  assert.equal(
    surfaceHints({ ...base, converseChatLog: false, supported: null })
      .chatLogOff,
    true,
    "loading does not hide it",
  );

  // The wake-word hint is gone: a wake word always starts a conversation, so no wake
  // setting shows a hint any more.
  const wakeOn = { ...base, wakeWordEnabled: true };
  assert.equal("wakeDictation" in surfaceHints(wakeOn), false, "no wake hint");
  assert.deepEqual(
    surfaceHints({ ...wakeOn, converseChatLog: false }),
    { chatLogOff: true },
    "the chat-log hint shows alone",
  );
}

// ---------- G4b source contracts: the surface select and its hints ----------
assert.match(
  component,
  /commands\.changeChatPresentationSetting\(/,
  "the surface select commits through the gated presentation command",
);
assert.doesNotMatch(
  component,
  /updateSetting\(\s*["'`]chat_presentation["'`]/,
  "chat_presentation must never be written through updateSetting",
);
assert.match(
  component,
  /<select\b[\s\S]*?data-testid="floating-orb-surface"/,
  "the surface control is a native select with data-testid floating-orb-surface",
);
assert.match(
  component,
  new RegExp('data-testid="floating-orb-surface-hint-chatlog"'),
  "component carries the chat-log hint",
);
for (const testId of [
  "floating-orb-surface-hint-wake",
  "floating-orb-wake-conversation",
]) {
  assert.doesNotMatch(
    component,
    new RegExp(`data-testid="${testId}"`),
    `component no longer carries ${testId}`,
  );
}
assert.doesNotMatch(
  component,
  /commands\.changeWakeWordActionSetting\(/,
  "the orb group no longer sets the wake-word action",
);
assert.doesNotMatch(
  component,
  /updateSetting\(\s*["'`]wake_word_action["'`]/,
  "the orb group sets wake_word_action only through its command",
);

// ---------- G4b i18n: surface and hint keys in every locale ----------
const SURFACE_KEYS = [
  "settings.floatingOrb.surface.title",
  "settings.floatingOrb.surface.description",
  "settings.floatingOrb.surface.auto",
  "settings.floatingOrb.surface.bubbles",
  "settings.floatingOrb.surface.window",
  "settings.floatingOrb.hints.chatLogOff",
  "settings.floatingOrb.hints.wakeDictation",
  "settings.floatingOrb.hints.wakeButton",
];
for (const name of localeNames) {
  const file = path.join(LOCALES_DIR, name, "translation.json");
  const json: unknown = JSON.parse(readFileSync(file, "utf8"));
  for (const key of SURFACE_KEYS) {
    const value = lookup(json, key);
    assert.equal(typeof value, "string", `${name}: ${key} must be a string`);
    assert.ok(
      (value as string).trim().length > 0,
      `${name}: ${key} must not be empty`,
    );
  }
}

console.log("floatingOrbForm.test.ts: all assertions passed");
