/**
 * Pure rules for the vault RAG settings section (DESIGN Rev 5, 3.7). No React, no Tauri imports.
 */
import type { VaultHitDto, VaultIndexStatusDto } from "@/bindings";

/** Status polling interval while the Experimental page is visible and the toggle is on. */
export const VAULT_RAG_POLL_MS = 2000;

/** Search preview hits shown at most. */
export const MAX_PREVIEW_HITS = 8;

const P = "settings.experimental.vaultRag";

/** Every translation key the vault RAG UI uses. All live under `settings.experimental.vaultRag.`. */
export const VAULT_RAG_I18N_KEYS: readonly string[] = [
  `${P}.title`,
  `${P}.description`,
  `${P}.enable.label`,
  `${P}.enable.description`,
  `${P}.needsVaultPath`,
  `${P}.folders.label`,
  `${P}.folders.description`,
  `${P}.folders.placeholder`,
  `${P}.status.off`,
  `${P}.status.indexing`,
  `${P}.status.ready`,
  `${P}.status.unavailable`,
  `${P}.status.busy`,
  `${P}.status.counts`,
  `${P}.reindex.button`,
  `${P}.probe.button`,
  `${P}.probe.results.supported`,
  `${P}.probe.results.unsupported`,
  `${P}.probe.results.unknown`,
  `${P}.preview.label`,
  `${P}.preview.placeholder`,
  `${P}.preview.empty`,
  `${P}.disclosure.folders`,
  `${P}.disclosure.sentToProvider`,
  `${P}.disclosure.bestEffortRedaction`,
  `${P}.disclosure.notesMayInstruct`,
  `${P}.disclosure.spokenAloud`,
  `${P}.disclosure.toolsRequirement`,
  `${P}.disclosure.unsupportedFallback`,
  `${P}.disclosure.localCache`,
];

const DISCLOSURE_PREFIX = `${P}.disclosure.`;

/** The eight plain-words disclosure keys (DESIGN 3.7). */
export const DISCLOSURE_KEYS: readonly string[] = [
  `${DISCLOSURE_PREFIX}folders`,
  `${DISCLOSURE_PREFIX}sentToProvider`,
  `${DISCLOSURE_PREFIX}bestEffortRedaction`,
  `${DISCLOSURE_PREFIX}notesMayInstruct`,
  `${DISCLOSURE_PREFIX}spokenAloud`,
  `${DISCLOSURE_PREFIX}toolsRequirement`,
  `${DISCLOSURE_PREFIX}unsupportedFallback`,
  `${DISCLOSURE_PREFIX}localCache`,
];

/** The toggle may be enabled only when a vault path is configured (trimmed non-empty). */
export function canEnableVaultRag(vaultPath: string): boolean {
  return vaultPath.trim() !== "";
}

/** Poll only while visible AND enabled AND no request in flight. */
export function shouldPollStatus(i: {
  pageVisible: boolean;
  enabled: boolean;
  requestInFlight: boolean;
}): boolean {
  return i.pageVisible && i.enabled && !i.requestInFlight;
}

/** Folder save outcome: ok keeps the attempted value; error keeps the current value and the backend text. */
export function foldersOutcome(
  current: string,
  attempted: string,
  result: { status: "ok" } | { status: "error"; error: string },
): { value: string; error: string | null } {
  if (result.status === "ok") {
    return { value: attempted, error: null };
  }
  return { value: current, error: result.error };
}

/** Plain-text preview lines `rel > heading: snippet` (heading omitted when empty), at most MAX_PREVIEW_HITS. */
export function previewLines(hits: VaultHitDto[]): string[] {
  return hits
    .slice(0, MAX_PREVIEW_HITS)
    .map((hit) =>
      hit.heading === ""
        ? `${hit.rel}: ${hit.snippet}`
        : `${hit.rel} > ${hit.heading}: ${hit.snippet}`,
    );
}

export type StatusKey = "off" | "indexing" | "ready" | "unavailable" | "busy";

export interface StatusView {
  key: StatusKey;
  notes: number;
  chunks: number;
  skipped: number;
  toolSupport: "untested" | "supported" | "unsupported" | "unknown";
}

const KNOWN_TOOL_SUPPORT: readonly string[] = [
  "untested",
  "supported",
  "unsupported",
  "unknown",
];

/** Maps the toggle and the status DTO to the status line shown in the UI. */
export function statusView(
  enabled: boolean,
  dto: VaultIndexStatusDto | null,
): StatusView {
  const toolSupport: StatusView["toolSupport"] =
    dto === null
      ? "untested"
      : KNOWN_TOOL_SUPPORT.includes(dto.tool_support)
        ? (dto.tool_support as StatusView["toolSupport"])
        : "unknown";
  const counts = {
    notes: dto?.notes ?? 0,
    chunks: dto?.chunks ?? 0,
    skipped: dto?.skipped ?? 0,
  };

  let key: StatusKey;
  if (!enabled) {
    key = "off";
  } else if (dto === null) {
    key = "unavailable";
  } else if (dto.last_error_kind === "busy") {
    // Busy also sets refreshing, so it must be checked before indexing.
    key = "busy";
  } else if (dto.refreshing) {
    key = "indexing";
  } else if (dto.last_error_kind !== null) {
    key = "unavailable";
  } else {
    key = "ready";
  }
  return { key, ...counts, toolSupport };
}

/** Re-index is allowed only when enabled, a status is known, and nothing is refreshing. */
export function canReindex(
  enabled: boolean,
  dto: VaultIndexStatusDto | null,
): boolean {
  return enabled && dto !== null && !dto.refreshing;
}

/** Maps the probe result to a kind and the text to show. */
export function probeMessage(
  r: { status: "ok"; data: string } | { status: "error"; error: string },
): {
  kind: "supported" | "unsupported" | "unknown" | "error";
  text: string | null;
} {
  if (r.status === "error") {
    return { kind: "error", text: r.error };
  }
  if (r.data === "supported" || r.data === "unsupported") {
    return { kind: r.data, text: null };
  }
  return { kind: "unknown", text: null };
}

export interface PollerDeps {
  run: () => Promise<void>;
  schedule: (fn: () => void, ms: number) => unknown;
  cancel: (handle: unknown) => void;
  intervalMs: number;
}

/**
 * Non-overlapping poller: the next tick is scheduled only after run() settles.
 * - start() while stopped sets running and runs at once only if no run is in flight;
 *   otherwise the in-flight completion schedules the next tick.
 * - stop() cancels the pending timer; an in-flight completion then schedules nothing.
 */
export function createStatusPoller(d: PollerDeps): {
  start(): void;
  stop(): void;
  running(): boolean;
} {
  let isRunning = false;
  let inFlight = false;
  let timer: { handle: unknown } | null = null;

  const settle = (): void => {
    inFlight = false;
    if (isRunning) schedule();
  };

  const fire = (): void => {
    inFlight = true;
    let pending: Promise<void>;
    try {
      pending = d.run();
    } catch {
      pending = Promise.resolve();
    }
    pending.then(settle, settle);
  };

  const schedule = (): void => {
    const handle = d.schedule(tick, d.intervalMs);
    timer = { handle };
  };

  const tick = (): void => {
    timer = null;
    if (isRunning && !inFlight) fire();
  };

  return {
    start(): void {
      if (isRunning) return;
      isRunning = true;
      if (!inFlight) fire();
    },
    stop(): void {
      isRunning = false;
      if (timer !== null) {
        d.cancel(timer.handle);
        timer = null;
      }
    },
    running(): boolean {
      return isRunning;
    },
  };
}

/**
 * The folder list as the backend stores it: split on ",", trim, drop empty names,
 * drop repeats (first kept), join with ",". Same rules as parse_rag_folders in
 * settings.rs without its validation; the backend stays the authority.
 */
export function normalizeFolders(raw: string): string {
  const kept: string[] = [];
  for (const part of raw.split(",")) {
    const name = part.trim();
    if (name !== "" && !kept.includes(name)) {
      kept.push(name);
    }
  }
  return kept.join(",");
}
