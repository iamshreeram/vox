import assert from "node:assert/strict";
import {
  RT_BOOL_FIELD,
  RT_LIMITS,
  RT_TEXT_FIELD,
  controlDisabled,
  earlySpeechUsable,
  endpointStatus,
  tokenSummary,
  validateText,
  type RtBoolKey,
} from "./realtimeForm";

let passed = 0;
function scenario(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`ok - ${name}`);
}

const ALL: RtBoolKey[] = [
  "enabled",
  "direct_endpoint",
  "stream_reply",
  "early_speech",
  "fast_voice",
  "ack",
  "prewarm",
  "auto_proxy",
  "proxy_enabled",
  "timing_log",
];

scenario("field names match the backend settings fields", () => {
  assert.deepEqual(Object.keys(RT_BOOL_FIELD).sort(), [...ALL].sort());
  for (const k of ALL) assert.equal(RT_BOOL_FIELD[k], `exp_rt_${k}`);
  assert.deepEqual(RT_TEXT_FIELD, {
    endpoint_url: "exp_rt_endpoint_url",
    model: "exp_rt_model",
    token: "exp_rt_token",
    proxy_url: "exp_rt_proxy_url",
  });
  assert.deepEqual(RT_LIMITS, { url: 2048, model: 128, token: 8192 });
});

scenario("only the master stays enabled while it is off", () => {
  for (const k of ALL) {
    assert.equal(controlDisabled(k, true), false, `${k} with master on`);
    assert.equal(controlDisabled(k, false), k !== "enabled", `${k} with master off`);
  }
});

scenario("early speech needs the master and streaming", () => {
  assert.equal(earlySpeechUsable(true, true), true);
  assert.equal(earlySpeechUsable(true, false), false);
  assert.equal(earlySpeechUsable(false, true), false);
  assert.equal(earlySpeechUsable(false, false), false);
});

scenario("token summary never reveals the token", () => {
  assert.deepEqual(tokenSummary(""), { hasToken: false, label: "none" });
  const secret = "eyJhbGciOi.super-secret.value";
  const s = tokenSummary(secret);
  assert.equal(s.hasToken, true);
  assert.ok(!s.label.includes("secret") && !s.label.includes("eyJ"));
  assert.ok(!s.label.includes(secret.slice(-4)));
  assert.equal(s.label, `set (${secret.length} characters)`);
  assert.equal(tokenSummary("   ").hasToken, false, "whitespace only is not a token");
});

scenario("endpoint status mirrors the backend preflight order", () => {
  const base = { master: true, direct: true, url: "", model: "", token: "" };
  assert.equal(endpointStatus({ ...base, master: false }), "off");
  assert.equal(endpointStatus({ ...base, direct: false }), "off");
  assert.equal(endpointStatus(base), "missing_endpoint");
  assert.equal(endpointStatus({ ...base, url: "https://gw.example.com" }), "missing_model");
  assert.equal(
    endpointStatus({ ...base, url: "https://gw.example.com", model: "m" }),
    "missing_token",
  );
  const ready = { ...base, url: "https://gw.example.com", model: "m", token: "t" };
  assert.equal(endpointStatus(ready), "ok");
  assert.equal(endpointStatus({ ...ready, url: "ftp://gw.example.com" }), "invalid_endpoint");
  assert.equal(endpointStatus({ ...ready, url: "https://u:p@gw.example.com" }), "invalid_endpoint");
  assert.equal(endpointStatus({ ...ready, url: "http://gw.example.com" }), "needs_https");
  assert.equal(endpointStatus({ ...ready, url: "http://127.0.0.1:8080" }), "ok");
  assert.equal(endpointStatus({ ...ready, url: "http://localhost:8080/a" }), "ok");
  assert.equal(endpointStatus({ ...ready, url: "http://[::1]:8080" }), "ok");
});

scenario("validateText trims and accepts good values", () => {
  assert.deepEqual(validateText("model", "  some-model "), { ok: true, value: "some-model" });
  assert.deepEqual(validateText("endpoint_url", " https://gw.example.com/anthropic/ "), {
    ok: true,
    value: "https://gw.example.com/anthropic",
  });
  assert.deepEqual(validateText("token", " abc.def-123 "), { ok: true, value: "abc.def-123" });
  assert.deepEqual(validateText("proxy_url", "http://proxy.example:8080"), {
    ok: true,
    value: "http://proxy.example:8080",
  });
  for (const k of ["endpoint_url", "model", "token", "proxy_url"] as const) {
    assert.deepEqual(validateText(k, "   "), { ok: true, value: "" }, `${k} empty clears`);
  }
  assert.deepEqual(validateText("endpoint_url", "http://127.0.0.1:9/x"), {
    ok: true,
    value: "http://127.0.0.1:9/x",
  });
});

scenario("validateText rejects bad values with stable reasons", () => {
  const bad = (k: Parameters<typeof validateText>[0], v: string) => {
    const r = validateText(k, v);
    assert.equal(r.ok, false, `${k} ${JSON.stringify(v)} should be rejected`);
    return r.ok ? "" : r.reason;
  };
  assert.equal(bad("endpoint_url", "ftp://x.example.com"), "invalid");
  assert.equal(bad("endpoint_url", "https://u:p@x.example.com"), "invalid");
  assert.equal(bad("endpoint_url", "https://x.example.com/?a=1"), "invalid");
  assert.equal(bad("endpoint_url", "https://x.example.com/#f"), "invalid");
  assert.equal(bad("endpoint_url", "http://x.example.com"), "needs_https");
  assert.equal(bad("endpoint_url", "https://x.example.com/" + "a".repeat(RT_LIMITS.url)), "too_long");
  assert.equal(bad("model", "bad\nmodel"), "invalid");
  assert.equal(bad("model", "m".repeat(RT_LIMITS.model + 1)), "too_long");
  assert.equal(bad("token", "has space"), "invalid");
  assert.equal(bad("token", "tab\there"), "invalid");
  assert.equal(bad("token", "t".repeat(RT_LIMITS.token + 1)), "too_long");
  assert.equal(bad("proxy_url", "garbage"), "invalid");
  assert.equal(bad("proxy_url", "ftp://p.example:1"), "invalid");
  assert.equal(bad("proxy_url", "http://p.example:1/?q=1"), "invalid");
});

console.log(`${passed} scenarios passed`);
