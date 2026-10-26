// Chat-window i18n bootstrap (DESIGN-CHAT A16). Deliberately NOT `@/i18n`: that module calls
// get_app_settings and plugin-os when it is imported, and the chat page may call only
// get_chat_snapshot and clear_chat_log (A8). Instead it bundles every locale the app bundles,
// picks the language from the code the main window stores in localStorage, then from the
// browser language, then English, and follows later changes through the `storage` event.
import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import {
  getLanguageDirection,
  updateDocumentDirection,
  updateDocumentLanguage,
} from "@/lib/utils/rtl";

/** localStorage key the main window writes its current language code to (src/i18n/index.ts). */
export const LANGUAGE_STORAGE_KEY = "vox-ui-language";

// Same resources as the app: every src/i18n/locales/<code>/translation.json.
const localeModules = import.meta.glob<{ default: Record<string, unknown> }>(
  "../i18n/locales/*/translation.json",
  { eager: true },
);

const resources: Record<string, { translation: Record<string, unknown> }> = {};
for (const [path, module] of Object.entries(localeModules)) {
  const code = path.match(/\.\.\/i18n\/locales\/(.+)\/translation\.json/)?.[1];
  if (code) {
    resources[code] = { translation: module.default };
  }
}

/**
 * Same matching rules as getSupportedLanguage in src/i18n/index.ts (kept in step with it;
 * that module is not imported here): exact code first, then the base language, with the
 * Chinese and Cantonese variants mapped to zh-TW or zh.
 */
function matchLanguage(langCode: string | null | undefined): string | null {
  if (!langCode) return null;

  const codes = Object.keys(resources);
  const normalized = langCode.toLowerCase().replace(/_/g, "-");
  const subtags = normalized.split("-");
  const language = subtags[0];
  const isHant = subtags.includes("hant");
  const isHans = subtags.includes("hans");
  const isTraditionalRegion = ["tw", "hk", "mo"].some((region) =>
    subtags.includes(region),
  );

  const exact = codes.find((code) => code.toLowerCase() === normalized);
  if (exact) return exact;

  let fallback = language;
  if (language === "zh" && (isHant || (!isHans && isTraditionalRegion))) {
    fallback = "zh-tw";
  } else if (language === "yue") {
    // Cantonese uses Traditional Chinese unless explicitly tagged as Hans.
    fallback = isHans ? "zh" : "zh-tw";
  }
  return codes.find((code) => code.toLowerCase() === fallback) ?? null;
}

function readStoredLanguage(): string | null {
  try {
    return localStorage.getItem(LANGUAGE_STORAGE_KEY);
  } catch {
    return null;
  }
}

function resolveLanguage(stored: string | null): string {
  return matchLanguage(stored) ?? matchLanguage(navigator.language) ?? "en";
}

function applyDocumentLanguage(lng: string): void {
  updateDocumentDirection(getLanguageDirection(lng));
  updateDocumentLanguage(lng);
}

const initialLanguage = resolveLanguage(readStoredLanguage());

if (!i18n.isInitialized) {
  void i18n.use(initReactI18next).init({
    resources,
    lng: initialLanguage,
    fallbackLng: "en",
    interpolation: { escapeValue: false }, // React already escapes values
    react: { useSuspense: false },
  });
}
applyDocumentLanguage(initialLanguage);

i18n.on("languageChanged", (lng) => {
  applyDocumentLanguage(lng);
});

// The main window writes the new language code to localStorage; follow it live.
window.addEventListener("storage", (event) => {
  if (event.key !== LANGUAGE_STORAGE_KEY) return;
  const next = resolveLanguage(event.newValue);
  if (next !== i18n.language) void i18n.changeLanguage(next);
});

export default i18n;
