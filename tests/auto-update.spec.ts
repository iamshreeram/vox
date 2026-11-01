import { test, expect, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * Automatic source update (Settings switch, status line, silent footer rules).
 * Runs against the Vite dev server with the Tauri mock; backend events arrive
 * through the mock event bus.
 */

const NARROW = { width: 760, height: 600 };
const AUTO_LABEL = "Install updates automatically";

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

const AUTO_STATUS_IDLE = {
  enabled: false,
  effective: false,
  phase: "idle",
  last_result: "none",
  reason_code: null,
  target_short: null,
  ts: null,
};

const SILENT_RUN = "s".repeat(16);

interface OpenOptions {
  settings?: Record<string, unknown>;
  commands?: Record<string, unknown>;
}

async function openApp(page: Page, options: OpenOptions = {}): Promise<void> {
  await page.setViewportSize(NARROW);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      update_checks_enabled: true,
      debug_mode: true,
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
  await expect(page.locator("p", { hasText: /^General$/ })).toBeVisible();
}

async function openAbout(page: Page): Promise<void> {
  await page.locator("p", { hasText: /^About$/ }).click();
  await expect(switchInput(page)).toBeVisible();
}

function switchInput(page: Page) {
  return page.getByRole("checkbox", { name: AUTO_LABEL, exact: true });
}

function statusLine(page: Page, text: RegExp | string) {
  return page.getByRole("status").filter({ hasText: text });
}

test.describe("automatic source update settings", () => {
  test("the switch is off by default and calls the command with the new value", async ({
    page,
  }) => {
    await openApp(page);
    await openAbout(page);

    const toggle = switchInput(page);
    await expect(toggle).not.toBeChecked();
    await expect(toggle).toBeEnabled();

    // The input is visually hidden under its track; click the wrapping label.
    const track = page.locator("label", { has: toggle });
    await track.click();
    await expect(toggle).toBeChecked();
    await expect
      .poll(async () => getMockCalls(page, "change_auto_update_setting"))
      .toEqual([{ enabled: true }]);

    await track.click();
    await expect(toggle).not.toBeChecked();
    await expect
      .poll(async () => getMockCalls(page, "change_auto_update_setting"))
      .toEqual([{ enabled: true }, { enabled: false }]);
  });

  test("the switch is disabled while update checks are off", async ({
    page,
  }) => {
    await openApp(page, { settings: { update_checks_enabled: false } });
    await openAbout(page);

    await expect(switchInput(page)).toBeDisabled();
    await expect(switchInput(page)).not.toBeChecked();
  });

  test("the switch is disabled when checks are forced off by configuration", async ({
    page,
  }) => {
    await openApp(page, { commands: { is_update_checks_locked: true } });
    await openAbout(page);

    await expect(switchInput(page)).toBeDisabled();
  });

  test("the switch is disabled in a build that is not a source build", async ({
    page,
  }) => {
    await openApp(page, { commands: { get_update_mode: "plugin" } });
    await openAbout(page);

    await expect(switchInput(page)).toBeDisabled();
  });

  test("the switch can be operated from the keyboard", async ({ page }) => {
    await openApp(page);
    await openAbout(page);

    const toggle = switchInput(page);
    await toggle.focus();
    await expect(toggle).toBeFocused();
    await page.keyboard.press("Space");

    await expect
      .poll(async () => getMockCalls(page, "change_auto_update_setting"))
      .toEqual([{ enabled: true }]);
  });
});

test.describe("automatic source update status line", () => {
  test("a failed attempt shows its reason text, not raw backend text", async ({
    page,
  }) => {
    await openApp(page, {
      settings: { auto_update_enabled: true },
      commands: {
        get_auto_update_status: {
          ...AUTO_STATUS_IDLE,
          enabled: true,
          effective: true,
          last_result: "failed",
          reason_code: "verify_failed",
          target_short: "abc12345",
          ts: 1700000000,
        },
      },
    });
    await openAbout(page);

    await expect(
      statusLine(
        page,
        "Last automatic update did not finish. The newest version could not be verified on GitHub.",
      ),
    ).toBeVisible();
  });

  test("an unknown reason code falls back to the generic reason", async ({
    page,
  }) => {
    await openApp(page, {
      settings: { auto_update_enabled: true },
      commands: {
        get_auto_update_status: {
          ...AUTO_STATUS_IDLE,
          enabled: true,
          effective: true,
          last_result: "skipped",
          reason_code: "something_new",
        },
      },
    });
    await openAbout(page);

    await expect(
      statusLine(page, "Last automatic update was skipped. Unknown reason."),
    ).toBeVisible();
  });

  test("an updated attempt shows the target and its date", async ({ page }) => {
    await openApp(page, {
      settings: { auto_update_enabled: true },
      commands: {
        get_auto_update_status: {
          ...AUTO_STATUS_IDLE,
          enabled: true,
          effective: true,
          last_result: "updated",
          target_short: "abc12345",
          ts: 1700000000,
        },
      },
    });
    await openAbout(page);

    await expect(
      statusLine(page, /^Last automatic update: abc12345 \(.+\)$/),
    ).toBeVisible();
  });

  test("the status changes when the backend event arrives", async ({
    page,
  }) => {
    await openApp(page, { settings: { auto_update_enabled: true } });
    await openAbout(page);

    await emitMockEvent(page, "auto-update-state-changed", {
      ...AUTO_STATUS_IDLE,
      enabled: true,
      effective: true,
      phase: "building",
    });
    await expect(
      statusLine(page, "Preparing an update in the background"),
    ).toBeVisible();

    await emitMockEvent(page, "auto-update-state-changed", {
      ...AUTO_STATUS_IDLE,
      enabled: true,
      effective: true,
      phase: "waiting",
    });
    await expect(
      statusLine(page, "Update ready; installing when Vox is idle"),
    ).toBeVisible();
  });

  test("nothing is shown while the feature is off and never ran", async ({
    page,
  }) => {
    await openApp(page);
    await openAbout(page);

    await expect(
      page.getByRole("status").filter({ hasText: /\S/ }),
    ).toHaveCount(0);
  });
});

test.describe("automatic source update footer rules", () => {
  test("silent progress does not show progress or failure in the footer", async ({
    page,
  }) => {
    await openApp(page);

    await emitMockEvent(page, "source-update-progress", {
      run_id: SILENT_RUN,
      phase: "compiling",
      percent: 40,
      target: "abc1234",
      message: "",
      estimated: false,
      silent: true,
    });
    await emitMockEvent(page, "source-update-progress", {
      run_id: SILENT_RUN,
      phase: "failed",
      percent: 0,
      target: "abc1234",
      message: "build broke",
      estimated: false,
      silent: true,
    });

    await expect(page.getByRole("progressbar")).toHaveCount(0);
    await expect(page.getByText("build broke")).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Update available" }),
    ).toBeVisible();
  });

  test("an update the automatic run is handling does not show as available", async ({
    page,
  }) => {
    await openApp(page, {
      commands: {
        get_cached_source_update: { ...UPDATE_INFO, auto_pending: true },
        check_source_update: { ...UPDATE_INFO, auto_pending: true },
      },
    });

    await expect(
      page.getByRole("button", { name: "Update available" }),
    ).toHaveCount(0);
    await expect(
      page.getByRole("button", { name: "Check for updates" }),
    ).toBeVisible();
  });
});
