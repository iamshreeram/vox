// Pure UI logic for the TTS engine picker in Spoken Replies settings.
// Spec = ttsEngineState.test.ts (authoritative); DESIGN.md Rev 4, section 3 P6.
// No React, no network, no Tauri imports: type-only imports from bindings.

import type { TtsEngine, TtsEngineInfo, TtsRuntime } from "@/bindings";
import { formatMegabytes } from "./naturalVoiceState";

/** Install state of an engine's model, as reported by the install hook. */
export type EngineInstall =
  | { phase: "not_installed" }
  | { phase: "downloading"; done: number; total: number }
  | { phase: "verifying" }
  | { phase: "ready" }
  | {
      phase: "error";
      kind: "corrupt" | "proxy" | "disk" | "blocked" | "generic";
    };

/** Badges derived only from catalog fields (plus the size label). */
export type EngineBadge =
  | { kind: "recommended" }
  | { kind: "runtime"; runtime: TtsRuntime }
  | { kind: "english_only" }
  | { kind: "size"; sizeBytes: number; label: string }
  | { kind: "license"; license: string };

/** Everything one engine card needs to render. */
export type EngineCard = {
  id: TtsEngine;
  selected: boolean;
  selectable: boolean;
  disabledReason: "unsupported_platform" | null;
  badges: EngineBadge[];
  primaryAction: "install" | "cancel" | "delete" | null;
  showImportFromFolder: boolean;
  usingSystemVoiceNote: boolean;
  showVoiceDropdown: boolean;
  showRateSlider: boolean;
  showRateNote: boolean;
  previewEnabled: boolean;
  error: { kind: string; showImportHint: boolean } | null;
  progress: number | null;
};

/** Error kinds that point the user at the import-from-folder hint. */
const IMPORT_HINT_KINDS = new Set(["corrupt", "proxy", "blocked", "disk"]);

/** Progress as a fraction in [0,1]; null when total is not positive or any input is non-finite. */
function downloadProgress(done: number, total: number): number | null {
  if (!Number.isFinite(done) || !Number.isFinite(total) || total <= 0) {
    return null;
  }
  return Math.min(1, Math.max(0, done / total));
}

function badgesFor(info: TtsEngineInfo): EngineBadge[] {
  const badges: EngineBadge[] = [];
  if (info.recommended && info.supported_on_platform) {
    badges.push({ kind: "recommended" });
  }
  badges.push({ kind: "runtime", runtime: info.runtime });
  if (info.english_only) {
    badges.push({ kind: "english_only" });
  }
  if (info.requires_download) {
    badges.push({
      kind: "size",
      sizeBytes: info.size_bytes,
      label: formatMegabytes(info.size_bytes),
    });
  }
  if (info.license.trim() !== "") {
    badges.push({ kind: "license", license: info.license });
  }
  return badges;
}

function cardFor(
  info: TtsEngineInfo,
  selected: TtsEngine,
  installs: Partial<Record<TtsEngine, EngineInstall>>,
): EngineCard {
  const supported = info.supported_on_platform;
  // An engine with no model (e.g. the OS voice) ignores any install state.
  const install: EngineInstall = info.requires_download
    ? (installs[info.id] ?? { phase: "not_installed" })
    : { phase: "ready" };
  const isSelected = info.id === selected;

  // Unsupported engines get no actions at all; supported ones follow the install phase.
  let primaryAction: EngineCard["primaryAction"] = null;
  let showImportFromFolder = false;
  let progress: number | null = null;
  let error: EngineCard["error"] = null;
  if (supported && info.requires_download) {
    switch (install.phase) {
      case "not_installed":
        primaryAction = "install";
        showImportFromFolder = true;
        break;
      case "downloading":
        primaryAction = "cancel";
        progress = downloadProgress(install.done, install.total);
        break;
      case "verifying":
        break;
      case "ready":
        primaryAction = "delete";
        break;
      case "error":
        primaryAction = "install";
        showImportFromFolder = true;
        error = {
          kind: install.kind,
          showImportHint: IMPORT_HINT_KINDS.has(install.kind),
        };
        break;
    }
  }

  return {
    id: info.id,
    selected: isSelected,
    selectable: supported,
    disabledReason: supported ? null : "unsupported_platform",
    badges: badgesFor(info),
    primaryAction,
    showImportFromFolder,
    usingSystemVoiceNote:
      isSelected &&
      supported &&
      info.requires_download &&
      install.phase !== "ready",
    showVoiceDropdown: info.supports_voice_choice,
    showRateSlider: info.supports_rate,
    showRateNote: !info.supports_rate,
    previewEnabled: supported,
    error,
    progress,
  };
}

/** One card per catalog entry, in catalog order. Never throws on unknown ids or an empty catalog. */
export function buildEngineCards(
  catalog: TtsEngineInfo[],
  selected: TtsEngine,
  installs: Partial<Record<TtsEngine, EngineInstall>>,
): EngineCard[] {
  return catalog.map((info) => cardFor(info, selected, installs));
}

/** A persisted selection is never rewritten, even when the engine is unsupported here. */
export function nextSelectionOnLoad(
  _catalog: TtsEngineInfo[],
  persisted: TtsEngine,
): TtsEngine {
  return persisted;
}

/** False for unknown engines and for engines unsupported on this platform. */
export function canSelect(catalog: TtsEngineInfo[], id: TtsEngine): boolean {
  const info = catalog.find((entry) => entry.id === id);
  return info !== undefined && info.supported_on_platform;
}
