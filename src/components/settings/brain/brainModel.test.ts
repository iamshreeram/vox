import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  BRAIN_PRESETS,
  BRAIN_QUICK_FILLS,
  configState,
  destinationKey,
  destinationLabel,
  featureView,
  isLoopbackUrl,
  testKindKey,
  urlProblem,
  type BrainSettingsView,
} from "./brainModel";

// P3b spec (DESIGN.md Rev 6): the pure logic behind the Brain page. i18n-free and React-free,
// mirrors the Rust resolver (src-tauri/src/managers/brain.rs) closely enough that the page
// never offers an action the backend would refuse.
const read = (relative: string): string =>
  readFileSync(new URL(relative, import.meta.url), "utf8");

const base: BrainSettingsView = {
  brain_provider_id: "custom",
  brain_base_url: "http://127.0.0.1:11434/v1",
  brain_model: "llama3",
  brain_token_set: false,
  brain_schema_v1: true,
  brain_source_composer: "inherit",
  brain_source_realtime: "inherit",
  brain_source_postprocess: "inherit",
  brain_test_ack_host: "",
  brain_transcript_consent_dest: "",
};
const s = (over: Partial<BrainSettingsView>): BrainSettingsView => ({
  ...base,
  ...over,
});

// presets: exactly the backend whitelist, https except custom
assert.deepEqual(
  BRAIN_PRESETS.map((p) => p.id),
  ["openai", "openrouter", "groq", "cerebras", "zai", "custom"],
);
for (const p of BRAIN_PRESETS) {
  if (p.id === "custom") assert.equal(p.baseUrl, "");
  else assert.equal(urlProblem(p.baseUrl), null, p.id);
}
assert.deepEqual(
  BRAIN_QUICK_FILLS.map((q) => [q.id, q.baseUrl]),
  [
    ["ollama", "http://127.0.0.1:11434/v1"],
    ["lmstudio", "http://127.0.0.1:1234/v1"],
  ],
);
for (const q of BRAIN_QUICK_FILLS) assert.equal(urlProblem(q.baseUrl), null);

// urls
for (const ok of [
  "https://api.openai.com/v1",
  "http://localhost:11434/v1",
  "http://127.0.0.1:1234/v1",
  "http://[::1]:11434/v1",
]) {
  assert.equal(urlProblem(ok), null, ok);
}
const problems: Array<[string, string]> = [
  ["", "empty"],
  ["   ", "empty"],
  ["ftp://x/v1", "scheme"],
  ["javascript:alert(1)", "scheme"],
  ["http://example.com/v1", "plain_http_remote"],
  ["http://192.168.1.10:11434/v1", "plain_http_remote"],
  ["https://user:pw@example.com/v1", "userinfo"],
  ["http://user@127.0.0.1/v1", "userinfo"],
  ["not a url", "invalid"],
];
for (const [url, why] of problems) assert.equal(urlProblem(url), why, url);
assert.equal(isLoopbackUrl("http://localhost:1/v1"), true);
assert.equal(isLoopbackUrl("http://127.0.0.1:1/v1"), true);
assert.equal(isLoopbackUrl("http://[::1]:1/v1"), true);
assert.equal(isLoopbackUrl("https://localhost.evil.com/v1"), false);
assert.equal(isLoopbackUrl("http://127.0.0.1.evil.com/v1"), false);
assert.equal(isLoopbackUrl("garbage"), false);

// destination label never carries credentials or query
assert.equal(destinationLabel("https://api.openai.com/v1"), "api.openai.com");
assert.equal(destinationLabel("http://127.0.0.1:11434/v1"), "127.0.0.1:11434");
assert.equal(destinationLabel("https://u:p@h.com:8443/v1?key=abc"), "h.com:8443");
assert.equal(destinationLabel("garbage"), "");

// destination key mirrors Rust `destination()`
assert.equal(
  destinationKey("custom", "HTTP://LocalHost:11434/v1/"),
  destinationKey("custom", "http://localhost:11434/v1"),
);
assert.notEqual(
  destinationKey("custom", "http://localhost:11434/v1"),
  destinationKey("openai", "http://localhost:11434/v1"),
);
assert.equal(
  destinationKey("custom", "http://localhost:11434/v1"),
  "custom|http://localhost:11434/v1",
);
assert.ok(!destinationKey("custom", "https://u:p@h.com/v1?key=abc").includes("abc"));

// configState
assert.equal(configState(s({ brain_provider_id: "" })), "unconfigured");
assert.equal(configState(s({ brain_provider_id: "anthropic" })), "unconfigured");
assert.equal(configState(s({ brain_base_url: "" })), "unconfigured");
assert.equal(configState(s({ brain_base_url: "ftp://x" })), "unconfigured");
assert.equal(configState(s({ brain_model: "  " })), "unconfigured");
assert.equal(configState(s({})), "ready", "loopback needs no token");
assert.equal(
  configState(s({ brain_base_url: "https://api.example.com/v1", brain_provider_id: "openai" })),
  "needs_token",
);
assert.equal(
  configState(
    s({
      brain_base_url: "https://api.example.com/v1",
      brain_provider_id: "openai",
      brain_token_set: true,
    }),
  ),
  "ready",
);

// featureView
const dest = destinationKey("custom", "http://127.0.0.1:11434/v1");
const consented = s({ brain_transcript_consent_dest: dest });
assert.deepEqual(featureView(consented, "composer"), {
  source: "inherit",
  state: "inheriting",
  canUseBrain: true,
});
assert.equal(featureView(s({}), "composer").state, "needs_consent");
assert.equal(
  featureView(s({ brain_source_composer: "own", brain_source_postprocess: "own" }), "post_process").state,
  "own",
);
assert.equal(featureView(s({ brain_model: "" }), "realtime_stream").state, "needs_setup");
assert.equal(featureView(s({ brain_model: "" }), "realtime_stream").canUseBrain, false);
// Without the migration marker the backend resolves everything to Own; the page must say so.
const legacy = featureView(s({ brain_schema_v1: false, ...{ brain_transcript_consent_dest: dest } }), "composer");
assert.equal(legacy.state, "own");
assert.equal(legacy.source, "own");
// A different destination invalidates consent.
assert.equal(
  featureView(s({ brain_transcript_consent_dest: destinationKey("custom", "http://127.0.0.1:9/v1") }), "composer").state,
  "needs_consent",
);
// Features are independent.
const mixed = s({ brain_transcript_consent_dest: dest, brain_source_composer: "own" });
assert.equal(featureView(mixed, "composer").state, "own");
assert.equal(featureView(mixed, "realtime_stream").state, "inheriting");

// test result keys
for (const kind of ["Ok", "Unauthorized", "NotFound", "Unreachable", "Redirect", "BadResponse", "Timeout", "InvalidUrl"] as const) {
  assert.ok(testKindKey(kind).startsWith("settings.brain.test."), kind);
}
assert.equal(testKindKey("NotFound"), "settings.brain.test.not_found");
assert.equal(testKindKey("InvalidUrl"), "settings.brain.test.invalid_url");

// the en locale carries every key the model can return, and no kind is left without copy
const en = JSON.parse(read("../../../i18n/locales/en/translation.json")) as {
  settings: { brain: { test: Record<string, string> } };
};
for (const kind of ["ok", "unauthorized", "not_found", "unreachable", "redirect", "bad_response", "timeout", "invalid_url"]) {
  assert.ok(en.settings.brain.test[kind]?.length > 0, `en copy for ${kind}`);
  assert.ok(!/sk-|bearer|token value/i.test(en.settings.brain.test[kind]), "copy never echoes secrets");
}

// the model module stays pure
const model = read("./brainModel.ts");
assert.ok(!/from "react"|useTranslation|commands\./.test(model), "brainModel is pure");

console.log("brain model: ok");
