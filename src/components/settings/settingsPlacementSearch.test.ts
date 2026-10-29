import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { SEARCH_ENTRIES } from "./search/settingsSearchIndex";
import { resolveTarget, search } from "./search/settingsSearch";
import type {
  Resolve,
  SearchContext,
  SearchEntry,
  SearchSection,
} from "./search/types";
import { PLACED_GROUPS } from "./settingsPlacement";

// Real English strings, so the test searches the way a user would.
const en = JSON.parse(
  readFileSync(
    new URL("../../i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

const lookup = (key: string): string | undefined => {
  let node: unknown = en;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
};

const resolve: Resolve = (key) => {
  const text = lookup(key) ?? key;
  return { local: text, en: text };
};

// Sections a user can see while the Experimental switch is off / on. The old
// "experimental" page and the new "agents" tab are both gated by the switch;
// postprocessing and debug are gated by their own switches and not under test.
const ALWAYS: readonly string[] = [
  "general",
  "history",
  "models",
  "advanced",
  "about",
];
const ctxOff: SearchContext = {
  isSectionEnabled: (s: SearchSection) => ALWAYS.includes(s),
  experimentalEnabled: false,
  platform: "macos",
};
const ctxOn: SearchContext = {
  isSectionEnabled: (s: SearchSection) =>
    ALWAYS.includes(s) || (s as string) === "agents",
  experimentalEnabled: true,
  platform: "macos",
};
// A context that predates the flag (field omitted) must keep the old behaviour.
const ctxLegacy: SearchContext = {
  isSectionEnabled: () => true,
  platform: "macos",
};

const requiresExperimental = (e: SearchEntry): boolean =>
  (e as { requiresExperimental?: boolean }).requiresExperimental === true;
// Every row of a placed control (gated or always visible) carries its group id.
// "sttModels" is the Models page's Speech-to-text card: a known group that is not a placed
// group, so its rows (language, translation, unload timeout) are checked by their own scenario.
const STT_GROUP_ID = "sttModels";
const moved = SEARCH_ENTRIES.filter(
  (e) => e.groupId !== undefined && e.groupId !== STT_GROUP_ID,
);
const gated = moved.filter(requiresExperimental);

const scenarios: Record<string, () => void> = {
  "moved rows exist and none still claims the experimental section": () => {
    assert.ok(moved.length >= 80, `found ${moved.length} moved rows`);
    for (const e of moved) {
      assert.notEqual(String(e.section), "experimental", e.id);
    }
  },
  "every moved row points at a real placed group in its own section": () => {
    for (const e of moved) {
      assert.ok(e.groupId, `${e.id} has a groupId`);
      const group = PLACED_GROUPS.find((g) => g.id === e.groupId);
      assert.ok(group, `${e.id}: unknown group ${e.groupId}`);
      assert.equal(String(e.section), group.section, e.id);
      // Gating follows the group: experimental groups need the switch, the rest do not.
      // The one exception is a control that is experimental on its own inside an ungated
      // group (realtime and the latency panel in Conversation), covered by its own scenario.
      const ownGate =
        e.labelKey.startsWith("settings.experimental.realtime.") ||
        e.labelKey === "converse.voice.latency.label";
      const gated = group.experimental || ownGate;
      assert.equal(requiresExperimental(e), gated, e.id);
      assert.equal(
        e.unlockedBy,
        gated ? "advanced.experimentalToggle" : undefined,
        e.id,
      );
    }
  },
  "switch off: every gated row is locked and resolves to the toggle": () => {
    let checked = 0;
    for (const e of gated) {
      const label = lookup(e.labelKey);
      if (label === undefined) continue; // dynamic labels are covered by the index tests
      const hits = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
        limit: 1000,
      });
      const hit = hits.find((h) => h.entry.id === e.id);
      assert.ok(hit, `${e.id} ("${label}") is searchable with the switch off`);
      assert.equal(hit.locked, true, `${e.id} must be locked`);
      assert.equal(
        resolveTarget(hit, SEARCH_ENTRIES, ctxOff).id,
        "advanced.experimentalToggle",
        e.id,
      );
      checked += 1;
    }
    assert.ok(checked >= 50, `only ${checked} gated rows were checked`);
  },
  "switch off: rows of always-visible groups stay unlocked and reachable":
    () => {
      for (const e of moved.filter((x) => !requiresExperimental(x))) {
        const label = lookup(e.labelKey);
        if (label === undefined) continue;
        const hit = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === e.id);
        assert.ok(
          hit,
          `${e.id} ("${label}") is searchable with the switch off`,
        );
        assert.equal(hit.locked, false, `${e.id} must stay unlocked`);
        assert.equal(resolveTarget(hit, SEARCH_ENTRIES, ctxOff).id, e.id, e.id);
      }
    },
  "switch off: ordinary rows in always-visible sections stay unlocked": () => {
    const hits = search(SEARCH_ENTRIES, "experimental", resolve, ctxOff, {
      limit: 1000,
    });
    const toggle = hits.find(
      (h) => h.entry.id === "advanced.experimentalToggle",
    );
    assert.ok(toggle);
    assert.equal(toggle.locked, false);
  },
  "switch on: moved rows are unlocked and reveal their own group": () => {
    let checked = 0;
    for (const e of moved) {
      const label = lookup(e.labelKey);
      if (label === undefined) continue;
      const hit = search(SEARCH_ENTRIES, label, resolve, ctxOn, {
        limit: 1000,
      }).find((h) => h.entry.id === e.id);
      assert.ok(hit, e.id);
      assert.equal(hit.locked, false, e.id);
      assert.equal(resolveTarget(hit, SEARCH_ENTRIES, ctxOn).id, e.id);
      checked += 1;
    }
    assert.ok(checked >= 60);
  },
  "a context without the flag keeps the old behaviour (treated as on)": () => {
    const e = moved.find((x) => lookup(x.labelKey) !== undefined);
    assert.ok(e);
    const hit = search(
      SEARCH_ENTRIES,
      lookup(e.labelKey) as string,
      resolve,
      ctxLegacy,
      { limit: 1000 },
    ).find((h) => h.entry.id === e.id);
    assert.ok(hit);
    assert.equal(hit.locked, false);
  },
  "realtime and latency rows stay experimental inside the ungated Conversation group":
    () => {
      const rows = SEARCH_ENTRIES.filter(
        (e) =>
          e.groupId === "conversation" &&
          (e.labelKey.startsWith("settings.experimental.realtime.") ||
            e.labelKey === "converse.voice.latency.label"),
      );
      assert.equal(
        rows.length,
        16,
        `found ${rows.length} realtime/latency rows`,
      );
      for (const e of rows) {
        assert.ok(e.id.startsWith("experimental."), e.id);
        assert.equal(String(e.section), "general", e.id);
        assert.equal(requiresExperimental(e), true, e.id);
        assert.equal(e.unlockedBy, "advanced.experimentalToggle", e.id);
        const label = lookup(e.labelKey) as string;
        const off = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === e.id);
        assert.ok(
          off,
          `${e.id} ("${label}") is searchable with the switch off`,
        );
        assert.equal(off.locked, true, e.id);
        const on = search(SEARCH_ENTRIES, label, resolve, ctxOn, {
          limit: 1000,
        }).find((h) => h.entry.id === e.id);
        assert.ok(on, e.id);
        assert.equal(on.locked, false, e.id);
      }
    },
  "switch off: reply style, smart turn, speaker filter, VAD and spoken reply rows are reachable":
    () => {
      const wanted = [
        "general.spokenReplyMode",
        "models.conversationSmartTurn",
        "models.conversationSpeakerFilter",
        "models.vadBackend",
        "models.tts",
      ];
      for (const id of wanted) {
        const e = SEARCH_ENTRIES.find((x) => x.id === id);
        assert.ok(e, `${id} exists`);
        assert.equal(requiresExperimental(e), false, `${id} is ungated`);
        const label = lookup(e.labelKey) as string;
        const hit = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === id);
        assert.ok(hit, `${id} ("${label}") is searchable with the switch off`);
        assert.equal(hit.locked, false, `${id} must stay unlocked`);
        assert.equal(resolveTarget(hit, SEARCH_ENTRIES, ctxOff).id, id, id);
      }
    },
  "rows follow their control into its new group": () => {
    // labelKey prefix -> the group that now holds the control that owns the row.
    const expected: Array<[string, string]> = [
      ["settings.advanced.tts.composer.", "conversation"],
      ["converse.section.", "conversation"],
      ["converse.voice.persona.", "conversation"],
      ["converse.voice.latency.", "conversation"],
      ["converse.voice.smartTurn.", "vadEngine"],
      ["converse.voice.speaker.", "vadEngine"],
    ];
    for (const [prefix, groupId] of expected) {
      const rows = moved.filter((e) => e.labelKey.startsWith(prefix));
      assert.ok(rows.length > 0, `no row found for ${prefix}`);
      for (const e of rows) {
        assert.equal(e.groupId, groupId, `${e.id} (${prefix})`);
      }
    }
    const sectionOf = (groupId: string): string | undefined =>
      PLACED_GROUPS.find((g) => g.id === groupId)?.section;
    assert.equal(sectionOf("conversation"), "general");
    assert.equal(sectionOf("vadEngine"), "models");
    assert.equal(sectionOf("spokenReplies"), "models");
  },
  "language, translation and the unload timeout live in Models and stay ungated":
    () => {
      const wanted: Array<[string, string]> = [
        ["models.language", "settings.general.language.title"],
        [
          "models.translateToEnglish",
          "settings.advanced.translateToEnglish.label",
        ],
        ["models.modelUnload", "settings.advanced.modelUnload.title"],
      ];
      for (const [id, labelKey] of wanted) {
        const e = SEARCH_ENTRIES.find((x) => x.id === id);
        assert.ok(e, `${id} exists`);
        assert.equal(String(e.section), "models", id);
        assert.equal(e.labelKey, labelKey, id);
        assert.ok(
          !requiresExperimental(e),
          `${id} must not require experimental`,
        );
        assert.equal(e.unlockedBy, undefined, id);
        // searchable and unlocked with the switch OFF, resolves to itself
        const label = lookup(labelKey) as string;
        const hit = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === id);
        assert.ok(hit, `${id} is searchable with the switch off`);
        assert.equal(hit.locked, false, id);
        assert.equal(resolveTarget(hit, SEARCH_ENTRIES, ctxOff).id, id);
      }
      const unload = SEARCH_ENTRIES.find((x) => x.id === "models.modelUnload");
      assert.equal(unload?.groupKey, "settings.models.groups.speechToText");
      for (const old of [
        "general.language",
        "general.translateToEnglish",
        "advanced.modelUnload",
      ]) {
        assert.ok(
          !SEARCH_ENTRIES.some((x) => x.id === old),
          `${old} was renamed`,
        );
      }
    },
  "the wake word row is ungated in Conversation; no action dropdown row exists; the barge-in control is always visible in its group":
    () => {
      assert.ok(
        !SEARCH_ENTRIES.some(
          (x) => x.labelKey === "settings.advanced.wakeWord.action.label",
        ),
        "the wake word action row is gone",
      );
      assert.ok(
        !SEARCH_ENTRIES.some(
          (x) =>
            x.id === "wakeWordSilence" ||
            x.labelKey === "settings.advanced.wakeWord.silenceTimeoutLabel",
        ),
        "the wake word silence slider has no search row (its silence watch was removed)",
      );
      const wanted: Array<[string, string, string, boolean]> = [
        ["settings.advanced.wakeWord.label", "conversation", "general", false],
        ["converse.section.bargeIn.label", "conversation", "general", false],
      ];
      for (const [labelKey, groupId, sectionId, gate] of wanted) {
        const e = SEARCH_ENTRIES.find((x) => x.labelKey === labelKey);
        assert.ok(e, `${labelKey} has a search row`);
        assert.equal(e.groupId, groupId, labelKey);
        assert.equal(String(e.section), sectionId, labelKey);
        assert.equal(requiresExperimental(e), gate, labelKey);
        assert.equal(
          e.unlockedBy,
          gate ? "advanced.experimentalToggle" : undefined,
          labelKey,
        );
        assert.ok(e.descriptionKey, `${labelKey} has a description`);
        assert.ok(
          lookup(e.descriptionKey as string),
          `${labelKey} description text`,
        );
        // searchable by its own English label; locked only while the switch is off and gated
        const label = lookup(labelKey) as string;
        const hit = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === e.id);
        assert.ok(hit, `${labelKey} is searchable`);
        assert.equal(hit.locked, gate, labelKey);
      }
    },
  "acp permission rows sit in the ACP agent group behind the Experimental switch":
    () => {
      const wanted: Array<[string, string, string]> = [
        [
          "experimental.acpPermissionMode",
          "settings.advanced.acpPermission.mode.label",
          "settings.advanced.acpPermission.mode.description",
        ],
        [
          "experimental.acpPermissionTimeout",
          "settings.advanced.acpPermission.timeout.label",
          "settings.advanced.acpPermission.timeout.description",
        ],
        [
          "experimental.acpConversationHandoff",
          "settings.advanced.acpAgent.conversationHandoff.label",
          "settings.advanced.acpAgent.conversationHandoff.description",
        ],
      ];
      for (const [id, labelKey, descriptionKey] of wanted) {
        const e = SEARCH_ENTRIES.find((x) => x.id === id);
        assert.ok(e, `${id} exists`);
        assert.equal(String(e.section), "agents", id);
        assert.equal(e.groupId, "acpAgent", id);
        assert.equal(e.labelKey, labelKey, id);
        assert.equal(e.descriptionKey, descriptionKey, id);
        assert.ok(requiresExperimental(e), `${id} must require experimental`);
        assert.equal(e.unlockedBy, "advanced.experimentalToggle", id);
        assert.ok(lookup(labelKey), `${labelKey} exists in English`);
        assert.ok(
          lookup(descriptionKey),
          `${descriptionKey} exists in English`,
        );

        // Searching the English label finds the row: locked with the switch off,
        // unlocked and resolving to itself with the switch on.
        const label = lookup(labelKey) as string;
        const off = search(SEARCH_ENTRIES, label, resolve, ctxOff, {
          limit: 1000,
        }).find((h) => h.entry.id === id);
        assert.ok(off, `${id} ("${label}") is searchable with the switch off`);
        assert.equal(off.locked, true, id);
        const on = search(SEARCH_ENTRIES, label, resolve, ctxOn, {
          limit: 1000,
        }).find((h) => h.entry.id === id);
        assert.ok(on, `${id} ("${label}") is searchable with the switch on`);
        assert.equal(on.locked, false, id);
        assert.equal(resolveTarget(on, SEARCH_ENTRIES, ctxOn).id, id);
      }
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
  `settings placement search: ${Object.keys(scenarios).length} scenarios passed`,
);
