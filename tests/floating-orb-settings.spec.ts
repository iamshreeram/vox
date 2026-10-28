import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";

/*
 * G3c SPEC: Settings > General > Conversation > "Floating orb" (DESIGN.md 'Frontend' ->
 * Settings UI). Mounted against the Vite dev server with the Tauri mock. Written first; workers
 * must not edit this file.
 *
 * Stable selectors (data-testid), each on the element named:
 *   floating-orb-group    container of the Floating orb rows, inside the Conversation card
 *                         (not a collapsible of its own: only the Conversation card is expanded)
 *   floating-orb-show     the "Show orb" checkbox <input type="checkbox">
 *   floating-orb-size     the size <select> with options Small, Medium, Large (values small|medium|large)
 *   floating-orb-reset    the "Reset position" <button>
 *   floating-orb-reason   the text shown when a requested orb resolves to unsupported
 *
 * Commands: get_orb_support (query on mount), change_orb_visible_setting {enabled},
 * change_orb_size_setting {size}, reset_orb_position {}. Nothing else may touch the orb.
 */

const SUPPORTED = { supported: true, block: null };
const WAYLAND = { supported: false, block: "wayland" };

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

const tid = (page: Page, id: string): Locator => page.getByTestId(id);
const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, label: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${label}$`) });
const conversationCard = (page: Page): Locator =>
  page.locator('[data-group-id="conversation"]');
const options = (page: Page): Locator => page.getByRole("option");
const searchBox = (page: Page): Locator =>
  page.getByRole("combobox", { name: /search settings/i });

async function openConversation(
  page: Page,
  settings: Record<string, unknown>,
  commands: Record<string, unknown> = { get_orb_support: SUPPORTED },
): Promise<void> {
  await installTauriMock(page, { settings, commands });
  await page.goto("/");
  await navItem(page, "General").click();
  await expect(section(page, "general")).toBeVisible();
  const header = conversationCard(page).getByRole("button").first();
  await header.click();
  await expect(header).toHaveAttribute("aria-expanded", "true");
  await expect(tid(page, "floating-orb-group")).toBeVisible();
}

async function invokedCommands(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const mock = (
      window as unknown as {
        __voxMock: { calls: Array<{ command: string }> };
      }
    ).__voxMock;
    return mock.calls.map((call) => call.command);
  });
}

async function orbCommands(page: Page): Promise<string[]> {
  return (await invokedCommands(page)).filter((name) => /orb/i.test(name));
}

test.describe("Floating orb settings", () => {
  test("1a. Show orb is off when orb_visible is unset (null)", async ({
    page,
  }) => {
    await openConversation(page, { orb_visible: null });
    await expect(tid(page, "floating-orb-show")).not.toBeChecked();
    await expect(tid(page, "floating-orb-show")).toBeEnabled();
  });

  test("1b. Show orb is off when orb_visible is false, and turning it on records the change", async ({
    page,
  }) => {
    await openConversation(page, { orb_visible: false });
    await expect(tid(page, "floating-orb-show")).not.toBeChecked();
    await tid(page, "floating-orb-show").check();
    await expect(tid(page, "floating-orb-show")).toBeChecked();
    const calls = await getMockCalls(page, "change_orb_visible_setting");
    expect(calls.at(-1)).toEqual({ enabled: true });
  });

  test("1c. Show orb is on when orb_visible is true", async ({ page }) => {
    await openConversation(page, { orb_visible: true });
    await expect(tid(page, "floating-orb-show")).toBeChecked();
    await expect(tid(page, "floating-orb-show")).toBeEnabled();
  });

  test("2. the size control offers Small, Medium, Large and records the choice", async ({
    page,
  }) => {
    await openConversation(page, { orb_size: "small" });
    const size = tid(page, "floating-orb-size");
    await expect(size).toHaveValue("small");
    await expect(size.locator("option")).toHaveText([
      text("settings.floatingOrb.size.small"),
      text("settings.floatingOrb.size.medium"),
      text("settings.floatingOrb.size.large"),
    ]);
    await size.selectOption("large");
    const calls = await getMockCalls(page, "change_orb_size_setting");
    expect(calls.at(-1)).toEqual({ size: "large" });
  });

  test("2b. a missing orb_size shows Medium", async ({ page }) => {
    await openConversation(page, {});
    await expect(tid(page, "floating-orb-size")).toHaveValue("medium");
  });

  test("3a. Reset position is disabled when no position is saved", async ({
    page,
  }) => {
    await openConversation(page, { orb_position: null });
    await expect(tid(page, "floating-orb-reset")).toBeDisabled();
  });

  test("3b. Reset position is enabled and records reset_orb_position", async ({
    page,
  }) => {
    await openConversation(page, {
      orb_position: {
        monitor_name: "Built-in",
        monitor_key: "Built-in|0|0|1920|1080",
        fx: 0.9,
        fy: 0.8,
      },
    });
    const reset = tid(page, "floating-orb-reset");
    await expect(reset).toBeEnabled();
    await reset.click();
    expect(
      (await invokedCommands(page)).filter((c) => c === "reset_orb_position"),
    ).toHaveLength(1);
  });

  test("4. an unsupported platform disables the toggle, leaves it unchecked and names the reason", async ({
    page,
  }) => {
    await openConversation(
      page,
      { orb_visible: true },
      { get_orb_support: WAYLAND },
    );
    await expect(tid(page, "floating-orb-show")).toBeDisabled();
    await expect(tid(page, "floating-orb-show")).not.toBeChecked();
    const reason = tid(page, "floating-orb-reason");
    await expect(reason).toBeVisible();
    await expect(reason).toContainText(
      text("settings.floatingOrb.reason.wayland"),
    );
  });

  test("5. mounting the group invokes only get_orb_support", async ({
    page,
  }) => {
    await openConversation(page, { orb_visible: true });
    await expect.poll(() => orbCommands(page)).toContain("get_orb_support");
    expect(new Set(await orbCommands(page))).toEqual(
      new Set(["get_orb_support"]),
    );
  });

  test("6. settings search for 'orb' finds the controls and opens them", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { orb_visible: false },
      commands: { get_orb_support: SUPPORTED },
    });
    await page.goto("/");
    await searchBox(page).fill("orb");
    const hit = options(page).filter({
      hasText: text("settings.floatingOrb.show.title"),
    });
    await expect(hit.first()).toBeVisible();
    await hit.first().click();
    await expect(section(page, "general")).toBeVisible();
    await expect(tid(page, "floating-orb-show")).toBeVisible();
    await expect(tid(page, "floating-orb-group")).toBeVisible();
  });
});
