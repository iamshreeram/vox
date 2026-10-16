// NOTE: this file depends on the real settings search index being complete.
// Results come from the real SEARCH_ENTRIES, so the "acceleration" and locked
// (unlock hint) expectations only hold once every index file has landed.
// Written before SettingsSearchBox.tsx exists (failing-first).
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import translationEn from "@/i18n/locales/en/translation.json";
import { SettingsSearchBox } from "./SettingsSearchBox";
import type { SearchSection } from "./types";

// Mirrors the app's i18n init (src/i18n/index.ts): English only, React escapes.
const testI18n = i18next.createInstance();
await testI18n.use(initReactI18next).init({
  resources: { en: { translation: translationEn } },
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

type Tree = { readonly [key: string]: Tree | string };
const enTree: Tree = translationEn;

/** English text at a dotted key. Throws a clear message when the key is absent. */
function enText(path: string): string {
  let node: Tree | string | undefined = enTree;
  for (const part of path.split(".")) {
    if (typeof node !== "object") {
      node = undefined;
      break;
    }
    node = node[part];
  }
  if (typeof node !== "string") {
    throw new Error(
      `en translation key "${path}" is missing or not a string in src/i18n/locales/en/translation.json (the search index work must add it)`,
    );
  }
  return node;
}

/** Fills {{name}} placeholders the way i18next does with escapeValue false. */
function interpolate(template: string, vars: Record<string, string>): string {
  return template.replace(
    /\{\{\s*(\w+)\s*\}\}/g,
    (_whole, name: string) => vars[name] ?? "",
  );
}

/** Same escaping React applies to text and attribute values. */
const htmlEscape = (s: string): string =>
  s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#x27;");

const attr = (tag: string, name: string): string | undefined =>
  tag.match(new RegExp(`\\s${name}="([^"]*)"`))?.[1];

const comboboxTag = (markup: string): string => {
  const tag = markup.match(/<[a-z]+[^>]*role="combobox"[^>]*>/)?.[0];
  assert.ok(tag, 'no element with role="combobox"');
  return tag;
};

const render = (
  isSectionEnabled: (section: SearchSection) => boolean,
  defaultQuery?: string,
): string =>
  renderToStaticMarkup(
    <I18nextProvider i18n={testI18n}>
      <SettingsSearchBox
        isSectionEnabled={isSectionEnabled}
        platform="macos"
        onNavigate={() => {}}
        defaultQuery={defaultQuery}
      />
    </I18nextProvider>,
  );

const allEnabled = (): boolean => true;
const experimentalDisabled = (section: SearchSection): boolean =>
  section !== "experimental";

const scenarios: Record<string, () => void> = {
  "empty query: closed combobox, no active descendant, no listbox, en placeholder":
    () => {
      const placeholder = enText("settings.search.placeholder");
      const markup = render(allEnabled, "");
      const tag = comboboxTag(markup);
      assert.equal(attr(tag, "aria-expanded"), "false");
      assert.equal(
        markup.includes("aria-activedescendant"),
        false,
        "no aria-activedescendant while closed",
      );
      assert.equal(markup.includes('role="listbox"'), false, "no listbox");
      assert.ok(
        markup.includes(`placeholder="${htmlEscape(placeholder)}"`),
        `placeholder "${placeholder}" not rendered`,
      );
    },

  "no-results query: en noResults text with the query interpolated": () => {
    const template = enText("settings.search.noResults");
    assert.match(
      template,
      /\{\{\s*query\s*\}\}/,
      "noResults template must interpolate {{query}}",
    );
    const expected = interpolate(template, { query: "zzzzqqq" });
    const markup = render(allEnabled, "zzzzqqq");
    assert.ok(
      markup.includes(htmlEscape(expected)),
      `expected no-results text "${expected}"`,
    );
  },

  "acceleration query: open listbox, unique option ids, <mark> emphasis, engine group path":
    () => {
      const engine = enText("settings.experimental.groups.engine");
      const markup = render(allEnabled, "acceleration");
      const tag = comboboxTag(markup);
      assert.equal(attr(tag, "aria-expanded"), "true");
      assert.ok(markup.includes('role="listbox"'), "listbox rendered");

      const optionTags = markup.match(/<[a-z]+[^>]*role="option"[^>]*>/g) ?? [];
      assert.ok(optionTags.length >= 1, "at least one role=option row");
      const ids = optionTags.map((optionTag) => attr(optionTag, "id"));
      for (const id of ids) {
        assert.ok(
          id?.startsWith("settings-search-option-"),
          `option id must start with settings-search-option-, got ${id}`,
        );
      }
      assert.equal(new Set(ids).size, ids.length, "option ids must be unique");
      assert.equal(
        attr(tag, "aria-activedescendant"),
        ids[0],
        "first result is the active option",
      );

      const marks = [...markup.matchAll(/<mark\b[^>]*>([^<]*)<\/mark>/g)].map(
        (m) => m[1] ?? "",
      );
      assert.ok(
        marks.some((text) => text.toLowerCase().includes("acceleration")),
        `matched fragment must be wrapped in <mark>; marks found: ${JSON.stringify(marks)}`,
      );

      assert.ok(
        markup.includes(htmlEscape(engine)),
        `path must contain the en engine group text "${engine}"`,
      );
    },

  "locked results (experimental disabled, advanced enabled) show the unlock hint":
    () => {
      const template = enText("settings.search.unlockHint");
      assert.match(
        template,
        /\{\{\s*setting\s*\}\}/,
        "unlockHint template must interpolate {{setting}}",
      );
      const setting = enText("settings.advanced.experimentalToggle.label");
      const expected = interpolate(template, { setting });
      const markup = render(experimentalDisabled, "acceleration");
      assert.ok(
        markup.includes(htmlEscape(expected)),
        `expected unlock hint "${expected}"`,
      );
    },

  "hostile query is escaped: no raw <img, escaped text present": () => {
    const hostile = "<img src=x onerror=alert(1)>";
    const markup = render(allEnabled, hostile);
    assert.equal(/<img/i.test(markup), false, "raw <img must not be rendered");
    assert.ok(
      markup.includes(htmlEscape(hostile)),
      "escaped hostile text must be present",
    );
  },
};

let failed = 0;
for (const [name, run] of Object.entries(scenarios)) {
  try {
    run();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n  ${(error as Error).message}`);
  }
}
if (failed > 0) {
  throw new Error(`${failed} settings search UI scenarios failed`);
}
console.log(
  `settings search UI: ${Object.keys(scenarios).length} scenarios passed`,
);
