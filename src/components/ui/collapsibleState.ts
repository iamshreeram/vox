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
