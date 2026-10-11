import assert from "node:assert/strict";
import type { TtsComposerSettings } from "../../bindings";
import {
  ackTuple,
  commandPatch,
  commandReadiness,
  composerFromApp,
  draftFromSettings,
  emptyPatch,
  isAcknowledged,
  isCommandDraftDirty,
  patchWith,
  statusLabelKey,
  KNOWN_OUTCOMES,
} from "./composerForm";

function settings(
  partial: Partial<TtsComposerSettings> = {},
): TtsComposerSettings {
  return {
    mode: "off",
    provider_id: "custom",
    model: "",
    command_path: "",
    command_args: "",
    prompt_flag: "-p",
    command_ack: "",
    timeout_secs: 30,
    style: "warm, upbeat and concise",
    send_user_utterance: true,
    fallback: "short_notice",
    ...partial,
  };
}

const PATCH_KEYS = [
  "acknowledge_command",
  "command_args",
  "command_path",
  "fallback",
  "mode",
  "model",
  "prompt_flag",
  "provider_id",
  "send_user_utterance",
  "style",
  "timeout_secs",
];

const scenarios: Record<string, () => void> = {
  "emptyPatch has all 11 keys, all null": () => {
    const p = emptyPatch();
    assert.deepEqual(Object.keys(p).sort(), PATCH_KEYS);
    for (const k of PATCH_KEYS) {
      assert.equal((p as Record<string, unknown>)[k], null, k);
    }
  },
  "patchWith overlays and never mutates": () => {
    const base = emptyPatch();
    const p = patchWith({ mode: "model", model: "m" });
    assert.equal(p.mode, "model");
    assert.equal(p.model, "m");
    assert.equal(p.style, null);
    assert.equal(base.mode, null);
    assert.deepEqual(Object.keys(p).sort(), PATCH_KEYS);
  },
  "ackTuple normalizes like the backend": () => {
    assert.equal(
      ackTuple(" /bin/agent ", "--no-tools   -m  fast ", " -p "),
      "/bin/agent\0--no-tools -m fast\0-p",
    );
    assert.equal(ackTuple("/a", "", "-p"), "/a\0\0-p");
  },
  "isAcknowledged requires a non-empty matching tuple": () => {
    const ok = settings({
      command_path: "/bin/agent",
      command_args: "-m fast",
      command_ack: "/bin/agent\0-m fast\0-p",
    });
    assert.equal(isAcknowledged(ok), true);
    assert.equal(isAcknowledged({ ...ok, command_ack: "" }), false);
    assert.equal(isAcknowledged({ ...ok, command_args: "-m slow" }), false);
    assert.equal(isAcknowledged({ ...ok, command_path: "/bin/other" }), false);
    assert.equal(isAcknowledged({ ...ok, prompt_flag: "--prompt" }), false);
    assert.equal(
      isAcknowledged({ ...ok, command_args: "-m    fast " }),
      true,
      "cosmetic spacing does not revoke",
    );
    assert.equal(isAcknowledged(settings()), false);
  },
  "draftFromSettings mirrors the saved values and ack state": () => {
    const s = settings({
      command_path: "/bin/agent",
      command_args: "-m fast",
      command_ack: "/bin/agent\0-m fast\0-p",
    });
    assert.deepEqual(draftFromSettings(s), {
      path: "/bin/agent",
      args: "-m fast",
      flag: "-p",
      acknowledge: true,
    });
    assert.equal(draftFromSettings(settings()).acknowledge, false);
  },
  "commandPatch sends the whole form in ONE patch": () => {
    const p = commandPatch({
      path: " /bin/agent ",
      args: " --no-tools  -m fast ",
      flag: " -p ",
      acknowledge: true,
    });
    assert.equal(p.command_path, "/bin/agent");
    assert.equal(p.command_args, "--no-tools -m fast");
    assert.equal(p.prompt_flag, "-p");
    assert.equal(p.acknowledge_command, true);
    for (const k of PATCH_KEYS.filter(
      (k) =>
        ![
          "command_path",
          "command_args",
          "prompt_flag",
          "acknowledge_command",
        ].includes(k),
    )) {
      assert.equal((p as Record<string, unknown>)[k], null, k);
    }
    assert.equal(
      commandPatch({ path: "/a", args: "", flag: "-p", acknowledge: false })
        .acknowledge_command,
      false,
    );
  },
  "isCommandDraftDirty ignores cosmetic differences": () => {
    const s = settings({
      command_path: "/bin/agent",
      command_args: "-m fast",
      command_ack: "/bin/agent\0-m fast\0-p",
    });
    const same = draftFromSettings(s);
    assert.equal(isCommandDraftDirty(same, s), false);
    assert.equal(isCommandDraftDirty({ ...same, path: "/bin/other" }, s), true);
    assert.equal(
      isCommandDraftDirty({ ...same, args: "-m  fast  " }, s),
      false,
    );
    assert.equal(isCommandDraftDirty({ ...same, args: "-m slow" }, s), true);
    assert.equal(isCommandDraftDirty({ ...same, flag: "--prompt" }, s), true);
    assert.equal(isCommandDraftDirty({ ...same, flag: "" }, s), false);
    assert.equal(isCommandDraftDirty({ ...same, acknowledge: false }, s), true);
  },
  commandReadiness: () => {
    assert.equal(commandReadiness(settings()), "needs_setup");
    const configured = settings({ command_path: "/bin/agent" });
    assert.equal(commandReadiness(configured), "needs_acknowledgement");
    assert.equal(
      commandReadiness({
        ...configured,
        command_ack: "/bin/agent\0\0-p",
      }),
      "ready",
    );
    assert.equal(
      commandReadiness({ ...configured, command_ack: "/stale\0\0-p" }),
      "needs_acknowledgement",
    );
  },
  "statusLabelKey maps known outcomes and falls back": () => {
    for (const o of KNOWN_OUTCOMES) {
      assert.equal(
        statusLabelKey(o),
        `settings.advanced.tts.composer.status.${o}`,
      );
    }
    assert.equal(
      statusLabelKey("whatever"),
      "settings.advanced.tts.composer.status.unknown",
    );
    assert.equal(
      statusLabelKey(""),
      "settings.advanced.tts.composer.status.unknown",
    );
    assert.equal(KNOWN_OUTCOMES.length, 9);
  },
  "composerFromApp maps flat fields and defaults the rest": () => {
    assert.deepEqual(composerFromApp({}), settings());
    const out = composerFromApp({
      tts_composer_mode: "command",
      tts_composer_provider_id: "groq",
      tts_composer_model: "m",
      tts_composer_command_path: "/bin/agent",
      tts_composer_command_args: "-m fast",
      tts_composer_prompt_flag: "--prompt",
      tts_composer_command_ack: "x",
      tts_composer_timeout_secs: 12,
      tts_composer_style: "calm",
      tts_composer_send_user_utterance: false,
      tts_composer_fallback: "silent",
    });
    assert.deepEqual(out, {
      mode: "command",
      provider_id: "groq",
      model: "m",
      command_path: "/bin/agent",
      command_args: "-m fast",
      prompt_flag: "--prompt",
      command_ack: "x",
      timeout_secs: 12,
      style: "calm",
      send_user_utterance: false,
      fallback: "silent",
    });
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
  throw new Error(
    `${failed} of ${Object.keys(scenarios).length} composer form scenarios failed`,
  );
}
console.log(`composer form: ${Object.keys(scenarios).length} scenarios passed`);
