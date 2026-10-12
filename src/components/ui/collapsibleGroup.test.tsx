// Initial-markup checks only (server render): these prove the closed/open
// markup, that children stay in the tree in BOTH states, and the context the
// group provides. They do not exercise effects or transitions; the
// collapse/reopen + tooltip sequence is a documented manual check.
import assert from "node:assert/strict";
import React, { useContext } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CollapsibleSettingsGroup } from "./CollapsibleSettingsGroup";
import {
  SettingsGroupContext,
  groupedRowClass,
  shouldShowTooltip,
} from "./settingsGroupContext";

const Probe: React.FC = () => {
  const { density, visible } = useContext(SettingsGroupContext);
  return <i data-probe={`${density}:${visible}`}>probe-child</i>;
};

const render = (defaultOpen: boolean): string =>
  renderToStaticMarkup(
    <CollapsibleSettingsGroup id="t" title="Group T" defaultOpen={defaultOpen}>
      <Probe />
    </CollapsibleSettingsGroup>,
  );

/** The opening tag of the element whose id is the header's aria-controls. */
const bodyTag = (markup: string): string => {
  const controls = markup.match(/aria-controls="([^"]+)"/)?.[1];
  assert.ok(controls, "header must set aria-controls");
  const tag = markup.match(
    new RegExp(`<[a-z]+[^>]*id="${controls}"[^>]*>`),
  )?.[0];
  assert.ok(tag, "no element with the aria-controls id");
  return tag;
};

const scenarios: Record<string, () => void> = {
  "closed: header says collapsed, body is hidden, children still in the tree":
    () => {
      const m = render(false);
      assert.match(m, /<button[^>]*type="button"/);
      assert.match(m, /aria-expanded="false"/);
      assert.match(bodyTag(m), /\bhidden\b/);
      assert.ok(m.includes("probe-child"), "children must stay mounted");
      assert.ok(m.includes("Group T"));
      assert.ok(m.includes("settings-card"));
    },
  "open: header says expanded and the body is not hidden": () => {
    const m = render(true);
    assert.match(m, /aria-expanded="true"/);
    assert.ok(!/\bhidden\b/.test(bodyTag(m)));
    assert.ok(m.includes("probe-child"));
  },
  "body provides compact density and its own visibility": () => {
    assert.ok(render(true).includes('data-probe="compact:true"'));
    assert.ok(render(false).includes('data-probe="compact:false"'));
  },
  "default context is comfortable and visible": () => {
    const m = renderToStaticMarkup(<Probe />);
    assert.ok(m.includes('data-probe="comfortable:true"'));
  },
  "grouped row classes: comfortable is unchanged, compact is tighter": () => {
    assert.equal(groupedRowClass("comfortable"), "min-h-12 px-4 p-2");
    assert.equal(groupedRowClass("compact"), "min-h-10 px-3 py-1");
  },
  "tooltip renders only when requested AND the group is visible": () => {
    assert.equal(shouldShowTooltip(true, true), true);
    assert.equal(shouldShowTooltip(true, false), false);
    assert.equal(shouldShowTooltip(false, true), false);
    assert.equal(shouldShowTooltip(false, false), false);
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
if (failed > 0) throw new Error(`${failed} collapsible group scenarios failed`);
console.log(
  `collapsible group: ${Object.keys(scenarios).length} scenarios passed`,
);
