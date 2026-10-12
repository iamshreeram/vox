// Pure helpers for the Experimental "Realtime conversation" settings group (design R9).
// The backend (realtime_config.rs) is authoritative; these mirror its rules only to give
// instant feedback in the UI.

export type RtBoolKey =
  | "enabled"
  | "direct_endpoint"
  | "stream_reply"
  | "early_speech"
  | "fast_voice"
  | "ack"
  | "prewarm"
  | "auto_proxy"
  | "proxy_enabled"
  | "timing_log";

export type RtTextKey = "endpoint_url" | "model" | "token" | "proxy_url";

/** Settings field name for each toggle / text key. */
export const RT_BOOL_FIELD: Record<RtBoolKey, string> = {
  enabled: "exp_rt_enabled",
  direct_endpoint: "exp_rt_direct_endpoint",
  stream_reply: "exp_rt_stream_reply",
  early_speech: "exp_rt_early_speech",
  fast_voice: "exp_rt_fast_voice",
  ack: "exp_rt_ack",
  prewarm: "exp_rt_prewarm",
  auto_proxy: "exp_rt_auto_proxy",
  proxy_enabled: "exp_rt_proxy_enabled",
  timing_log: "exp_rt_timing_log",
};

export const RT_TEXT_FIELD: Record<RtTextKey, string> = {
  endpoint_url: "exp_rt_endpoint_url",
  model: "exp_rt_model",
  token: "exp_rt_token",
  proxy_url: "exp_rt_proxy_url",
};

export const RT_LIMITS = { url: 2048, model: 128, token: 8192 } as const;

/** Every control except the master is disabled while the master is off. */
export function controlDisabled(key: RtBoolKey, master: boolean): boolean {
  return key !== "enabled" && !master;
}

/** Early speech additionally needs streaming. */
export function earlySpeechUsable(
  master: boolean,
  streamReply: boolean,
): boolean {
  return master && streamReply;
}

/** Never reveals the token: a short status string and a flag. */
export function tokenSummary(token: string): {
  hasToken: boolean;
  label: string;
} {
  if (token.trim() === "") {
    return { hasToken: false, label: "none" };
  }
  return { hasToken: true, label: `set (${token.length} characters)` };
}

export type EndpointStatus =
  | "off"
  | "ok"
  | "missing_endpoint"
  | "missing_model"
  | "missing_token"
  | "invalid_endpoint"
  | "needs_https";

/** Mirror of the backend preflight; "off" when the master or direct-endpoint toggle is off. */
export function endpointStatus(input: {
  master: boolean;
  direct: boolean;
  url: string;
  model: string;
  token: string;
}): EndpointStatus {
  if (!input.master || !input.direct) return "off";
  if (input.url.trim() === "") return "missing_endpoint";
  if (input.model.trim() === "") return "missing_model";
  if (input.token.trim() === "") return "missing_token";
  const check = validateText("endpoint_url", input.url);
  if (!check.ok) {
    return check.reason === "needs_https" ? "needs_https" : "invalid_endpoint";
  }
  return "ok";
}

export type TextCheck =
  | { ok: true; value: string }
  | { ok: false; reason: string };

const CONTROL_CHARS = /[\u0000-\u001f\u007f]/;
const WHITESPACE = /\s/;
const LOOPBACK_IPV4 = /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/;

function fail(reason: "invalid" | "needs_https" | "too_long"): TextCheck {
  return { ok: false, reason };
}

function isLoopbackHost(host: string): boolean {
  return host === "localhost" || host === "[::1]" || LOOPBACK_IPV4.test(host);
}

function parseHttpUrl(value: string): URL | null {
  // Query and fragment markers are refused even when empty ("?" / "#").
  if (value.includes("?") || value.includes("#")) return null;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.hostname === "") return null;
  return url;
}

/** Trim + validate a text value like the backend setter. reason is a stable code. */
export function validateText(key: RtTextKey, raw: string): TextCheck {
  const value = raw.trim();
  if (value === "") return { ok: true, value: "" };
  if (CONTROL_CHARS.test(value)) return fail("invalid");
  if (key === "token" && WHITESPACE.test(value)) return fail("invalid");

  const limit =
    key === "model"
      ? RT_LIMITS.model
      : key === "token"
        ? RT_LIMITS.token
        : RT_LIMITS.url;
  if (Array.from(value).length > limit) return fail("too_long");

  if (key === "endpoint_url") {
    const url = parseHttpUrl(value);
    if (url === null) return fail("invalid");
    if (url.username !== "" || url.password !== "") return fail("invalid");
    if (url.protocol === "http:" && !isLoopbackHost(url.hostname)) {
      return fail("needs_https");
    }
    return { ok: true, value: value.replace(/\/+$/, "") };
  }

  if (key === "proxy_url") {
    if (parseHttpUrl(value) === null) return fail("invalid");
    return { ok: true, value };
  }

  return { ok: true, value };
}
