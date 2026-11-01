import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./fixtures/tauriMock";

/*
 * Footer behaviour when a source-update CHECK fails (pins existing behaviour).
 * Runs against the Vite dev server with the Tauri mock.
 *
 * - A check that resolves with the backend error result shows the generic
 *   footer text, and the backend sentence is the tooltip and accessible name.
 * - A check whose command throws shows the generic English
 *   footer.sourceUpdate.error text ("Update check failed").
 */

const NARROW = { width: 760, height: 600 };

const BACKEND_SENTENCE =
  "Could not reach GitHub to check for updates. Check your connection.";
const GENERIC_ERROR = "Update check failed";

/**
 * Opens the app with no cached update and a failing check. The mount-time check
 * is silent on failure, so the footer starts at "Check for updates" and only a
 * manual click surfaces the error.
 */
async function open(
  page: Page,
  checkCommand: Record<string, unknown> = {},
  extraInit?: () => void,
  extraArg?: string,
): Promise<void> {
  await page.setViewportSize(NARROW);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      update_checks_enabled: true,
    },
    commands: {
      get_update_mode: "source",
      is_update_checks_locked: false,
      get_cached_source_update: null,
      get_source_update_install_blocker: null,
      "plugin:app|version": "0.9.9",
      ...checkCommand,
    },
  });
  if (extraInit) await page.addInitScript(extraInit, extraArg);
  await page.goto("/");
  await expect(page.locator("p", { hasText: /^General$/ })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Check for updates" }),
  ).toBeVisible();
}

/** Runs in the page: rejects check_source_update with a plain string (Err(String)). */
function rejectCheckWithString(sentence: string): void {
  const internals = (
    window as unknown as {
      __TAURI_INTERNALS__: { invoke: (...a: unknown[]) => Promise<unknown> };
    }
  ).__TAURI_INTERNALS__;
  const original = internals.invoke;
  internals.invoke = (command: unknown, ...rest: unknown[]) =>
    command === "check_source_update"
      ? Promise.reject(sentence)
      : original(command, ...rest);
}

test.describe("source update check failure footer", () => {
  test("a backend error result shows the generic footer text; the backend sentence is the tooltip and accessible name", async ({
    page,
  }) => {
    // Tauri rejects a command with a plain string for Err(String); the generated
    // bindings turn that into { status: "error", error }. The shared mock only
    // rejects with Error objects, so this init script rejects with the string.
    await open(page, {}, rejectCheckWithString, BACKEND_SENTENCE);

    await page.getByRole("button", { name: "Check for updates" }).click();

    // Visible text stays the generic label; the specific reason is exposed as
    // the tooltip and as the control's accessible name.
    await expect(page.getByText(GENERIC_ERROR, { exact: true })).toBeVisible();
    await expect(
      page.getByTitle(BACKEND_SENTENCE, { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: BACKEND_SENTENCE, exact: true }),
    ).toBeVisible();
  });

  test("a check that throws shows the generic footer error text", async ({
    page,
  }) => {
    await open(page, {
      check_source_update: { __error__: "boom: internal detail" },
    });

    await page.getByRole("button", { name: "Check for updates" }).click();

    await expect(page.getByText(GENERIC_ERROR, { exact: true })).toBeVisible();
    await expect(page.getByText("boom: internal detail")).toHaveCount(0);
  });
});
