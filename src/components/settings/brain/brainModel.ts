// Pure logic for the Brain page. No React, no i18n, no Tauri calls, so it runs under plain
// Bun (brainModel.test.ts). It mirrors the Rust resolver (src-tauri/src/managers/brain.rs)
// closely enough that the page never offers an action the backend would refuse.
import type {
  AppSettings,
  BrainFeature,
  BrainSource,
  BrainTestKind,
} from "@/bindings";

export type BrainProviderId =
  | "openai"
  | "openrouter"
  | "groq"
  | "cerebras"
  | "zai"
  | "custom";

/** The subset of AppSettings the Brain page reads. */
export type BrainSettingsView = Pick<
  AppSettings,
  | "brain_provider_id"
  | "brain_base_url"
  | "brain_model"
  | "brain_token_set"
  | "brain_schema_v1"
  | "brain_source_composer"
  | "brain_source_realtime"
  | "brain_source_postprocess"
  | "brain_test_ack_host"
  | "brain_transcript_consent_dest"
>;

export type UrlProblem =
  | "empty"
  | "invalid"
  | "scheme"
  | "userinfo"
  | "plain_http_remote";

export type BrainConfigState = "unconfigured" | "needs_token" | "ready";

export type BrainFeatureState =
  | "needs_setup"
  | "inheriting"
  | "own"
  | "needs_consent";

export interface BrainFeatureView {
  source: BrainSource;
  state: BrainFeatureState;
  canUseBrain: boolean;
}

/** The exact prompt the connection test sends. Never localised, never user text. */
export const BRAIN_TEST_PROMPT = "Reply with OK.";

/** The services the backend accepts as a Brain provider (SUPPORTED_PROVIDER_IDS). */
export const BRAIN_PRESETS: ReadonlyArray<{
  id: BrainProviderId;
  baseUrl: string;
}> = [
  { id: "openai", baseUrl: "https://api.openai.com/v1" },
  { id: "openrouter", baseUrl: "https://openrouter.ai/api/v1" },
  { id: "groq", baseUrl: "https://api.groq.com/openai/v1" },
  { id: "cerebras", baseUrl: "https://api.cerebras.ai/v1" },
  { id: "zai", baseUrl: "https://api.z.ai/api/paas/v4" },
  { id: "custom", baseUrl: "" },
];

/** Local servers on this computer. They need no token. */
export const BRAIN_QUICK_FILLS: ReadonlyArray<{
  id: "ollama" | "lmstudio";
  baseUrl: string;
}> = [
  { id: "ollama", baseUrl: "http://127.0.0.1:11434/v1" },
  { id: "lmstudio", baseUrl: "http://127.0.0.1:1234/v1" },
];

const SOURCE_FIELD: Record<BrainFeature, keyof BrainSettingsView> = {
  composer: "brain_source_composer",
  realtime_stream: "brain_source_realtime",
  post_process: "brain_source_postprocess",
};

const LOOPBACK_IPV4 = /^127(\.\d{1,3}){3}$/;

/** Picks the Brain fields out of the full settings object. */
export function brainViewOf(settings: AppSettings): BrainSettingsView {
  return {
    brain_provider_id: settings.brain_provider_id,
    brain_base_url: settings.brain_base_url,
    brain_model: settings.brain_model,
    brain_token_set: settings.brain_token_set,
    brain_schema_v1: settings.brain_schema_v1,
    brain_source_composer: settings.brain_source_composer,
    brain_source_realtime: settings.brain_source_realtime,
    brain_source_postprocess: settings.brain_source_postprocess,
    brain_test_ack_host: settings.brain_test_ack_host,
    brain_transcript_consent_dest: settings.brain_transcript_consent_dest,
  };
}

/** True when the URL's host is this computer (localhost, 127.0.0.0/8 or ::1). */
export function isLoopbackUrl(raw: string): boolean {
  try {
    const host = new URL(raw.trim()).hostname;
    return host === "localhost" || host === "[::1]" || LOOPBACK_IPV4.test(host);
  } catch {
    return false;
  }
}

/**
 * Why an endpoint address is not allowed, or null when it is. Same rules as
 * validate_base_url in Rust: https anywhere, http for loopback only, no userinfo.
 */
export function urlProblem(raw: string): UrlProblem | null {
  const trimmed = raw.trim();
  if (trimmed === "") return "empty";
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return "invalid";
  }
  if (url.username !== "" || url.password !== "") return "userinfo";
  if (url.protocol === "https:") return null;
  if (url.protocol === "http:") {
    return isLoopbackUrl(trimmed) ? null : "plain_http_remote";
  }
  return "scheme";
}

/** The host (and port when not the default), for display. Never credentials or query. */
export function destinationLabel(raw: string): string {
  try {
    return new URL(raw.trim()).host;
  } catch {
    return "";
  }
}

/**
 * `provider|scheme://host[:port]path`, mirroring Rust `destination()`: the path loses its
 * trailing slash and userinfo, query and fragment are dropped.
 */
export function destinationKey(providerId: string, raw: string): string {
  try {
    const url = new URL(raw.trim());
    const path = url.pathname.replace(/\/+$/, "");
    return `${providerId}|${url.protocol}//${url.host}${path}`;
  } catch {
    return `${providerId}|invalid`;
  }
}

function isPreset(providerId: string): boolean {
  return BRAIN_PRESETS.some((preset) => preset.id === providerId);
}

/** Whether the connection can be used at all, and what is missing. */
export function configState(s: BrainSettingsView): BrainConfigState {
  if (!isPreset(s.brain_provider_id ?? "")) return "unconfigured";
  const baseUrl = s.brain_base_url ?? "";
  if (urlProblem(baseUrl) !== null) return "unconfigured";
  if ((s.brain_model ?? "").trim() === "") return "unconfigured";
  if (!isLoopbackUrl(baseUrl) && !s.brain_token_set) return "needs_token";
  return "ready";
}

/**
 * Model listing needs a usable endpoint and token, but not a model (the model is what the
 * listing is for).
 */
export function canFetchModels(s: BrainSettingsView): boolean {
  return configState({ ...s, brain_model: "x" }) === "ready";
}

/** What one feature gets from the Brain, and whether the page may offer "Use Brain". */
export function featureView(
  s: BrainSettingsView,
  feature: BrainFeature,
): BrainFeatureView {
  const connected = configState(s) === "ready";
  if (s.brain_schema_v1 !== true) {
    // Without the migration marker the backend resolves every feature to its own settings.
    return { source: "own", state: "own", canUseBrain: connected };
  }
  const source: BrainSource =
    s[SOURCE_FIELD[feature]] === "inherit" ? "inherit" : "own";
  if (source === "own") {
    return { source, state: "own", canUseBrain: connected };
  }
  if (!connected) {
    return { source, state: "needs_setup", canUseBrain: false };
  }
  const destination = destinationKey(
    s.brain_provider_id ?? "",
    s.brain_base_url ?? "",
  );
  if (s.brain_transcript_consent_dest !== destination) {
    return { source, state: "needs_consent", canUseBrain: true };
  }
  return { source, state: "inheriting", canUseBrain: true };
}

/** Translation key of the copy for one connection test outcome. */
export function testKindKey(kind: BrainTestKind): string {
  switch (kind) {
    case "Ok":
      return "settings.brain.test.ok";
    case "Unauthorized":
      return "settings.brain.test.unauthorized";
    case "NotFound":
      return "settings.brain.test.not_found";
    case "Unreachable":
      return "settings.brain.test.unreachable";
    case "Redirect":
      return "settings.brain.test.redirect";
    case "BadResponse":
      return "settings.brain.test.bad_response";
    case "Timeout":
      return "settings.brain.test.timeout";
    case "InvalidUrl":
      return "settings.brain.test.invalid_url";
  }
}
