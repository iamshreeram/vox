// Server-render checks for the DOM anchors used by settings search:
// SettingContainer roots carry data-setting-title, CollapsibleSettingsGroup
// wrappers carry data-group-id. Written before the attributes exist
// (failing-first).
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CollapsibleSettingsGroup } from "./CollapsibleSettingsGroup";
import { SettingContainer } from "./SettingContainer";

/** The opening tag of the markup's root element (the first tag rendered). */
const rootTag = (markup: string): string => {
  const tag = markup.match(/^<[a-zA-Z][^>]*>/)?.[0];
  assert.ok(tag, `no root tag in markup: ${markup.slice(0, 120)}`);
  return tag;
};

/** Reverses React's attribute escaping for the five characters it encodes. */
const decodeEntities = (value: string): string =>
  value
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");

const titleAttr = (markup: string): string | undefined => {
  const raw = rootTag(markup).match(/\sdata-setting-title="([^"]*)"/)?.[1];
  return raw === undefined ? undefined : decodeEntities(raw);
};

const TITLE = "Keep Mic Open";

const containerMarkup = (
  overrides: Partial<React.ComponentProps<typeof SettingContainer>>,
): string =>
  renderToStaticMarkup(
    <SettingContainer
      title={overrides.title ?? TITLE}
      description="A description"
      {...overrides}
    >
      <span>{"control"}</span>
    </SettingContainer>,
  );

type Mode = "inline" | "tooltip";
type Layout = "horizontal" | "stacked";

const combinations: Array<{
  descriptionMode: Mode;
  grouped: boolean;
  layout: Layout;
  disabled: boolean;
}> = [];
for (const descriptionMode of ["inline", "tooltip"] as const) {
  for (const grouped of [true, false]) {
    for (const layout of ["horizontal", "stacked"] as const) {
      for (const disabled of [false, true]) {
        combinations.push({ descriptionMode, grouped, layout, disabled });
      }
    }
  }
}

const scenarios: Record<string, () => void> = {
  "SettingContainer root carries data-setting-title in every mode/layout/disabled combination":
    () => {
      for (const combo of combinations) {
        const markup = containerMarkup(combo);
        assert.equal(
          titleAttr(markup),
          TITLE,
          `missing or wrong data-setting-title for ${JSON.stringify(combo)}`,
        );
      }
    },

  "data-setting-title is on the root element itself, not a descendant": () => {
    for (const combo of combinations) {
      const markup = containerMarkup(combo);
      assert.equal(
        (markup.match(/data-setting-title=/g) ?? []).length,
        1,
        `expected exactly one anchor for ${JSON.stringify(combo)}`,
      );
      assert.ok(
        rootTag(markup).includes("data-setting-title="),
        `anchor is not on the root tag for ${JSON.stringify(combo)}`,
      );
    }
  },

  "title with quotes, ampersand and <b> is HTML-escaped in the attribute":
    () => {
      const hostile = `Say "hi" & <b>bold</b> it's`;
      const markup = containerMarkup({ title: hostile });
      const tag = rootTag(markup);
      assert.ok(
        !tag.includes("<b>"),
        "raw <b> must not appear in the root tag",
      );
      // Decoding the attribute back must give the exact title, which also
      // proves no raw quote broke out of the attribute value.
      assert.equal(
        titleAttr(markup),
        hostile,
        "decoded attribute must equal title",
      );
      assert.ok(
        tag.includes("&quot;hi&quot;") && tag.includes("&lt;b&gt;"),
        "quotes and angle brackets must be entity-encoded",
      );
    },

  "CollapsibleSettingsGroup wrapper carries data-group-id and still renders its title":
    () => {
      const markup = renderToStaticMarkup(
        <CollapsibleSettingsGroup id="engine-group" title="Engine card">
          <span>{"child"}</span>
        </CollapsibleSettingsGroup>,
      );
      assert.ok(
        rootTag(markup).includes('data-group-id="engine-group"'),
        `wrapper root tag lacks data-group-id: ${rootTag(markup)}`,
      );
      assert.ok(
        markup.includes("Engine card"),
        "group title must still render",
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
  throw new Error(`${failed} setting anchor scenarios failed`);
}
console.log(
  `setting anchors: ${Object.keys(scenarios).length} scenarios passed`,
);
