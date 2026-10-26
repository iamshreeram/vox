import { mkdirSync } from "node:fs";
import path from "node:path";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { emitMockEvent, installTauriMock } from "./fixtures/tauriMock";

/*
 * Approval dialog for agent tool calls (PermissionPrompt). Mounted tests against the Vite dev
 * server with the Tauri mock. Calls to acp_permission_respond are recorded by an init script
 * that wraps __TAURI_INTERNALS__.invoke, so each test can assert arguments and counts.
 * Screenshots are written only when SHOTS_DIR is set.
 */

interface PermissionRequest {
  token: string;
  title: string;
  kind: string;
  details: string | null;
  truncated: boolean;
  timeoutSecs: number;
  queued: number;
}

interface RecordingWindow {
  __TAURI_INTERNALS__: {
    invoke: (
      command: string,
      args?: unknown,
      options?: unknown,
    ) => Promise<unknown>;
  };
  __pwInvokes: { command: string; args: unknown }[];
}

const REQUEST_EVENT = "acp-permission-request";
const CLOSED_EVENT = "acp-permission-closed";
const RESPOND_COMMAND = "acp_permission_respond";

function request(
  overrides: Partial<PermissionRequest> = {},
): PermissionRequest {
  return {
    token: "t1",
    title: "Run tests",
    kind: "execute",
    details: "command: cargo test",
    truncated: false,
    timeoutSecs: 60,
    queued: 0,
    ...overrides,
  };
}

const dialog = (page: Page): Locator => page.getByRole("alertdialog");
const denyButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Deny", exact: true });
const allowButton = (page: Page): Locator =>
  page.getByRole("button", { name: "Allow once", exact: true });

/** Mock first, then a recorder that wraps the mock's invoke (init scripts run in order). */
async function installPermissionHarness(page: Page): Promise<void> {
  await installTauriMock(page);
  await page.addInitScript((command: string) => {
    const win = window as unknown as RecordingWindow;
    win.__pwInvokes = [];
    const original = win.__TAURI_INTERNALS__.invoke;
    win.__TAURI_INTERNALS__.invoke = (name, args, options) => {
      if (name === command) {
        win.__pwInvokes.push({ command: name, args });
      }
      return original(name, args, options);
    };
  }, RESPOND_COMMAND);
}

async function respondCalls(
  page: Page,
): Promise<{ command: string; args: unknown }[]> {
  return page.evaluate(
    () => (window as unknown as RecordingWindow).__pwInvokes,
  );
}

/**
 * The app registers its listeners after mount, so re-deliver until the dialog shows.
 * The component ignores a repeated token, so re-delivery is safe.
 */
async function showRequest(
  page: Page,
  payload: PermissionRequest,
): Promise<void> {
  await expect(async () => {
    await emitMockEvent(page, REQUEST_EVENT, payload);
    await expect(dialog(page)).toBeVisible({ timeout: 500 });
  }).toPass({ timeout: 15000 });
}

test.describe("Permission prompt: content and focus", () => {
  test("shows title, kind and details, with Allow once and Deny", async ({
    page,
  }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request());

    const shown = dialog(page);
    await expect(shown).toBeVisible();
    await expect(shown).toContainText("Run tests");
    await expect(shown).toContainText("Run a command");
    await expect(shown.locator("pre")).toHaveText("command: cargo test");
    await expect(allowButton(page)).toBeVisible();
    await expect(denyButton(page)).toBeVisible();
  });

  test("Deny has focus as soon as the dialog appears", async ({ page }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request());

    await expect(dialog(page)).toBeVisible();
    const focusedLabel = await page.evaluate(
      () => document.activeElement?.textContent?.trim() ?? null,
    );
    expect(focusedLabel).toBe("Deny");
    await expect(denyButton(page)).toBeFocused();
  });

  test("shows the unavailable warning and no details block when details are null", async ({
    page,
  }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ details: null }));

    const shown = dialog(page);
    await expect(shown).toBeVisible();
    await expect(shown.getByRole("note")).toContainText(
      "Operation details unavailable",
    );
    await expect(shown.locator("pre")).toHaveCount(0);
  });
});

test.describe("Permission prompt: keyboard", () => {
  test("Escape denies once and closes the dialog", async ({ page }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1" }));

    await page.keyboard.press("Escape");
    await expect(dialog(page)).toBeHidden();
    expect(await respondCalls(page)).toEqual([
      { command: RESPOND_COMMAND, args: { token: "t1", allow: false } },
    ]);
  });

  test("Enter denies once and closes the dialog", async ({ page }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1" }));

    await page.keyboard.press("Enter");
    await expect(dialog(page)).toBeHidden();
    expect(await respondCalls(page)).toEqual([
      { command: RESPOND_COMMAND, args: { token: "t1", allow: false } },
    ]);
  });

  test("Allow needs a click and sends allow:true", async ({ page }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1" }));

    await expect(allowButton(page)).toBeVisible();
    await allowButton(page).click();
    await expect(dialog(page)).toBeHidden();
    expect(await respondCalls(page)).toEqual([
      { command: RESPOND_COMMAND, args: { token: "t1", allow: true } },
    ]);
  });

  test("double click on Allow responds once", async ({ page }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1" }));

    await allowButton(page).dblclick();
    await expect(dialog(page)).toBeHidden();
    expect(await respondCalls(page)).toEqual([
      { command: RESPOND_COMMAND, args: { token: "t1", allow: true } },
    ]);
  });
});

test.describe("Permission prompt: lifecycle and safety", () => {
  test("acp-permission-closed removes the dialog without responding", async ({
    page,
  }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1" }));
    await expect(dialog(page)).toBeVisible();

    await emitMockEvent(page, CLOSED_EVENT, { token: "t1" });
    await expect(dialog(page)).toBeHidden();
    expect(await respondCalls(page)).toEqual([]);
  });

  test("a queued request is shown after the first is answered", async ({
    page,
  }) => {
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(page, request({ token: "t1", title: "First request" }));
    await emitMockEvent(
      page,
      REQUEST_EVENT,
      request({ token: "t2", title: "Second request", queued: 0 }),
    );

    await expect(dialog(page)).toContainText("First request");
    await page.keyboard.press("Escape");
    await expect(dialog(page)).toContainText("Second request");
    await expect(dialog(page)).not.toContainText("First request");
    expect(await respondCalls(page)).toEqual([
      { command: RESPOND_COMMAND, args: { token: "t1", allow: false } },
    ]);
  });

  test("title and details are shown as plain text, never as markup", async ({
    page,
  }) => {
    const hostileTitle = "<img src=x onerror=window.__pwned=1>";
    const hostileDetails = "<script>window.__pwned=1</script>";
    await installPermissionHarness(page);
    await page.goto("/");
    await showRequest(
      page,
      request({ title: hostileTitle, details: hostileDetails }),
    );

    const shown = dialog(page);
    await expect(shown).toBeVisible();
    await expect(shown).toContainText(hostileTitle);
    await expect(shown.locator("pre")).toHaveText(hostileDetails);
    await expect(shown.locator("img")).toHaveCount(0);
    await expect(shown.locator("script")).toHaveCount(0);
    const pwned = await page.evaluate(
      () => (window as unknown as { __pwned?: unknown }).__pwned,
    );
    expect(pwned).toBeUndefined();
  });
});

const SHOTS_DIR = process.env.SHOTS_DIR;

/** Screenshot of the dialog only. Written to SHOTS_DIR, not committed. */
async function shoot(
  page: Page,
  dir: string,
  payload: PermissionRequest,
  name: string,
): Promise<void> {
  await installPermissionHarness(page);
  await page.goto("/");
  await showRequest(page, payload);
  mkdirSync(dir, { recursive: true });
  await dialog(page).screenshot({ path: path.join(dir, name) });
}

test.describe("Permission prompt: screenshots", () => {
  test.skip(!SHOTS_DIR, "set SHOTS_DIR to write screenshots");

  test("with details", async ({ page }) => {
    await shoot(page, SHOTS_DIR ?? "", request(), "permission-details.png");
  });

  test("with unavailable details", async ({ page }) => {
    await shoot(
      page,
      SHOTS_DIR ?? "",
      request({ details: null }),
      "permission-unavailable.png",
    );
  });
});
