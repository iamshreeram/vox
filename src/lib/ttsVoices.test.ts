import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  buildVoiceOptions,
  type TtsVoiceInfo,
  type VoiceLabels,
  voiceValueToSetting,
} from "./ttsVoices";

// Distinctive fake labels so hard-coded strings in the implementation fail.
const labels: VoiceLabels = {
  defaultLabel: "ZZ-DEFAULT-LABEL",
  notInstalled: (name: string) => `<<NOT-INSTALLED:${name}>>`,
};

const samantha: TtsVoiceInfo = { name: "Samantha", locale: "en_US" };
const alex: TtsVoiceInfo = { name: "Alex", locale: "en_US" };
const daniel: TtsVoiceInfo = { name: "Daniel", locale: "en_GB" };
const voices: TtsVoiceInfo[] = [samantha, alex, daniel];

const DEFAULT_OPTION = { value: "", label: "ZZ-DEFAULT-LABEL" };
const LIST_KEY = "settings.advanced.tts.voiceListDescription";
const TEXT_KEY = "settings.advanced.tts.voiceDescription";

// Each case runs independently so one failure does not hide the others.
const failures: string[] = [];
function testCase(name: string, fn: () => void): void {
  try {
    fn();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    failures.push(`${name}: ${message}`);
  }
}

testCase("empty list with null current falls back to text input", () => {
  const result = buildVoiceOptions([], null, labels);
  assert.deepEqual(result.options, [DEFAULT_OPTION]);
  assert.equal(result.selected, "");
  assert.equal(result.useTextFallback, true);
  assert.equal(result.descriptionKey, TEXT_KEY);
});

testCase(
  "empty list with non-empty current: fallback, no not-installed option",
  () => {
    const result = buildVoiceOptions([], "Samantha", labels);
    assert.deepEqual(result.options, [DEFAULT_OPTION]);
    assert.equal(result.selected, "Samantha");
    assert.equal(result.useTextFallback, true);
    assert.equal(result.descriptionKey, TEXT_KEY);
  },
);

testCase(
  "empty list with whitespace current: fallback selects trimmed value",
  () => {
    const result = buildVoiceOptions([], "   ", labels);
    assert.deepEqual(result.options, [DEFAULT_OPTION]);
    assert.equal(result.selected, "");
    assert.equal(result.useTextFallback, true);
    assert.equal(result.descriptionKey, TEXT_KEY);
  },
);

testCase(
  "null current with voices: selected is empty string, no extra option",
  () => {
    const result = buildVoiceOptions(voices, null, labels);
    assert.deepEqual(result.options, [
      DEFAULT_OPTION,
      { value: "Samantha", label: "Samantha (en_US)" },
      { value: "Alex", label: "Alex (en_US)" },
      { value: "Daniel", label: "Daniel (en_GB)" },
    ]);
    assert.equal(result.selected, "");
    assert.equal(result.useTextFallback, false);
    assert.equal(result.descriptionKey, LIST_KEY);
  },
);

testCase(
  "empty string current: selected is empty string, no extra option",
  () => {
    const result = buildVoiceOptions(voices, "", labels);
    assert.equal(result.options.length, 1 + voices.length);
    assert.equal(result.selected, "");
    assert.equal(
      result.options.some((option) => option.value === ""),
      true,
    );
    assert.equal(
      result.options.filter((option) => option.value === "").length,
      1,
    );
  },
);

testCase(
  "whitespace-only current: selected is empty string, no extra option",
  () => {
    const result = buildVoiceOptions(voices, "   \t ", labels);
    assert.equal(result.options.length, 1 + voices.length);
    assert.equal(result.selected, "");
    assert.equal(result.useTextFallback, false);
    assert.equal(result.descriptionKey, LIST_KEY);
  },
);

testCase("current present in list: selected equals it, no extra option", () => {
  const result = buildVoiceOptions(voices, "Alex", labels);
  assert.equal(result.options.length, 1 + voices.length);
  assert.equal(result.selected, "Alex");
  assert.equal(result.useTextFallback, false);
  assert.equal(result.descriptionKey, LIST_KEY);
  assert.equal(
    result.options.filter((option) => option.value === "Alex").length,
    1,
  );
});

testCase(
  "current absent: one not-installed option appended last and selected",
  () => {
    const result = buildVoiceOptions(voices, "Zarvox", labels);
    assert.deepEqual(result.options, [
      DEFAULT_OPTION,
      { value: "Samantha", label: "Samantha (en_US)" },
      { value: "Alex", label: "Alex (en_US)" },
      { value: "Daniel", label: "Daniel (en_GB)" },
      { value: "Zarvox", label: "<<NOT-INSTALLED:Zarvox>>" },
    ]);
    assert.equal(result.selected, "Zarvox");
    assert.equal(result.useTextFallback, false);
    assert.equal(result.descriptionKey, LIST_KEY);
    assert.equal(
      result.options.filter((option) => option.value === "Zarvox").length,
      1,
    );
  },
);

testCase(
  "current with surrounding spaces and present: trimmed, no extra option",
  () => {
    const result = buildVoiceOptions(voices, "  Samantha  ", labels);
    assert.equal(result.options.length, 1 + voices.length);
    assert.equal(result.selected, "Samantha");
    assert.equal(
      result.options.filter((option) => option.value === "Samantha").length,
      1,
    );
  },
);

testCase(
  "current with surrounding spaces and absent: trimmed name in not-installed option",
  () => {
    const result = buildVoiceOptions(voices, "  Zarvox  ", labels);
    assert.equal(result.options.length, 2 + voices.length);
    assert.equal(result.selected, "Zarvox");
    const last = result.options[result.options.length - 1];
    assert.deepEqual(last, {
      value: "Zarvox",
      label: "<<NOT-INSTALLED:Zarvox>>",
    });
  },
);

testCase("duplicate voice names: first wins, later ones dropped", () => {
  const dupes: TtsVoiceInfo[] = [
    { name: "Samantha", locale: "en_US" },
    { name: "Samantha", locale: "en_GB" },
    { name: "Alex", locale: "en_US" },
  ];
  const result = buildVoiceOptions(dupes, null, labels);
  assert.deepEqual(result.options, [
    DEFAULT_OPTION,
    { value: "Samantha", label: "Samantha (en_US)" },
    { value: "Alex", label: "Alex (en_US)" },
  ]);
});

testCase("order preserved with default option first", () => {
  const reversed: TtsVoiceInfo[] = [daniel, alex, samantha];
  const result = buildVoiceOptions(reversed, null, labels);
  assert.deepEqual(
    result.options.map((option) => option.value),
    ["", "Daniel", "Alex", "Samantha"],
  );
});

testCase("labels come from the supplied strings", () => {
  const other: VoiceLabels = {
    defaultLabel: "QQ-OTHER-DEFAULT",
    notInstalled: (name: string) => `[[missing ${name}]]`,
  };
  const result = buildVoiceOptions(voices, "Nope", other);
  assert.equal(result.options[0].label, "QQ-OTHER-DEFAULT");
  assert.equal(
    result.options[result.options.length - 1].label,
    "[[missing Nope]]",
  );
});

testCase("voiceValueToSetting: empty and whitespace map to null", () => {
  assert.equal(voiceValueToSetting(""), null);
  assert.equal(voiceValueToSetting("   "), null);
});

testCase("voiceValueToSetting: names pass through, trimmed", () => {
  assert.equal(voiceValueToSetting("Samantha"), "Samantha");
  assert.equal(voiceValueToSetting("  Samantha  "), "Samantha");
});

// Locale files: every translation must carry the new keys and keep the old ones.
testCase("every locale has the new tts keys and keeps the old ones", () => {
  const localesDir = fileURLToPath(new URL("../i18n/locales", import.meta.url));
  const required = [
    "voiceListDescription",
    "voiceDefaultOption",
    "voiceNotInstalled",
  ];
  const kept = ["voiceDescription", "voicePlaceholder"];
  const localeDirs = readdirSync(localesDir).filter((entry) =>
    statSync(`${localesDir}/${entry}`).isDirectory(),
  );
  assert.ok(localeDirs.length > 0, "no locale directories found");
  for (const locale of localeDirs) {
    const file = `${localesDir}/${locale}/translation.json`;
    const json = JSON.parse(readFileSync(file, "utf8")) as {
      settings?: { advanced?: { tts?: Record<string, unknown> } };
    };
    const tts = json.settings?.advanced?.tts;
    assert.ok(tts, `${locale}: settings.advanced.tts missing`);
    for (const key of required) {
      const value = tts[key];
      assert.equal(typeof value, "string", `${locale}: ${key} missing`);
      assert.notEqual(
        (value as string).trim(),
        "",
        `${locale}: ${key} is empty`,
      );
    }
    for (const key of kept) {
      assert.ok(key in tts, `${locale}: ${key} was removed`);
    }
  }
});

if (failures.length > 0) {
  console.error(`ttsVoices.test.ts: ${failures.length} case(s) failed`);
  for (const failure of failures) {
    console.error(`- ${failure}`);
  }
  process.exit(1);
}

console.log("ttsVoices.test.ts: all assertions passed");
