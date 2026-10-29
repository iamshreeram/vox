import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";

/*
 * Issue #118: Settings > General > Conversation > Spoken reply style, command mode.
 * "I understand the risks" approves ONE command tuple (path, args, prompt flag).
 * Editing path, args or flag (a change to the trimmed path / whitespace-collapsed args /
 * trimmed flag) must UNTICK the checkbox; a cosmetic edit (trailing space on the path) keeps it.
 *
 * Driven through the real settings UI with the Tauri bridge mocked (tests/fixtures/tauriMock.ts).
 * Save sends the tauri-specta command `change_tts_composer_settings` with args `{ patch }`
 * (src/bindings.ts). Selectors come from data attributes (data-group-id, data-setting-title)
 * and English strings from src/i18n/locales/en/translation.json.
 */

const TRANSLATION = JSON.parse(
  readFileSync(
    new URL("../src/i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

function textAt(root: Record<string, unknown>, dottedKey: string): string {
  let node: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (typeof node !== "object" || node === null || !(part in node)) {
      throw new Error(`missing translation key ${dottedKey}`);
    }
    node = (node as Record<string, unknown>)[part];
  }
  if (typeof node !== "string") {
    throw new Error(`translation key ${dottedKey} is not a string`);
  }
  return node;
}

const label = (key: string): string => textAt(TRANSLATION, key);

const COMPOSER = "settings.advanced.tts.composer";
const ACK_LABEL = label(`${COMPOSER}.ackLabel`);
const SAVE_LABEL = label(`${COMPOSER}.saveCommand`);
const PATH_LABEL = label(`${COMPOSER}.commandPathLabel`);
const ARGS_LABEL = label(`${COMPOSER}.commandArgsLabel`);
const FLAG_LABEL = label(`${COMPOSER}.promptFlagLabel`);

const OLD_PATH = "/opt/audit/composer";
const OLD_ARGS = "-m fast";
const OLD_FLAG = "-p";
// The stored ack is the tuple path \0 args \0 flag (composerForm.ts ackTuple).
const OLD_ACK = `${OLD_PATH}\0${OLD_ARGS}\0${OLD_FLAG}`;

const SAVE_COMMAND = "change_tts_composer_settings";

interface SavedPatch {
  command_path?: string | null;
  command_args?: string | null;
  prompt_flag?: string | null;
  acknowledge_command?: boolean | null;
}

const navItem = (page: Page, text: string): Locator =>
  page.locator("p", {
    hasText: new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
  });

/** Opens the command-mode reply style composer (General > Conversation group). */
async function openCommandComposer(
  page: Page,
  settings: Record<string, unknown>,
): Promise<Locator> {
  await installTauriMock(page, {
    settings: {
      tts_composer_mode: "command",
      tts_composer_command_path: OLD_PATH,
      tts_composer_command_args: OLD_ARGS,
      tts_composer_prompt_flag: OLD_FLAG,
      tts_composer_command_ack: "",
      tts_composer_send_user_utterance: true,
      ...settings,
    },
  });
  await page.goto("/");
  await expect(navItem(page, label("sidebar.general")).first()).toBeVisible({
    timeout: 30000,
  });
  await navItem(page, label("sidebar.general")).first().click();

  const group = page.locator('[data-group-id="conversation"]');
  const header = group.locator(":scope > button");
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(
    group.locator(`[data-setting-title="${ACK_LABEL}"]`),
  ).toBeVisible();
  return group;
}

const textField = (group: Locator, title: string): Locator =>
  group.locator(`[data-setting-title="${title}"] input[type="text"]`);

const ackCheckbox = (group: Locator): Locator =>
  group.locator(`[data-setting-title="${ACK_LABEL}"] input[type="checkbox"]`);

/** Clicks the visible toggle (the input itself is sr-only). */
const toggleAck = (group: Locator): Promise<void> =>
  group.locator(`[data-setting-title="${ACK_LABEL}"] label`).click();

const saveButton = (group: Locator): Locator =>
  group.getByRole("button", { name: SAVE_LABEL, exact: true });

/** Every `change_tts_composer_settings` patch sent so far, in order. */
async function savedPatches(page: Page): Promise<SavedPatch[]> {
  const calls = await getMockCalls(page, SAVE_COMMAND);
  return calls.map((args) => args.patch as SavedPatch);
}

/** Waits for the Nth save invoke to arrive and returns its patch. */
async function nthSave(page: Page, n: number): Promise<SavedPatch> {
  await expect
    .poll(async () => (await savedPatches(page)).length, { timeout: 10000 })
    .toBe(n);
  const patches = await savedPatches(page);
  const patch = patches[n - 1];
  if (patch === undefined) throw new Error(`no save #${n}`);
  return patch;
}

test.describe("#118 command acknowledgement resets on command edits", () => {
  test("S1 editing the command PATH unticks the acknowledgement; save and re-ack", async ({
    page,
  }) => {
    const group = await openCommandComposer(page, {
      tts_composer_command_ack: OLD_ACK,
    });
    const newPath = "/opt/audit/composer-v2";

    await expect(ackCheckbox(group)).toBeChecked();

    await textField(group, PATH_LABEL).fill(newPath);
    await expect(textField(group, PATH_LABEL)).toHaveValue(newPath);
    await expect(ackCheckbox(group)).not.toBeChecked();

    await saveButton(group).click();
    const first = await nthSave(page, 1);
    expect(first.acknowledge_command).toBe(false);
    expect(first.command_path).toBe(newPath);

    await toggleAck(group);
    await expect(ackCheckbox(group)).toBeChecked();
    await saveButton(group).click();
    const second = await nthSave(page, 2);
    expect(second.acknowledge_command).toBe(true);
    expect(second.command_path).toBe(newPath);
  });

  test("S2 editing the command ARGS unticks the acknowledgement; save and re-ack", async ({
    page,
  }) => {
    const group = await openCommandComposer(page, {
      tts_composer_command_ack: OLD_ACK,
    });
    const newArgs = "-m slow";

    await expect(ackCheckbox(group)).toBeChecked();

    await textField(group, ARGS_LABEL).fill(newArgs);
    await expect(textField(group, ARGS_LABEL)).toHaveValue(newArgs);
    await expect(ackCheckbox(group)).not.toBeChecked();

    await saveButton(group).click();
    const first = await nthSave(page, 1);
    expect(first.acknowledge_command).toBe(false);
    expect(first.command_args).toBe(newArgs);

    await toggleAck(group);
    await expect(ackCheckbox(group)).toBeChecked();
    await saveButton(group).click();
    const second = await nthSave(page, 2);
    expect(second.acknowledge_command).toBe(true);
    expect(second.command_args).toBe(newArgs);
  });

  test("S3 editing the prompt FLAG unticks the acknowledgement; save and re-ack", async ({
    page,
  }) => {
    const group = await openCommandComposer(page, {
      tts_composer_command_ack: OLD_ACK,
    });
    const newFlag = "-q";

    await expect(ackCheckbox(group)).toBeChecked();

    await textField(group, FLAG_LABEL).fill(newFlag);
    await expect(textField(group, FLAG_LABEL)).toHaveValue(newFlag);
    await expect(ackCheckbox(group)).not.toBeChecked();

    await saveButton(group).click();
    const first = await nthSave(page, 1);
    expect(first.acknowledge_command).toBe(false);
    expect(first.prompt_flag).toBe(newFlag);

    await toggleAck(group);
    await expect(ackCheckbox(group)).toBeChecked();
    await saveButton(group).click();
    const second = await nthSave(page, 2);
    expect(second.acknowledge_command).toBe(true);
    expect(second.prompt_flag).toBe(newFlag);
  });

  test("S4 a cosmetic edit (trailing space on the path) keeps the acknowledgement ticked", async ({
    page,
  }) => {
    const group = await openCommandComposer(page, {
      tts_composer_command_ack: OLD_ACK,
    });

    await expect(ackCheckbox(group)).toBeChecked();

    const path = textField(group, PATH_LABEL);
    await path.fill(`${OLD_PATH} `);
    await expect(path).toHaveValue(`${OLD_PATH} `);
    await expect(ackCheckbox(group)).toBeChecked();
  });

  test("S5 control: ticking without any edit still acknowledges and saves acknowledge_command true", async ({
    page,
  }) => {
    const group = await openCommandComposer(page, {
      tts_composer_command_ack: "",
    });

    await expect(ackCheckbox(group)).not.toBeChecked();

    await toggleAck(group);
    await expect(ackCheckbox(group)).toBeChecked();

    await saveButton(group).click();
    const patch = await nthSave(page, 1);
    expect(patch.acknowledge_command).toBe(true);
    expect(patch.command_path).toBe(OLD_PATH);
  });
});
