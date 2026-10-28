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
 *   floating-orb-size-slider     <input type="range"> min 48, max 192, step 4 (Revision 6: free size)
 *   floating-orb-size-value      the numeric label, e.g. "96 px"
 *   floating-orb-size-small/-medium/-large  preset buttons (72 / 96 / 128), aria-pressed="true"
 *                                when the effective px equals the preset
 *   floating-orb-size-reset      the "Reset size" <button>, disabled when orb_size_px is null
 *   floating-orb-reset    the "Reset position" <button>
 *   floating-orb-reason   the text shown when a requested orb resolves to unsupported
 *
 * Commands: get_orb_support (query on mount), change_orb_visible_setting {enabled},
 * change_orb_size_px_setting {px} (debounced 150 ms, last value wins; null on reset),
 * reset_orb_position {}. Nothing else may touch the orb. The old select and
 * change_orb_size_setting are gone from the UI (deliberate Revision 6 change).
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

/** The argument object of the latest change_orb_size_px_setting call, or undefined. */
async function lastPxCall(page: Page): Promise<unknown> {
  const calls = await getMockCalls(page, "change_orb_size_px_setting");
  return calls.at(-1);
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

  test("2a. the slider shows the effective px from orb_size_px, with the range and label", async ({
    page,
  }) => {
    await openConversation(page, { orb_size: "small", orb_size_px: 140 });
    const slider = tid(page, "floating-orb-size-slider");
    await expect(slider).toHaveValue("140");
    await expect(slider).toHaveAttribute("min", "48");
    await expect(slider).toHaveAttribute("max", "192");
    await expect(slider).toHaveAttribute("step", "4");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("140 px");
    await expect(tid(page, "floating-orb-size-small")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await expect(tid(page, "floating-orb-size-reset")).toBeEnabled();
  });

  test("2b. without orb_size_px the slider shows the orb_size preset and reset is disabled", async ({
    page,
  }) => {
    await openConversation(page, { orb_size: "large", orb_size_px: null });
    await expect(tid(page, "floating-orb-size-slider")).toHaveValue("128");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("128 px");
    await expect(tid(page, "floating-orb-size-large")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(tid(page, "floating-orb-size-reset")).toBeDisabled();
  });

  test("2c. a missing orb_size and no px shows Medium, 96 px", async ({
    page,
  }) => {
    await openConversation(page, {});
    await expect(tid(page, "floating-orb-size-slider")).toHaveValue("96");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("96 px");
    await expect(tid(page, "floating-orb-size-medium")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  test("2d. moving the slider previews at once and records one change after release", async ({
    page,
  }) => {
    // Deterministic debounce: the page clock is frozen once the page is up, so the 150 ms quiet
    // period ends only when this test advances the clock (no real-time race with the keypresses).
    await page.clock.install({ time: new Date("2026-01-01T00:00:00Z") });
    await openConversation(page, { orb_size: "medium", orb_size_px: null });
    await page.clock.pauseAt(new Date("2026-01-01T00:01:00Z"));
    const slider = tid(page, "floating-orb-size-slider");
    await slider.focus();
    for (let i = 0; i < 11; i += 1) {
      await slider.press("ArrowRight"); // 96 -> 140
    }
    await expect(slider).toHaveValue("140");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("140 px");
    // Still moving: the 150 ms debounce has not released the value, so nothing is recorded yet.
    expect(await getMockCalls(page, "change_orb_size_px_setting")).toHaveLength(
      0,
    );
    // Released: exactly one call, carrying the final value.
    await page.clock.runFor(150);
    await expect
      .poll(() => getMockCalls(page, "change_orb_size_px_setting"))
      .toEqual([{ px: 140 }]);
  });

  test("2e. ten rapid ArrowRight presses commit only the last value, once", async ({
    page,
  }) => {
    await openConversation(page, { orb_size: "medium", orb_size_px: null });
    const slider = tid(page, "floating-orb-size-slider");
    await slider.focus();
    for (let i = 0; i < 10; i += 1) {
      await slider.press("ArrowRight"); // 96 -> 136
    }
    await expect(slider).toHaveValue("136");
    await expect
      .poll(() => getMockCalls(page, "change_orb_size_px_setting"))
      .toEqual([{ px: 136 }]);
  });

  test("2f. the preset buttons record 72, 128 and 96", async ({ page }) => {
    await openConversation(page, { orb_size: "medium", orb_size_px: null });
    await tid(page, "floating-orb-size-small").click();
    await expect.poll(() => lastPxCall(page)).toEqual({ px: 72 });
    await tid(page, "floating-orb-size-large").click();
    await expect.poll(() => lastPxCall(page)).toEqual({ px: 128 });
    await tid(page, "floating-orb-size-medium").click();
    await expect.poll(() => lastPxCall(page)).toEqual({ px: 96 });
  });

  test("2g. Reset size records px null", async ({ page }) => {
    await openConversation(page, { orb_size: "medium", orb_size_px: 140 });
    await tid(page, "floating-orb-size-reset").click();
    await expect.poll(() => lastPxCall(page)).toEqual({ px: null });
  });

  test("2h. the value never leaves 48 to 192", async ({ page }) => {
    await openConversation(page, { orb_size: "large", orb_size_px: 192 });
    const slider = tid(page, "floating-orb-size-slider");
    await slider.focus();
    await slider.press("End");
    await slider.press("ArrowRight");
    await expect(slider).toHaveValue("192");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("192 px");
    await slider.press("Home");
    await expect(slider).toHaveValue("48");
    await expect(tid(page, "floating-orb-size-value")).toHaveText("48 px");
    await expect.poll(() => lastPxCall(page)).toEqual({ px: 48 });
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
