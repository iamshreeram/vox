import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import {
  getMockCalls,
  installTauriMock,
  setMockCommand,
} from "./fixtures/tauriMock";

/*
 * Settings > Agents > Conversation memory > "Remember recent conversations" (the opt-in
 * conversation summary memory). Mounted against the Vite dev server with the Tauri mock.
 * Copy is read from the English translation file so the assertions track the real strings.
 *
 * Commands: change_conversation_memory_setting {enabled}, conversation_memory_count,
 * clear_conversation_memory. The mock returns null for change_* commands.
 */

const en = JSON.parse(
  readFileSync(
    new URL("../src/i18n/locales/en/translation.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;

function text(dotted: string): string {
  let node: unknown = en;
  for (const part of dotted.split(".")) {
    node = (node as Record<string, unknown> | undefined)?.[part];
  }
  if (typeof node !== "string") throw new Error(`missing ${dotted}`);
  return node;
}

const K = "settings.experimental.rememberConversations";
const LABEL = text(`${K}.label`);
const DESCRIPTION = text(`${K}.description`);
const CLEAR_BUTTON = text(`${K}.clearButton`);
const CONFIRM_ACTION = text(`${K}.clearConfirmAction`);
const CONFIRM_TEXT = text(`${K}.clearConfirm`);
const COUNT_3 = "Remembered conversations: 3";
const COUNT_0 = "Remembered conversations: 0";
const CLEAR_FAILED = text(`${K}.clearFailed`);

const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });
const group = (page: Page): Locator =>
  page.locator('[data-group-id="conversationMemory"]');
const row = (page: Page): Locator =>
  group(page).locator(`[data-setting-title="${LABEL}"]`);
const toggle = (page: Page): Locator =>
  group(page).getByRole("checkbox", { name: LABEL, exact: true });
const clearButton = (page: Page): Locator =>
  group(page).getByRole("button", { name: CLEAR_BUTTON, exact: true });
const countLine = (page: Page): Locator =>
  group(page).getByRole("status").filter({ hasText: "Remembered" });

async function openRemember(
  page: Page,
  commands: Record<string, unknown>,
  settings: Record<string, unknown> = {},
): Promise<void> {
  await installTauriMock(page, {
    settings: { experimental_enabled: true, ...settings },
    commands,
  });
  await page.goto("/");
  await navItem(page, "Agents").click();
  await expect(section(page, "agents")).toBeVisible();
  const header = group(page).getByRole("button").first();
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(toggle(page)).toBeVisible();
}

async function invokedArgs(
  page: Page,
  command: string,
): Promise<Record<string, unknown>[]> {
  return getMockCalls(page, command);
}

test.describe("Remember recent conversations", () => {
  test("the toggle is on by default for a fresh install", async ({ page }) => {
    await openRemember(page, { conversation_memory_count: 0 });
    await expect(toggle(page)).toBeChecked();
  });

  test("the disclosure states that notes are stored on this Mac and may go to a remote service", async ({
    page,
  }) => {
    await openRemember(page, { conversation_memory_count: 0 });
    await row(page)
      .getByRole("button", { name: "More information", exact: true })
      .click();
    await expect(page.getByText(/which may be a remote service/)).toBeVisible();
    await expect(
      page.getByText(
        /short notes about your recent voice conversations on this Mac/,
      ),
    ).toBeVisible();
  });

  test("the disclosure says both uses send data to the configured model", async ({
    page,
  }) => {
    await openRemember(page, { conversation_memory_count: 0 });
    await row(page)
      .getByRole("button", { name: "More information", exact: true })
      .click();
    const tooltip = page.getByText(DESCRIPTION, { exact: true });
    await expect(tooltip).toBeVisible();
    // Use 1: the conversation is sent to the configured model when it ends, and the note
    // can carry an excerpt of what was said.
    expect(DESCRIPTION).toMatch(
      /when a conversation ends, what you said by voice and the replies you got .* are sent to that model to write a short note/,
    );
    expect(DESCRIPTION).toMatch(/which may be a remote service/);
    expect(DESCRIPTION).toMatch(
      /the note can include a short excerpt of what you said/,
    );
    // Use 2: the stored notes go to the model at the start of the next conversation.
    expect(DESCRIPTION).toMatch(
      /the newest notes are sent to the model at the start of your next voice conversation/,
    );
    expect(DESCRIPTION).toMatch(/on this Mac/);
    // Limits that were already true and must stay disclosed.
    expect(DESCRIPTION).toMatch(/at most the 20 newest notes/);
    expect(DESCRIPTION).toMatch(/not remembered at all/);
    expect(DESCRIPTION).toMatch(/use Clear remembered conversations below/);
  });

  test("turning it on calls change_conversation_memory_setting with enabled true", async ({
    page,
  }) => {
    // Start from OFF so the click turns it on.
    await openRemember(
      page,
      { conversation_memory_count: 0 },
      { conversation_memory_enabled: false },
    );
    await expect(toggle(page)).not.toBeChecked();
    await row(page).locator("label").first().click();
    await expect
      .poll(
        async () =>
          (await invokedArgs(page, "change_conversation_memory_setting"))
            .length,
      )
      .toBe(1);
    expect(
      await invokedArgs(page, "change_conversation_memory_setting"),
    ).toEqual([{ enabled: true }]);
  });

  test("with 3 remembered the line shows 3 and Clear is enabled", async ({
    page,
  }) => {
    await openRemember(page, { conversation_memory_count: 3 });
    await expect(countLine(page)).toHaveText(COUNT_3);
    await expect(clearButton(page)).toBeEnabled();
  });

  test("Clear asks for confirmation, then clears and shows 0 with the button disabled", async ({
    page,
  }) => {
    // The raw invoke value of a Result command is the Ok payload; the wrapper wraps it as
    // { status: "ok", data: 3 }.
    await openRemember(page, {
      conversation_memory_count: 3,
      clear_conversation_memory: 3,
    });
    await clearButton(page).click();
    await expect(
      group(page).getByText(CONFIRM_TEXT, { exact: true }),
    ).toBeVisible();
    expect(await invokedArgs(page, "clear_conversation_memory")).toEqual([]);

    // The backend count is 0 once the clear has run; the refresh after clear reads it.
    await setMockCommand(page, "conversation_memory_count", 0);
    await group(page)
      .getByRole("button", { name: CONFIRM_ACTION, exact: true })
      .click();

    await expect
      .poll(
        async () =>
          (await invokedArgs(page, "clear_conversation_memory")).length,
      )
      .toBe(1);
    await expect(countLine(page)).toHaveText(COUNT_0);
    await expect(clearButton(page)).toBeDisabled();
  });

  test("a failed clear shows the error, keeps the old count and leaves Clear usable", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: { conversation_memory_count: 3 },
    });
    // The real command returns Err("clear failed"); Tauri rejects the invoke with that string,
    // and the generated wrapper turns a non-Error rejection into { status: "error", error }.
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (
              cmd: string,
              args?: unknown,
              options?: unknown,
            ) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke;
      // Call through first so the mock still records the invoke, then reject.
      internals.invoke = (cmd, args, options) =>
        cmd === "clear_conversation_memory"
          ? original(cmd, args, options).then(() =>
              Promise.reject("clear failed"),
            )
          : original(cmd, args, options);
    });
    await page.goto("/");
    await navItem(page, "Agents").click();
    await expect(section(page, "agents")).toBeVisible();
    const header = group(page).getByRole("button").first();
    await header.click();
    await expect(header).toHaveAttribute("aria-expanded", "true");
    await expect(toggle(page)).toBeVisible();
    await expect(countLine(page)).toHaveText(COUNT_3);

    await clearButton(page).click();
    await group(page)
      .getByRole("button", { name: CONFIRM_ACTION, exact: true })
      .click();

    await expect(
      group(page).getByRole("alert").filter({ hasText: CLEAR_FAILED }),
    ).toBeVisible();
    await expect
      .poll(
        async () =>
          (await invokedArgs(page, "clear_conversation_memory")).length,
      )
      .toBe(1);
    await expect(countLine(page)).toHaveText(COUNT_3);
    await expect(clearButton(page)).toBeEnabled();
  });

  test("a failing count command still renders the group", async ({ page }) => {
    await installTauriMock(page, {
      settings: { experimental_enabled: true },
      commands: {},
    });
    // Make only the count command reject, after the mock is installed.
    await page.addInitScript(() => {
      const internals = (
        window as unknown as {
          __TAURI_INTERNALS__: {
            invoke: (
              cmd: string,
              args?: unknown,
              options?: unknown,
            ) => Promise<unknown>;
          };
        }
      ).__TAURI_INTERNALS__;
      const original = internals.invoke;
      internals.invoke = (cmd, args, options) =>
        cmd === "conversation_memory_count"
          ? Promise.reject(new Error("count failed"))
          : original(cmd, args, options);
    });
    await page.goto("/");
    await navItem(page, "Agents").click();
    await expect(section(page, "agents")).toBeVisible();
    const header = group(page).getByRole("button").first();
    await header.click();
    await expect(header).toHaveAttribute("aria-expanded", "true");
    await expect(toggle(page)).toBeVisible();
    await expect(countLine(page)).toHaveCount(0);
    await expect(clearButton(page)).toBeVisible();
  });
});
