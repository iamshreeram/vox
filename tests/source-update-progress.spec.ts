import { test, expect, type Page } from "@playwright/test";
import {
  emitMockEvent,
  getMockCalls,
  installTauriMock,
} from "./fixtures/tauriMock";

/*
 * Background source update: footer progress, done and failure (UI, STEP A3).
 * Runs against the Vite dev server with the Tauri mock. Progress arrives as
 * `source-update-progress` events from the mock event bus.
 */

const NARROW = { width: 760, height: 600 };

const UPDATE_INFO = {
  state: "available",
  current_commit: "1111111",
  latest_commit: "2222222",
  latest_full: "2".repeat(40),
  behind_by: 3,
  changes: ["Fix the thing", "Add the other thing"],
  compare_url: "https://github.com/iamshreeram/vox/compare/1111111...2222222",
  note: null,
  can_install: true,
  install_blocker: null,
};

const RUN = "r".repeat(16);
const OTHER_RUN = "s".repeat(16);

function progress(
  runId: string,
  phase: string,
  percent: number,
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    run_id: runId,
    phase,
    percent,
    target: "abc1234",
    message: "",
    estimated: false,
    ...extra,
  };
}

async function open(
  page: Page,
  size = NARROW,
  extraCommands: Record<string, unknown> = {},
): Promise<void> {
  await page.setViewportSize(size);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      update_checks_enabled: true,
    },
    commands: {
      get_update_mode: "source",
      is_update_checks_locked: false,
      check_source_update: UPDATE_INFO,
      get_cached_source_update: UPDATE_INFO,
      get_source_update_install_blocker: null,
      "plugin:app|version": "0.9.9",
      ...extraCommands,
    },
  });
  await page.goto("/");
  await expect(page.locator("p", { hasText: /^General$/ })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Update available" }),
  ).toBeVisible();
}

async function startUpdate(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Update available" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Update now" }).click();
  await expect(dialog).toBeHidden();
}

function bar(page: Page) {
  return page.getByRole("progressbar", { name: "Vox update progress" });
}

async function barValue(page: Page): Promise<number> {
  const value = await bar(page).getAttribute("aria-valuenow");
  return Number(value);
}

/** Records every distinct text the footer's live region shows, in order. */
async function watchAnnouncements(page: Page): Promise<void> {
  await page.evaluate(() => {
    const live = document.querySelector<HTMLElement>('[role="status"].sr-only');
    if (!live) throw new Error("footer live region not found");
    const seen: string[] = [];
    (window as unknown as { __announcements: string[] }).__announcements = seen;
    const observer = new MutationObserver(() => {
      const text = live.textContent ?? "";
      if (text !== "" && text !== seen[seen.length - 1]) seen.push(text);
    });
    observer.observe(live, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  });
}

async function announcements(page: Page): Promise<string[]> {
  return page.evaluate(
    () => (window as unknown as { __announcements: string[] }).__announcements,
  );
}

test.describe("source update progress footer", () => {
  test("progress increases monotonically, announces only at buckets, then shows done", async ({
    page,
  }) => {
    await open(page);
    await startUpdate(page);
    await expect(bar(page)).toBeVisible();
    await watchAnnouncements(page);

    const values: number[] = [];
    const steps: Array<[string, number]> = [
      ["preparing", 0],
      ["dependencies", 10],
      ["compiling", 25],
      ["compiling", 40],
      ["compiling", 30], // stale: must not move the bar backwards
      ["bundling", 50],
      ["bundling", 60],
      ["bundling", 75],
      ["installing", 90],
      ["installing", 99],
    ];
    for (const [phase, percent] of steps) {
      await emitMockEvent(
        page,
        "source-update-progress",
        progress(RUN, phase, percent),
      );
      await expect(bar(page)).toHaveAttribute(
        "aria-valuenow",
        String(Math.max(percent, values[values.length - 1] ?? 0)),
      );
      values.push(await barValue(page));
    }
    for (let i = 1; i < values.length; i += 1) {
      expect(values[i]).toBeGreaterThanOrEqual(values[i - 1]);
    }
    expect(values[values.length - 1]).toBe(99);

    await expect(page.getByText("Updating… 99%")).toBeVisible();
    await expect(bar(page)).toHaveAttribute(
      "aria-valuetext",
      "99%, Installing",
    );

    expect(await announcements(page)).toEqual([
      "Update 0%, Preparing",
      "Update 25%, Compiling",
      "Update 50%, Bundling",
      "Update 75%, Bundling",
    ]);

    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "done", 100),
    );
    await expect(page.getByText("Up to date · v0.9.9 (abc1234)")).toBeVisible();
    expect(await announcements(page)).toEqual([
      "Update 0%, Preparing",
      "Update 25%, Compiling",
      "Update 50%, Bundling",
      "Update 75%, Bundling",
      "Update complete: abc1234",
    ]);
  });

  test("the updated label reverts to up to date after 30 seconds", async ({
    page,
  }) => {
    await page.clock.install();
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "done", 100),
    );
    await expect(page.getByText("Up to date · v0.9.9 (abc1234)")).toBeVisible();

    await page.clock.fastForward(31_000);
    await expect(page.getByText("Up to date", { exact: true })).toBeVisible();
    await expect(page.getByText(/\(abc1234\)/)).toHaveCount(0);
  });

  test("a failed run shows the message in an alert and Retry re-checks", async ({
    page,
  }) => {
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 30),
    );
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "failed", 30, {
        message: "Build step exited with code 101",
      }),
    );

    await expect(
      page.locator("span.text-danger", { hasText: "Update failed" }),
    ).toBeVisible();
    const alert = page.getByRole("alert").filter({
      hasText: "Build step exited with code 101",
    });
    await expect(alert).toBeVisible();
    await expect(alert).toHaveAttribute(
      "title",
      "Build step exited with code 101",
    );

    const before = (await getMockCalls(page, "check_source_update")).length;
    const retry = page.getByRole("button", { name: "Retry" });
    await retry.focus();
    await page.keyboard.press("Enter");
    await expect
      .poll(
        async () => (await getMockCalls(page, "check_source_update")).length,
      )
      .toBe(before + 1);
    // A manual check that finds an update opens the confirmation again.
    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(
      page.getByRole("button", { name: "Update available" }),
    ).toBeVisible();
  });

  test("Retry also works with Space", async ({ page }) => {
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 10),
    );
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "failed", 10, { message: "Second failure" }),
    );
    const before = (await getMockCalls(page, "check_source_update")).length;
    const retry = page.getByRole("button", { name: "Retry" });
    await retry.focus();
    await page.keyboard.press(" ");
    await expect
      .poll(
        async () => (await getMockCalls(page, "check_source_update")).length,
      )
      .toBe(before + 1);
  });

  test("a new run_id resets the bar and older run events are ignored", async ({
    page,
  }) => {
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 80),
    );
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "80");
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(OTHER_RUN, "preparing", 5),
    );
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "5");
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 95),
    );
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "5");
  });

  test("the footer item is not focusable while the update runs", async ({
    page,
  }) => {
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 40),
    );
    await expect(bar(page)).toBeVisible();
    await expect(page.getByRole("button", { name: /Updating/ })).toHaveCount(0);
    await expect(bar(page)).not.toHaveAttribute("tabindex", /.*/);
  });

  test("the footer stays on one line with no overflow at the narrowest settings width", async ({
    page,
  }) => {
    await open(page, NARROW);
    await startUpdate(page);
    const row = bar(page).locator("xpath=..");
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "preparing", 0),
    );
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "0");
    const atStart = await row.boundingBox();
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "installing", 99),
    );
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "99");
    const atEnd = await row.boundingBox();
    expect(atStart).not.toBeNull();
    expect(atEnd).not.toBeNull();
    expect(atEnd?.height).toBeCloseTo(atStart?.height ?? -1, 1);
    expect(atEnd?.height ?? Infinity).toBeLessThan(24);
    const overflow = await page.evaluate(
      () =>
        document.documentElement.scrollWidth -
        document.documentElement.clientWidth,
    );
    expect(overflow).toBeLessThanOrEqual(0);
  });

  test("the bar stops transitioning under reduced motion", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 50),
    );
    const fill = bar(page).locator("span").first();
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "50");
    // The bar has no width transition; the app's global reduced-motion rule
    // (src/styles/theme.css) also pins any remaining duration to 0.01 ms.
    await expect(fill).toHaveCSS("transition-property", "none");
    const seconds = await fill.evaluate((element) =>
      Number.parseFloat(getComputedStyle(element).transitionDuration),
    );
    expect(seconds).toBeLessThanOrEqual(0.00001);
  });

  test("the bar keeps its 300 ms width transition without reduced motion", async ({
    page,
  }) => {
    await page.emulateMedia({ reducedMotion: "no-preference" });
    await open(page);
    await startUpdate(page);
    await emitMockEvent(
      page,
      "source-update-progress",
      progress(RUN, "compiling", 50),
    );
    const fill = bar(page).locator("span").first();
    await expect(bar(page)).toHaveAttribute("aria-valuenow", "50");
    await expect(fill).toHaveCSS("transition-duration", "0.3s");
  });
});
