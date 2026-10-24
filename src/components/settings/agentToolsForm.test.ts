import assert from "node:assert/strict";
import {
  MAX_PROJECT_ROOTS,
  appendProjectRoot,
  canAddProjectRoot,
  SETTINGS_SECTION_IDS,
  isSettingsSectionId,
  resolveSectionAlias,
} from "./agentToolsForm";

// Every existing sidebar section except "debug" is accepted.
for (const id of [
  "general",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "experimental",
  "about",
]) {
  assert.equal(isSettingsSectionId(id), true, `expected ${id} to be accepted`);
}

// "debug", unknown ids, case variants, empty and non-string values are refused.
const refused: unknown[] = [
  "debug",
  "unknown",
  "",
  " general",
  "General",
  "postProcessing",
  null,
  undefined,
  3,
  ["general"],
  {},
];
for (const value of refused) {
  assert.equal(
    isSettingsSectionId(value),
    false,
    `expected ${JSON.stringify(value)} to be refused`,
  );
}

// Adding folders: empty and whitespace-only input is refused.
assert.equal(canAddProjectRoot([], ""), false);
assert.equal(canAddProjectRoot([], "   "), false);

// Duplicates are refused, including when only whitespace differs.
assert.equal(canAddProjectRoot(["/a/b"], "/a/b"), false);
assert.equal(canAddProjectRoot(["/a/b"], "  /a/b  "), false);

// A new distinct path is accepted.
assert.equal(canAddProjectRoot(["/a/b"], "/c/d"), true);

// The list cap is enforced at exactly MAX_PROJECT_ROOTS.
assert.equal(MAX_PROJECT_ROOTS, 20);
const nineteen = Array.from({ length: 19 }, (_, i) => `/p/${i}`);
const twenty = [...nineteen, "/p/19"];
assert.equal(canAddProjectRoot(nineteen, "/p/new"), true);
assert.equal(canAddProjectRoot(twenty, "/p/new"), false);

// Appending trims the candidate and does not mutate the input list.
const original = ["/a"];
const appended = appendProjectRoot(original, "  /b  ");
assert.deepEqual(appended, ["/a", "/b"]);
assert.deepEqual(original, ["/a"]);

// The Agents tab id is accepted; the old "experimental" id stays accepted as a legacy
// alias that resolves to it; "debug" stays unreachable.
assert.equal(isSettingsSectionId("agents"), true);
assert.equal(isSettingsSectionId("experimental"), true);
assert.equal(isSettingsSectionId("debug"), false);
assert.deepEqual(
  [...SETTINGS_SECTION_IDS],
  [
    "general",
    "history",
    "models",
    "advanced",
    "postprocessing",
    "agents",
    "experimental",
    "about",
  ],
);
assert.equal(resolveSectionAlias("experimental"), "agents");
for (const id of [
  "general",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "agents",
  "about",
] as const) {
  assert.equal(resolveSectionAlias(id), id);
}

console.log("agentToolsForm.test.ts: all assertions passed");
