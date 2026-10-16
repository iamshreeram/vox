import type { DynamicFamily, SearchEntry, SearchSection } from "./types";
import { CORE_DYNAMIC, CORE_ENTRIES, CORE_EXCLUDED } from "./index/core";
import {
  EXPERIMENTAL_DYNAMIC,
  EXPERIMENTAL_ENTRIES,
  EXPERIMENTAL_EXCLUDED,
} from "./index/experimental";

/** Same order as the sidebar. Used for stable ordering of equal-score results. */
export const SECTION_ORDER: readonly SearchSection[] = [
  "general",
  "history",
  "models",
  "advanced",
  "postprocessing",
  "debug",
  "experimental",
  "about",
];

/** Translation key of each page's sidebar label (shown in result paths, also searchable). */
export const SECTION_LABEL_KEY: Readonly<Record<SearchSection, string>> = {
  general: "sidebar.general",
  history: "sidebar.history",
  models: "sidebar.models",
  advanced: "sidebar.advanced",
  postprocessing: "sidebar.postProcessing",
  debug: "sidebar.debug",
  experimental: "sidebar.experimental",
  about: "sidebar.about",
};

const rank = (section: SearchSection): number => SECTION_ORDER.indexOf(section);

export const SEARCH_ENTRIES: readonly SearchEntry[] = [
  ...CORE_ENTRIES,
  ...EXPERIMENTAL_ENTRIES,
]
  .map((entry, index) => ({ entry, index }))
  .sort(
    (a, b) =>
      rank(a.entry.section) - rank(b.entry.section) || a.index - b.index,
  )
  .map(({ entry }) => entry);

/** Translation keys deliberately not searchable, each with a real reason. */
export const EXCLUDED_KEYS: Readonly<Record<string, string>> = {
  ...CORE_EXCLUDED,
  ...EXPERIMENTAL_EXCLUDED,
};

export const DYNAMIC_FAMILIES: readonly DynamicFamily[] = [
  ...CORE_DYNAMIC,
  ...EXPERIMENTAL_DYNAMIC,
];
