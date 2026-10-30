import React, { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { Search, X } from "lucide-react";
import {
  SEARCH_ENTRIES,
  SECTION_LABEL_KEY,
  SECTION_ORDER,
} from "./settingsSearchIndex";
import { matchRange, resolveTarget, search } from "./settingsSearch";
import { cancelReveal, revealSetting } from "./settingsReveal";
import type {
  Resolve,
  SearchContext,
  SearchEntry,
  SearchHit,
  SearchPlatform,
  SearchSection,
} from "./types";

const LISTBOX_ID = "settings-search-listbox";
const OPTION_ID_PREFIX = "settings-search-option-";
const PATH_SEPARATOR = " › ";

/**
 * IME guard (WebKit order): some engines deliver the Enter keydown that ends a
 * composition just after compositionend. Enter within this window is ignored.
 */
const COMPOSITION_ENTER_GUARD_MS = 50;

export interface SettingsSearchBoxProps {
  isSectionEnabled(section: SearchSection): boolean;
  /** Experimental switch state; rows with requiresExperimental are locked while false. */
  experimentalEnabled: boolean;
  platform?: SearchPlatform;
  onNavigate(section: SearchSection): void;
  defaultQuery?: string;
}

const optionIdFor = (entry: SearchEntry): string =>
  `${OPTION_ID_PREFIX}${entry.id}`;

/** Same OS call GeneralSettings uses. Falls back when not running inside Tauri. */
function detectPlatform(): SearchPlatform {
  try {
    const osName = type();
    if (osName === "macos" || osName === "windows" || osName === "linux") {
      return osName;
    }
  } catch {
    // Not running inside Tauri (plain browser or a server-side render).
  }
  return "windows";
}

/** Splits the display label at code-point offsets and wraps the match in <mark>. */
function renderLabel(label: string, query: string): React.ReactNode {
  const range = matchRange(label, query);
  if (range === null) return label;
  const chars = Array.from(label);
  return (
    <>
      {chars.slice(0, range.start).join("")}
      <mark className="rounded-sm bg-accent-a/25 font-semibold text-inherit">
        {chars.slice(range.start, range.end).join("")}
      </mark>
      {chars.slice(range.end).join("")}
    </>
  );
}

export const SettingsSearchBox: React.FC<SettingsSearchBoxProps> = ({
  isSectionEnabled,
  experimentalEnabled,
  platform: platformProp,
  onNavigate,
  defaultQuery = "",
}) => {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const platform = useMemo(
    () => platformProp ?? detectPlatform(),
    [platformProp],
  );

  const [query, setQuery] = useState(defaultQuery);
  const [dismissed, setDismissed] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");

  const inputRef = useRef<HTMLInputElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const composingRef = useRef(false);
  const compositionEndedAtRef = useRef(0);
  const lastLanguageRef = useRef(language);

  // Gate state as a string key, so the memoised results only change when a
  // section actually flips (the caller's predicate identity changes each render).
  const gateKey = `${SECTION_ORDER.filter((section) =>
    isSectionEnabled(section),
  ).join(",")}|experimental:${experimentalEnabled ? "on" : "off"}`;

  const ctx = useMemo<SearchContext>(() => {
    const enabled = new Set<string>(gateKey.split("|")[0].split(","));
    return {
      isSectionEnabled: (section) => enabled.has(section),
      experimentalEnabled,
      platform,
    };
  }, [gateKey, experimentalEnabled, platform]);

  const englishT = useMemo(() => i18n.getFixedT("en"), [i18n, language]);

  const resolve = useMemo<Resolve>(
    () => (key) => {
      const local = t(key);
      const english = englishT(key);
      return { local, en: english === key ? local : english };
    },
    [t, englishT],
  );

  const hits = useMemo(
    () => search(SEARCH_ENTRIES, query, resolve, ctx),
    [query, resolve, ctx],
  );

  const hasQuery = query.trim() !== "";
  const isOpen = hasQuery && !dismissed;

  // Same entry stays active while it is still a result; otherwise the first one.
  const activeIndex = hits.findIndex((hit) => hit.entry.id === activeId);
  const effectiveIndex = activeIndex === -1 ? 0 : activeIndex;
  const activeHit: SearchHit | undefined = isOpen
    ? hits[effectiveIndex]
    : undefined;
  const activeOptionDomId =
    activeHit === undefined ? undefined : optionIdFor(activeHit.entry);

  // Keep the active row visible while the user moves through results.
  useEffect(() => {
    if (activeOptionDomId === undefined) return;
    document
      .getElementById(activeOptionDomId)
      ?.scrollIntoView({ block: "nearest" });
  }, [activeOptionDomId]);

  // Clicking anywhere outside the search box closes the popover.
  useEffect(() => {
    if (!isOpen) return;
    const handlePointerDown = (event: PointerEvent): void => {
      const container = containerRef.current;
      if (
        container !== null &&
        event.target instanceof Node &&
        container.contains(event.target)
      ) {
        return;
      }
      setDismissed(true);
    };
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [isOpen]);

  // Cmd+F (macOS) / Ctrl+F (others) focuses the search box.
  useEffect(() => {
    const handleWindowKeyDown = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || composingRef.current) return;
      if (event.shiftKey || event.altKey) return;
      if (event.key.toLowerCase() !== "f") return;
      const modifierHeld = platform === "macos" ? event.metaKey : event.ctrlKey;
      if (!modifierHeld) return;
      event.preventDefault();
      inputRef.current?.focus();
      inputRef.current?.select();
    };
    window.addEventListener("keydown", handleWindowKeyDown);
    return () => window.removeEventListener("keydown", handleWindowKeyDown);
  }, [platform]);

  // A reveal belongs to the language it started in.
  useEffect(() => {
    if (lastLanguageRef.current === language) return;
    lastLanguageRef.current = language;
    cancelReveal();
  }, [language]);

  // Leaving the Settings window must not leave a reveal running.
  useEffect(() => () => cancelReveal(), []);

  const select = (hit: SearchHit): void => {
    const target = resolveTarget(hit, SEARCH_ENTRIES, ctx);
    const label = resolve(target.labelKey).local;
    setQuery("");
    setDismissed(true);
    onNavigate(target.section);
    if (!target.pageOnly) {
      revealSetting({
        section: target.section,
        groupId: target.groupId,
        labelText: label,
        occurrence: target.occurrence,
      });
    }
    setAnnouncement(t("settings.search.showing", { setting: label }));
  };

  const handleKeyDown = (
    event: React.KeyboardEvent<HTMLInputElement>,
  ): void => {
    if (event.key === "Tab") {
      // Closes the popover; focus moves on as usual (no preventDefault).
      setDismissed(true);
      return;
    }
    // While an IME composition is active, these keys belong to the IME.
    if (composingRef.current || event.nativeEvent.isComposing) return;

    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp": {
        if (hits.length === 0) return;
        event.preventDefault();
        if (!isOpen) {
          setDismissed(false);
          return;
        }
        const step = event.key === "ArrowDown" ? 1 : -1;
        const next = (effectiveIndex + step + hits.length) % hits.length;
        setActiveId(hits[next].entry.id);
        return;
      }
      case "Enter": {
        if (
          Date.now() - compositionEndedAtRef.current <=
          COMPOSITION_ENTER_GUARD_MS
        ) {
          return;
        }
        if (activeHit === undefined) return;
        event.preventDefault();
        select(activeHit);
        return;
      }
      case "Escape": {
        if (query !== "") {
          event.preventDefault();
          setQuery("");
          return;
        }
        inputRef.current?.blur();
        return;
      }
      default:
        return;
    }
  };

  const popover = (
    <div className="absolute top-full start-0 mt-1 w-72 z-50 max-h-80 overflow-auto bg-background border border-mid-gray/80 rounded-lg shadow-lg">
      {hits.length === 0 ? (
        <p className="px-3 py-2 text-sm">
          {t("settings.search.noResults", { query })}
        </p>
      ) : (
        <div
          id={LISTBOX_ID}
          role="listbox"
          aria-label={t("settings.search.label")}
        >
          {hits.map((hit) => {
            const { entry } = hit;
            const isActive = entry.id === activeHit?.entry.id;
            const label = resolve(entry.labelKey).local;
            const description =
              entry.descriptionKey === undefined
                ? null
                : resolve(entry.descriptionKey).local;
            let path: string;
            if (hit.locked) {
              const target = resolveTarget(hit, SEARCH_ENTRIES, ctx);
              path = t("settings.search.unlockHint", {
                setting: resolve(target.labelKey).local,
              });
            } else {
              const parts = [t(SECTION_LABEL_KEY[entry.section])];
              if (entry.groupKey !== undefined) {
                parts.push(t(entry.groupKey));
              }
              path = parts.join(PATH_SEPARATOR);
            }
            return (
              <div
                key={entry.id}
                id={optionIdFor(entry)}
                role="option"
                aria-selected={isActive}
                className={`cursor-pointer px-3 py-2 ${isActive ? "bg-mid-gray/15" : ""}`}
                onMouseDown={(event) => event.preventDefault()}
                onMouseEnter={() => setActiveId(entry.id)}
                onClick={() => select(hit)}
              >
                <div className="text-sm font-medium truncate">
                  {renderLabel(label, query)}
                </div>
                {description !== null && (
                  <div className="text-xs opacity-70 truncate">
                    {description}
                  </div>
                )}
                <div className="text-xs opacity-60 truncate">{path}</div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );

  return (
    <div ref={containerRef} className="relative w-full">
      <div className="relative flex items-center">
        <Search
          aria-hidden="true"
          width={14}
          height={14}
          className="pointer-events-none absolute start-2 opacity-60"
        />
        <input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-label={t("settings.search.label")}
          placeholder={t("settings.search.placeholder")}
          aria-expanded={isOpen}
          aria-controls={LISTBOX_ID}
          aria-autocomplete="list"
          aria-activedescendant={
            isOpen && activeOptionDomId !== undefined
              ? activeOptionDomId
              : undefined
          }
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setDismissed(false);
          }}
          onFocus={() => setDismissed(false)}
          onBlur={() => setDismissed(true)}
          onKeyDown={handleKeyDown}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            compositionEndedAtRef.current = Date.now();
          }}
          className={`w-full ps-7 ${hasQuery ? "pe-7" : "pe-2"} py-1.5 text-sm rounded-lg border border-mid-gray/20 bg-background`}
        />
        {hasQuery && (
          <button
            type="button"
            aria-label={t("settings.search.clear")}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => {
              setQuery("");
              inputRef.current?.focus();
            }}
            className="absolute end-1 p-1 rounded opacity-60 hover:opacity-100"
          >
            <X aria-hidden="true" width={12} height={12} />
          </button>
        )}
      </div>
      {isOpen && popover}
      <div aria-live="polite" className="sr-only">
        {announcement}
      </div>
    </div>
  );
};
