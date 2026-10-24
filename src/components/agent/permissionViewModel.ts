// Pure view model for the ACP permission approval dialog (no React, no DOM).
// The payload comes from the `acp-permission-request` event emitted by the backend.
// Title and details are untrusted text supplied by the agent: they are only ever
// rendered as plain text by the component, and this module never builds markup.

/** Payload of the `acp-permission-request` event (mirrors the Rust struct). */
export interface PermissionRequestEvent {
  token: string;
  title: string;
  kind: string;
  details: string | null;
  truncated: boolean;
  timeoutSecs: number;
  queued: number;
}

/** Which dialog control receives initial focus. Always Deny (fail closed). */
export type PermissionDefaultFocus = "deny";

/** Everything the dialog needs to render one approval request. */
export interface PermissionViewModel {
  token: string;
  title: string;
  kindLabelKey: string;
  /** Trimmed details text, or null when there is nothing usable to show. */
  detailsText: string | null;
  /** True when the details are missing or blank and the warning must be shown. */
  showUnavailableWarning: boolean;
  truncated: boolean;
  timeoutSecs: number;
  queued: number;
  defaultFocus: PermissionDefaultFocus;
}

const KIND_LABEL_PREFIX = "settings.advanced.acpPermission.kind.";

/** Kinds that have a dedicated label. Anything else maps to `other`. */
const KNOWN_KINDS: ReadonlySet<string> = new Set([
  "read",
  "edit",
  "delete",
  "move",
  "search",
  "execute",
  "think",
  "fetch",
  "switch_mode",
  "other",
]);

/** Maps a backend tool kind to its i18n key; unknown kinds become `other`. */
export function kindLabelKeyFor(kind: string): string {
  const normalized = KNOWN_KINDS.has(kind) ? kind : "other";
  return `${KIND_LABEL_PREFIX}${normalized}`;
}

/** Builds the render model for one approval request. */
export function permissionViewModel(
  event: PermissionRequestEvent,
): PermissionViewModel {
  const trimmedDetails = event.details?.trim() ?? "";
  const hasDetails = trimmedDetails.length > 0;
  return {
    token: event.token,
    title: event.title.trim(),
    kindLabelKey: kindLabelKeyFor(event.kind),
    detailsText: hasDetails ? trimmedDetails : null,
    showUnavailableWarning: !hasDetails,
    truncated: event.truncated,
    timeoutSecs: event.timeoutSecs,
    queued: event.queued,
    defaultFocus: "deny",
  };
}
