import { test, expect, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * Footer notice for a failed automatic update (DESIGN.md R10 + R11).
 * Runs against the Vite dev server with the Tauri mock. The notice is a
 * role=alert in the footer and must show without opening Settings or Debug.
 * Written before the production code: these tests are expected to FAIL now.
 */

const NARROW = { width: 760, height: 600 };
const MIN_WINDOW = { width: 480, height: 600 };
const AUTO_STATUS_EVENT = "auto-update-state-changed";
const DISMISS_KEY = "vox.autoUpdate.dismissedFailureTs";
const FAILED_TEXT = "Last automatic update to abc1234 failed.";
const BUILD_FAILED_TEXT = "The build failed.";
const SILENT_TEXT =
  /Preparing an update in the background|Update ready; installing when Vox is idle|Installing the update|Last automatic update/;

const UPDATE_INFO = {
  state: "available",
  current_commit: "1111111",
  latest_commit: "2222222",
  latest_full: "2".repeat(40),
  behind_by: 3,
  changes: ["Fix the thing"],
  compare_url: "https://github.com/iamshreeram/vox/compare/1111111...2222222",
  note: null,
  can_install: true,
  install_blocker: null,
  auto_pending: false,
};

// Manual updater reports the installed build as current.
const MANUAL_UP_TO_DATE = {
  ...UPDATE_INFO,
  state: "up_to_date",
  current_commit: "2222222",
  behind_by: 0,
  changes: [],
  can_install: false,
};

const AUTO_STATUS_IDLE = {
  enabled: false,
  effective: false,
  phase: "idle",
  last_result: "none",
  reason_code: null,
  target_short: null,
  ts: null,
};

type AutoStatus = Record<string, unknown>;

function failedStatus(overrides: AutoStatus = {}): AutoStatus {
  return {
    ...AUTO_STATUS_IDLE,
    enabled: true,
    effective: true,
    last_result: "failed",
    reason_code: "build_failed",
    target_short: "abc1234",
    ts: 1700000000,
    ...overrides,
  };
}

interface OpenOptions {
  viewport?: { width: number; height: number };
  settings?: Record<string, unknown>;
  commands?: Record<string, unknown>;
}

/** Opens the app and waits for the footer update controls (no Settings/Debug). */
async function openFooter(
  page: Page,
  options: OpenOptions = {},
): Promise<void> {
  await page.setViewportSize(options.viewport ?? NARROW);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      update_checks_enabled: true,
      debug_mode: true,
      auto_update_enabled: true,
      ...options.settings,
    },
    commands: {
      get_update_mode: "source",
      is_update_checks_locked: false,
      check_source_update: UPDATE_INFO,
      get_cached_source_update: UPDATE_INFO,
      get_source_update_install_blocker: null,
      get_auto_update_status: AUTO_STATUS_IDLE,
      change_auto_update_setting: null,
      "plugin:app|version": "0.9.9",
      ...options.commands,
    },
  });
  await page.goto("/");
  await expect(
    page.getByRole("button", {
      name: /^(Check for updates|Update available)$/,
    }),
  ).toBeVisible();
}

function footerAlert(page: Page) {
  return page.getByRole("alert");
}

function dismissButton(page: Page) {
  return footerAlert(page).getByRole("button", {
    name: "Dismiss",
    exact: true,
  });
}

/** Moves focus to Dismiss with Tab only (bounded so a missing control fails fast). */
async function tabToDismiss(page: Page): Promise<void> {
  const dismiss = dismissButton(page);
  for (let press = 0; press < 80; press += 1) {
    if (await dismiss.evaluate((el) => el === document.activeElement)) return;
    await page.keyboard.press("Tab");
  }
  throw new Error("Dismiss was not reached with Tab");
}

async function readDismissedTs(page: Page): Promise<string | null> {
  return page.evaluate((key) => localStorage.getItem(key), DISMISS_KEY);
}

test.describe("automatic update failure notice in the footer", () => {
  test("a failed automatic update shows an alert without opening Settings", async ({
    page,
  }) => {
    await openFooter(page);

    await emitMockEvent(page, AUTO_STATUS_EVENT, failedStatus());

    const alert = footerAlert(page);
    await expect(alert).toBeVisible();
    await expect(alert).toContainText(FAILED_TEXT);
    await expect(alert).toContainText(BUILD_FAILED_TEXT);
    // Settings > Debug was never opened.
    await expect(
      page.getByRole("checkbox", {
        name: "Install updates automatically",
        exact: true,
      }),
    ).toHaveCount(0);
  });

  test("a failure recorded before the app loaded is shown after hydration", async ({
    page,
  }) => {
    await openFooter(page, {
      commands: { get_auto_update_status: failedStatus() },
    });

    await expect(footerAlert(page)).toBeVisible();
    await expect(footerAlert(page)).toContainText(FAILED_TEXT);
    await expect(footerAlert(page)).toContainText(BUILD_FAILED_TEXT);
  });

  test("a failure without a target says the last automatic update failed", async ({
    page,
  }) => {
    await openFooter(page);

    await emitMockEvent(
      page,
      AUTO_STATUS_EVENT,
      failedStatus({ target_short: null }),
    );

    const alert = footerAlert(page);
    await expect(alert).toBeVisible();
    await expect(alert).toContainText(
      "Last automatic update failed. The build failed.",
    );
    await expect(alert).not.toContainText("Last automatic update to");
  });

  // Each case first shows a failure (so the test fails for the missing alert,
  // not vacuously), then a non-failure result must hide it.
  const HIDDEN_CASES: { name: string; status: AutoStatus }[] = [
    {
      name: "skipped",
      status: { last_result: "skipped", reason_code: "no_update" },
    },
    { name: "updated", status: { last_result: "updated" } },
    {
      name: "none",
      status: { last_result: "none", reason_code: null, target_short: null },
    },
    { name: "building phase", status: { phase: "building" } },
    { name: "waiting phase", status: { phase: "waiting" } },
    { name: "installing phase", status: { phase: "installing" } },
    { name: "feature disabled", status: { enabled: false, effective: false } },
  ];

  for (const hiddenCase of HIDDEN_CASES) {
    test(`no alert when the result is ${hiddenCase.name}`, async ({ page }) => {
      await openFooter(page);

      await emitMockEvent(
        page,
        AUTO_STATUS_EVENT,
        failedStatus({ ts: 1700000000 }),
      );
      await expect(footerAlert(page)).toBeVisible();

      await emitMockEvent(
        page,
        AUTO_STATUS_EVENT,
        failedStatus({ ...hiddenCase.status, ts: 1700000100 }),
      );
      await expect(footerAlert(page)).toHaveCount(0);
    });
  }

  test("Dismiss hides the notice, stays hidden after reload, and returns for a newer failure", async ({
    page,
  }) => {
    await openFooter(page, {
      commands: { get_auto_update_status: failedStatus() },
    });
    const alert = footerAlert(page);
    await expect(alert).toBeVisible();

    await dismissButton(page).click();
    await expect(alert).toHaveCount(0);
    await expect.poll(() => readDismissedTs(page)).toBe("1700000000");

    await page.reload();
    await expect(
      page.getByRole("button", {
        name: /^(Check for updates|Update available)$/,
      }),
    ).toBeVisible();
    await expect(alert).toHaveCount(0);

    await emitMockEvent(
      page,
      AUTO_STATUS_EVENT,
      failedStatus({ ts: 1700000500 }),
    );
    await expect(alert).toBeVisible();
    await expect(alert).toContainText(FAILED_TEXT);
  });

  test("keyboard: Tab reaches Dismiss and Enter dismisses the notice", async ({
    page,
  }) => {
    await openFooter(page);
    await emitMockEvent(page, AUTO_STATUS_EVENT, failedStatus());
    await expect(footerAlert(page)).toBeVisible();

    await tabToDismiss(page);
    await expect(dismissButton(page)).toBeFocused();
    await page.keyboard.press("Enter");

    await expect(footerAlert(page)).toHaveCount(0);
    await expect.poll(() => readDismissedTs(page)).toBe("1700000000");
  });

  test("a successful manual check does not clear the automatic failure wording", async ({
    page,
  }) => {
    // Historical wording is intentional (issue #154): the alert names the automatic attempt.
    await openFooter(page, {
      commands: {
        get_auto_update_status: failedStatus(),
        get_cached_source_update: MANUAL_UP_TO_DATE,
        check_source_update: MANUAL_UP_TO_DATE,
      },
    });

    await expect(page.getByText(/0\.9\.9/).first()).toBeVisible();
    await expect(footerAlert(page)).toBeVisible();
    await expect(footerAlert(page)).toContainText(FAILED_TEXT);
  });

  // Silent states never put text in the footer (one fresh page per case).
  const SILENT_CASES: { name: string; status: AutoStatus }[] = [
    { name: "building", status: { phase: "building" } },
    { name: "waiting", status: { phase: "waiting" } },
    { name: "installing", status: { phase: "installing" } },
    { name: "updated", status: { last_result: "updated" } },
  ];

  for (const silentCase of SILENT_CASES) {
    test(`footer shows no automatic update text while ${silentCase.name}`, async ({
      page,
    }) => {
      await openFooter(page);
      // Control: a failure is shown first, so the test is red until the notice exists.
      await emitMockEvent(
        page,
        AUTO_STATUS_EVENT,
        failedStatus({ ts: 1700000000 }),
      );
      await expect(footerAlert(page)).toBeVisible();

      await emitMockEvent(
        page,
        AUTO_STATUS_EVENT,
        failedStatus({ ...silentCase.status, ts: 1700000100 }),
      );
      await expect(footerAlert(page)).toHaveCount(0);
      await expect(page.getByText(SILENT_TEXT)).toHaveCount(0);
    });
  }

  // Layout at the narrowest supported window and the 760 px width (R11 C1).
  const LAYOUT_VIEWPORTS = [
    { name: "480 px minimum window", size: MIN_WINDOW },
    { name: "760 px window", size: NARROW },
  ];
  const LAYOUT_REASONS = [
    // Unknown code falls back to the generic reason.
    { name: "unknown reason code", reason_code: "something_new" },
    // verify_failed is the longest English reason string.
    { name: "longest English reason", reason_code: "verify_failed" },
  ];

  for (const viewport of LAYOUT_VIEWPORTS) {
    for (const reason of LAYOUT_REASONS) {
      test(`footer notice fits at ${viewport.name} with ${reason.name}`, async ({
        page,
      }) => {
        await openFooter(page, {
          viewport: viewport.size,
          commands: {
            get_auto_update_status: failedStatus({
              reason_code: reason.reason_code,
            }),
          },
        });

        const alert = footerAlert(page);
        await expect(alert).toBeVisible();
        await expect(alert).toContainText(FAILED_TEXT);

        const widths = await page.evaluate(() => ({
          scrollWidth: document.documentElement.scrollWidth,
          innerWidth: window.innerWidth,
        }));
        expect(widths.scrollWidth).toBeLessThanOrEqual(widths.innerWidth);

        const dismiss = dismissButton(page);
        await expect(dismiss).toBeVisible();
        const box = await dismiss.boundingBox();
        expect(box, "Dismiss has a bounding box").not.toBeNull();
        if (box === null) return;
        expect(box.x).toBeGreaterThanOrEqual(0);
        expect(box.y).toBeGreaterThanOrEqual(0);
        expect(box.x + box.width).toBeLessThanOrEqual(viewport.size.width);
        expect(box.y + box.height).toBeLessThanOrEqual(viewport.size.height);

        await tabToDismiss(page);
        await page.keyboard.press("Enter");
        await expect(footerAlert(page)).toHaveCount(0);
      });
    }
  }
});
