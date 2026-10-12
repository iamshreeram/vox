// Pure helpers for the Experimental "Realtime conversation" settings group (design R9).
// SKELETON: bodies throw; realtimeForm.test.ts is the authoritative spec.
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
  void key;
  void master;
  throw new Error("not implemented");
}

/** Early speech additionally needs streaming. */
export function earlySpeechUsable(master: boolean, streamReply: boolean): boolean {
  void master;
  void streamReply;
  throw new Error("not implemented");
}

/** Never reveals the token: a short status string and a flag. */
export function tokenSummary(token: string): { hasToken: boolean; label: string } {
  void token;
  throw new Error("not implemented");
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
  void input;
  throw new Error("not implemented");
}

export type TextCheck = { ok: true; value: string } | { ok: false; reason: string };

/** Trim + validate a text value like the backend setter. reason is a stable code. */
export function validateText(key: RtTextKey, raw: string): TextCheck {
  void key;
  void raw;
  throw new Error("not implemented");
}
