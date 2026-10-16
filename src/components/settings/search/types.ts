/**
 * Contract for Settings search. Frozen: change only with a new DESIGN.md revision.
 * `SearchSection` must stay equal to `SidebarSection` in components/Sidebar.tsx.
 */
export type SearchSection =
  | "general"
  | "history"
  | "models"
  | "advanced"
  | "postprocessing"
  | "debug"
  | "experimental"
  | "about";

export type SearchPlatform = "macos" | "windows" | "linux";

export interface SearchEntry {
  /** Unique and stable: "<section>.<slug>". */
  id: string;
  section: SearchSection;
  /** Translation key of the card title (shown in the result path). */
  groupKey?: string;
  /** CollapsibleSettingsGroup id to open (experimental page only). */
  groupId?: string;
  /** Translation key of the row title. Also the DOM anchor text (SettingContainer title). */
  labelKey: string;
  descriptionKey?: string;
  /** Lowercase English synonyms. Matched, never displayed. */
  keywords?: readonly string[];
  /** Entry id of the setting that makes this section visible. Required for postprocessing and experimental. */
  unlockedBy?: string;
  /**
   * 0-based n-th row carrying this translated title inside the destination scope
   * (section root, or the group card when groupId is set). Default 0.
   */
  occurrence?: number;
  /** Omitted = available on every platform. */
  platforms?: readonly SearchPlatform[];
  /** Navigate to the page only; there is no row to reveal (history and models pages). */
  pageOnly?: boolean;
}

export interface TextPair {
  local: string;
  en: string;
}

export type Resolve = (key: string) => TextPair;

export interface SearchContext {
  isSectionEnabled(section: SearchSection): boolean;
  platform: SearchPlatform;
}

export interface SearchHit {
  entry: SearchEntry;
  score: number;
  locked: boolean;
  /** Entry ids following unlockedBy until a reachable (enabled) section; empty when not locked. */
  unlockChain: readonly string[];
}

/** Code-point offsets into the DISPLAY string; end is exclusive. */
export interface MatchRange {
  start: number;
  end: number;
}

/**
 * A family of translation keys built dynamically (template or variable keys) that the
 * coverage test cannot see literally. key = prefix + id + (suffix ?? "").
 * `ids: []` is allowed only with a `note` (>= 20 chars) explaining why nothing is searchable.
 */
export interface DynamicFamily {
  /** Repo-relative path of the file containing the dynamic t() calls. */
  source: string;
  prefix: string;
  ids: readonly string[];
  suffix?: string;
  note?: string;
}
