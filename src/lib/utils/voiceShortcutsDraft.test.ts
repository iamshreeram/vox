import assert from "node:assert/strict";
import {
  actionTarget,
  addRow,
  changeActionType,
  emptyShortcut,
  isDirty,
  removeRow,
  setActionTarget,
  updateRow,
} from "./voiceShortcutsDraft";
import type { VoiceShortcut } from "@/bindings";

const url = (phrase: string, u: string): VoiceShortcut => ({
  phrase,
  action: { type: "open_url", url: u },
});
const folder = (phrase: string, p: string): VoiceShortcut => ({
  phrase,
  action: { type: "open_path", path: p },
});

// emptyShortcut is a blank URL shortcut.
assert.deepEqual(emptyShortcut(), {
  phrase: "",
  action: { type: "open_url", url: "" },
});

// addRow appends a blank row and does not mutate the input.
{
  const list = [url("docs", "https://a.test")];
  const next = addRow(list);
  assert.equal(next.length, 2);
  assert.deepEqual(next[1], emptyShortcut());
  assert.equal(list.length, 1);
  assert.notEqual(next, list);
}

// updateRow shallow-merges the patch into the targeted row only.
{
  const list = [url("docs", "https://a.test"), url("mail", "https://b.test")];
  const next = updateRow(list, 1, { phrase: "inbox" });
  assert.equal(next[1].phrase, "inbox");
  assert.deepEqual(next[1].action, { type: "open_url", url: "https://b.test" });
  assert.equal(next[0], list[0]);
  assert.equal(list[1].phrase, "mail", "input must not be mutated");
}

// updateRow with an out-of-range index is a no-op.
{
  const list = [url("docs", "https://a.test")];
  assert.deepEqual(updateRow(list, 5, { phrase: "x" }), list);
  assert.deepEqual(updateRow(list, -1, { phrase: "x" }), list);
  assert.deepEqual(updateRow(list, 1.5, { phrase: "x" }), list);
}

// removeRow drops the targeted row without mutating the input.
{
  const list = [url("a", "https://a.test"), url("b", "https://b.test")];
  const next = removeRow(list, 0);
  assert.deepEqual(next, [url("b", "https://b.test")]);
  assert.equal(list.length, 2);
}

// removeRow with an out-of-range index is a no-op.
{
  const list = [url("a", "https://a.test")];
  assert.deepEqual(removeRow(list, 3), list);
  assert.deepEqual(removeRow(list, -1), list);
}

// isDirty: identical lists are clean, any change is dirty.
{
  const saved = [url("docs", "https://a.test")];
  assert.equal(isDirty(saved, [url("docs", "https://a.test")]), false);
  assert.equal(isDirty(saved, []), true);
  assert.equal(isDirty(saved, [url("docs", "https://z.test")]), true);
  assert.equal(isDirty([], []), false);
  assert.equal(isDirty([], [emptyShortcut()]), true);
}

// changeActionType resets the target and switches the variant.
{
  const start = url("docs", "https://a.test");
  assert.deepEqual(changeActionType(start, "open_path"), {
    phrase: "docs",
    action: { type: "open_path", path: "" },
  });
  assert.deepEqual(changeActionType(start, "open_app"), {
    phrase: "docs",
    action: { type: "open_app", name: "" },
  });
  assert.deepEqual(changeActionType(start, "open_url").action, {
    type: "open_url",
    url: "",
  });
  assert.equal(start.action.type, "open_url", "input must not be mutated");
}

// actionTarget / setActionTarget round-trip the target for each variant.
{
  assert.equal(actionTarget(url("a", "https://a.test")), "https://a.test");
  assert.equal(actionTarget(folder("a", "~/Downloads")), "~/Downloads");
  const app: VoiceShortcut = {
    phrase: "a",
    action: { type: "open_app", name: "Safari" },
  };
  assert.equal(actionTarget(app), "Safari");
  assert.deepEqual(
    setActionTarget(folder("a", "~"), "~/Docs"),
    folder("a", "~/Docs"),
  );
}

console.log("voice shortcuts draft: all assertions passed");
