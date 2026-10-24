import assert from "node:assert/strict";
import {
  permissionViewModel,
  type PermissionRequestEvent,
} from "./permissionViewModel";

// Pure view-model test for the ACP permission approval dialog (DESIGN.md Revision 3,
// C6): details are shown or an explicit unavailable warning is shown, never an empty
// body, and Deny is always the default-focused control. No React, no DOM.

const base: PermissionRequestEvent = {
  token: "perm-1",
  title: "Run a command",
  kind: "execute",
  details: "command: rm -rf ~/x",
  truncated: false,
  timeoutSecs: 60,
  queued: 0,
};

const KIND_KEYS: Record<string, string> = {
  read: "settings.advanced.acpPermission.kind.read",
  edit: "settings.advanced.acpPermission.kind.edit",
  delete: "settings.advanced.acpPermission.kind.delete",
  move: "settings.advanced.acpPermission.kind.move",
  search: "settings.advanced.acpPermission.kind.search",
  execute: "settings.advanced.acpPermission.kind.execute",
  think: "settings.advanced.acpPermission.kind.think",
  fetch: "settings.advanced.acpPermission.kind.fetch",
  switch_mode: "settings.advanced.acpPermission.kind.switch_mode",
  other: "settings.advanced.acpPermission.kind.other",
};

const scenarios: Record<string, () => void> = {
  "exposes concrete command details": () => {
    const vm = permissionViewModel(base);
    assert.equal(vm.token, "perm-1");
    assert.equal(vm.title, "Run a command");
    assert.equal(vm.detailsText, "command: rm -rf ~/x");
    assert.equal(vm.showUnavailableWarning, false);
    assert.equal(vm.timeoutSecs, 60);
    assert.equal(vm.queued, 0);
  },

  "unavailable warning when details is null": () => {
    const vm = permissionViewModel({ ...base, details: null });
    assert.equal(vm.detailsText, null);
    assert.equal(vm.showUnavailableWarning, true);
  },

  "unavailable warning when details is blank": () => {
    for (const blank of ["", "   ", "\n\t  \n"]) {
      const vm = permissionViewModel({ ...base, details: blank });
      assert.equal(vm.showUnavailableWarning, true, JSON.stringify(blank));
      assert.equal(vm.detailsText, null, JSON.stringify(blank));
    }
  },

  "truncated flag passes through": () => {
    assert.equal(
      permissionViewModel({ ...base, truncated: true }).truncated,
      true,
    );
    assert.equal(permissionViewModel(base).truncated, false);
  },

  "default focus is always deny": () => {
    const cases: PermissionRequestEvent[] = [
      base,
      { ...base, details: null },
      { ...base, details: "  " },
      { ...base, truncated: true },
      { ...base, kind: "unknown_kind", queued: 3 },
    ];
    for (const p of cases) {
      assert.equal(permissionViewModel(p).defaultFocus, "deny");
    }
  },

  "unknown kind maps to other key": () => {
    for (const kind of ["frobnicate", "", "EXECUTE", "execute "]) {
      assert.equal(
        permissionViewModel({ ...base, kind }).kindLabelKey,
        KIND_KEYS.other,
        JSON.stringify(kind),
      );
    }
  },

  "known kinds map to their keys": () => {
    for (const [kind, key] of Object.entries(KIND_KEYS)) {
      if (kind === "other") continue;
      assert.equal(
        permissionViewModel({ ...base, kind }).kindLabelKey,
        key,
        kind,
      );
    }
  },

  "title is trimmed and empty title stays empty": () => {
    assert.equal(
      permissionViewModel({ ...base, title: "  Run it \n" }).title,
      "Run it",
    );
    assert.equal(permissionViewModel({ ...base, title: "" }).title, "");
    assert.equal(permissionViewModel({ ...base, title: "   " }).title, "");
  },
};

let failed = 0;
for (const [name, fn] of Object.entries(scenarios)) {
  try {
    fn();
  } catch (error) {
    failed += 1;
    console.error(`FAIL: ${name}\n`, error);
  }
}
if (failed > 0) {
  console.error(`${failed} scenario(s) failed`);
  process.exit(1);
}
console.log(
  `permission prompt view model: ${Object.keys(scenarios).length} scenarios passed`,
);
