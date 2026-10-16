type ReadStore = Pick<Storage, "getItem">;
type WriteStore = Pick<Storage, "setItem">;

export const openStorageKey = (id: string): string =>
  `vox.settings.group.open.${id}`;

const defaultStorage = (): Storage | undefined => {
  try {
    return typeof globalThis.localStorage === "undefined"
      ? undefined
      : globalThis.localStorage;
  } catch {
    return undefined;
  }
};

/** Reads the persisted open state. Anything other than "1"/"0" (or a missing or throwing store) yields defaultOpen. */
export const readOpen = (
  id: string,
  defaultOpen: boolean,
  storage: ReadStore | undefined = defaultStorage(),
): boolean => {
  try {
    const value = storage?.getItem(openStorageKey(id));
    if (value === "1") return true;
    if (value === "0") return false;
    return defaultOpen;
  } catch {
    return defaultOpen;
  }
};

/** Persists the open state as "1"/"0". Never throws. */
export const writeOpen = (
  id: string,
  open: boolean,
  storage: WriteStore | undefined = defaultStorage(),
): void => {
  try {
    storage?.setItem(openStorageKey(id), open ? "1" : "0");
  } catch {
    // Persistence is best-effort; the in-memory state still works.
  }
};

/** Window event that asks mounted groups to open. detail: { id: string }. */
export const GROUP_OPEN_EVENT = "vox:settings-group-open";

/** How long an openGroup request waits for its group to mount. */
export const PENDING_OPEN_TTL_MS = 3000;

/** In-memory open requests (id -> time requested). Independent of storage. */
const pendingOpens = new Map<string, number>();

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null;

const defaultDispatch = (id: string): void => {
  if (typeof window === "undefined" || typeof CustomEvent === "undefined") {
    return;
  }
  window.dispatchEvent(new CustomEvent(GROUP_OPEN_EVENT, { detail: { id } }));
};

/**
 * Requests that the group `id` opens. Records an in-memory pending open,
 * writes the persisted flag (best-effort) and notifies mounted groups.
 * Never throws.
 */
export function openGroup(
  id: string,
  deps: {
    now?: () => number;
    storage?: Pick<Storage, "setItem"> | null;
    dispatch?: (id: string) => void;
  } = {},
): void {
  try {
    const now = deps.now ?? Date.now;
    pendingOpens.set(id, now());
  } catch {
    // Without a timestamp the request cannot be consumed; the event still runs.
  }
  try {
    const storage =
      deps.storage === undefined ? defaultStorage() : deps.storage;
    storage?.setItem(openStorageKey(id), "1");
  } catch {
    // Persistence is best-effort; the open request still works in memory.
  }
  try {
    (deps.dispatch ?? defaultDispatch)(id);
  } catch {
    // A failing listener must not break the caller.
  }
}

/**
 * True at most once per openGroup(id) call, and only while the request is
 * younger than PENDING_OPEN_TTL_MS. Never throws.
 */
export function consumePendingOpen(
  id: string,
  now: () => number = Date.now,
): boolean {
  try {
    const requestedAt = pendingOpens.get(id);
    if (requestedAt === undefined) return false;
    pendingOpens.delete(id);
    return now() - requestedAt <= PENDING_OPEN_TTL_MS;
  } catch {
    return false;
  }
}

/** Returns detail.id when the event carries a non-empty string id, else null. */
export function parseGroupOpenEvent(e: unknown): string | null {
  const detail = isRecord(e) ? e.detail : undefined;
  if (!isRecord(detail)) return null;
  const id = detail.id;
  return typeof id === "string" && id.length > 0 ? id : null;
}
