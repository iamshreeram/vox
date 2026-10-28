import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { nextFirstRunStep } from "./firstRunFlow";

// P4 spec (DESIGN.md Rev 2/3/4 'First run'): after the Speech-to-text pick a fresh install sees a
// Voice step and a Brain step, each skippable; returning users never see them automatically.
// Contract: `nextFirstRunStep(settings)` mirrors Rust `first_run::next_step`.
const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");
const en = JSON.parse(read("../../i18n/locales/en/translation.json")) as Record<
  string,
  unknown
>;
const lookup = (key: string): unknown =>
  key
    .split(".")
    .reduce<unknown>(
      (node, part) =>
        node !== null && typeof node === "object"
          ? (node as Record<string, unknown>)[part]
          : undefined,
      en,
    );

/** Source of every `useEffect(...)` call in a file (balanced parentheses). */
function effectBodies(source: string): string[] {
  const bodies: string[] = [];
  let from = 0;
  for (;;) {
    const at = source.indexOf("useEffect(", from);
    if (at < 0) return bodies;
    let depth = 0;
    let end = at + "useEffect".length;
    for (; end < source.length; end += 1) {
      if (source[end] === "(") depth += 1;
      if (source[end] === ")") {
        depth -= 1;
        if (depth === 0) break;
      }
    }
    bodies.push(source.slice(at, end + 1));
    from = end + 1;
  }
}

const scenarios: Record<string, () => void> = {
  "a store without first-run state shows nothing": () => {
    assert.equal(nextFirstRunStep({}), null);
    assert.equal(nextFirstRunStep({ first_run_setup_pending: false }), null);
    assert.equal(
      nextFirstRunStep({
        first_run_setup_pending: false,
        first_run_tts: "unset",
        first_run_brain: "unset",
      }),
      null,
    );
  },

  "a fresh install shows Voice first, then Brain, then nothing": () => {
    const base = { first_run_setup_pending: true } as const;
    assert.equal(nextFirstRunStep(base), "voice");
    assert.equal(
      nextFirstRunStep({
        ...base,
        first_run_tts: "unset",
        first_run_brain: "unset",
      }),
      "voice",
    );
    assert.equal(nextFirstRunStep({ ...base, first_run_tts: "done" }), "brain");
    assert.equal(
      nextFirstRunStep({ ...base, first_run_tts: "skipped" }),
      "brain",
    );
    assert.equal(
      nextFirstRunStep({
        ...base,
        first_run_tts: "done",
        first_run_brain: "done",
      }),
      null,
    );
    assert.equal(
      nextFirstRunStep({
        ...base,
        first_run_tts: "skipped",
        first_run_brain: "skipped",
      }),
      null,
    );
  },

  "a restart resumes at the first unresolved step": () => {
    assert.equal(
      nextFirstRunStep({
        first_run_setup_pending: true,
        first_run_tts: "done",
        first_run_brain: "unset",
      }),
      "brain",
    );
  },

  "App.tsx runs the extra steps after the model pick and before done": () => {
    const app = read("../../App.tsx");
    assert.match(app, /"voice"/);
    assert.match(app, /"brain"/);
    assert.match(app, /nextFirstRunStep/);
    assert.match(app, /FirstRunVoiceStep/);
    assert.match(app, /FirstRunBrainStep/);
    // The route to the main app waits for the steps; both new steps keep onboarding styling.
    assert.match(app, /onboardingStep === "voice"/);
    assert.match(app, /onboardingStep === "brain"/);
  },

  "the step components exist and use the shared pieces": () => {
    for (const file of [
      "./FirstRunVoiceStep.tsx",
      "./FirstRunBrainStep.tsx",
      "./firstRunFlow.ts",
    ]) {
      assert.ok(existsSync(new URL(file, import.meta.url)), `${file} exists`);
    }
    const voice = read("./FirstRunVoiceStep.tsx");
    assert.match(
      voice,
      /TtsEngineCard|ModelRow/,
      "voice choices use the shared model row",
    );
    assert.match(voice, /firstRunFinishStep/);
    const brain = read("./FirstRunBrainStep.tsx");
    assert.match(brain, /useBrainActions|BrainEndpointGroup|BrainTestGroup/);
    assert.match(brain, /firstRunFinishStep/);
  },

  "no network call hides in an effect, and Skip never calls the Brain": () => {
    for (const file of ["./FirstRunVoiceStep.tsx", "./FirstRunBrainStep.tsx"]) {
      const source = read(file);
      for (const body of effectBodies(source)) {
        assert.ok(
          !/brainTestConnection|brainFetchModels|testConnection|fetchModels|brainAcknowledgeTest/.test(
            body,
          ),
          `${file}: an effect must not call the Brain network commands`,
        );
      }
    }
    const brain = read("./FirstRunBrainStep.tsx");
    const skip = brain.indexOf("skip");
    assert.ok(skip > 0, "a Skip control exists");
    assert.match(brain, /"skipped"/);
    assert.match(read("./FirstRunVoiceStep.tsx"), /"skipped"/);
  },

  "the 'Use this for spoken replies' button is the only thing that finishes Brain as done":
    () => {
      const brain = read("./FirstRunBrainStep.tsx");
      const uses = brain.split('"done"').length - 1;
      assert.equal(
        uses,
        1,
        "exactly one place resolves the Brain step as done",
      );
      assert.match(brain, /brainUseForFeature/);
    },

  "strings are translated": () => {
    for (const key of [
      "onboarding.firstRun.voice.title",
      "onboarding.firstRun.voice.description",
      "onboarding.firstRun.voice.continue",
      "onboarding.firstRun.voice.skip",
      "onboarding.firstRun.brain.title",
      "onboarding.firstRun.brain.description",
      "onboarding.firstRun.brain.useForReplies",
      "onboarding.firstRun.brain.skip",
      "settings.models.finishSetup.title",
      "settings.models.finishSetup.action",
      "settings.models.finishSetup.dismiss",
    ]) {
      assert.equal(typeof lookup(key), "string", `${key} in the English file`);
    }
  },

  "returning users get a dismissible Finish voice setup callout on Models":
    () => {
      const models = read("../settings/models/ModelsSettings.tsx");
      assert.match(models, /FinishSetupCallout/);
      const callout = "../settings/models/FinishSetupCallout.tsx";
      assert.ok(existsSync(new URL(callout, import.meta.url)));
      const source = read(callout);
      assert.match(source, /localStorage/);
      assert.match(source, /brain_provider_id|brain_model/);
      assert.ok(
        !/brainTestConnection|brainFetchModels/.test(source),
        "the callout never makes requests",
      );
    },

  "the converse preflight tells the user which piece is missing": () => {
    const rust = read("../../../src-tauri/src/managers/converse_platform.rs");
    assert.match(
      rust,
      /Models/,
      "spoken-reply message points at the Models page",
    );
    assert.match(rust, /Brain/, "reply-model message points at the Brain page");
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
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
console.log("firstRunFlow ok");
