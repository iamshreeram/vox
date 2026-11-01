import { readFileSync } from "node:fs";
import { test, expect, type Locator, type Page } from "@playwright/test";
import { installTauriMock, setMockSetting } from "./fixtures/tauriMock";

/*
 * Settings cleanup, wave 1 UI (DESIGN.md Revision 3, UI1 = #169 #172 #184 #189).
 * Runs against the Vite dev server with the Tauri mock. Labels are read from the English
 * translation file so the spec follows the shipped copy. Every assertion here is written for
 * the target state, so these tests are expected to FAIL until the UI1 changes land.
 */

const NARROW = { width: 760, height: 600 };

function translation(locale: string): Record<string, unknown> {
  const url = new URL(
    `../src/i18n/locales/${locale}/translation.json`,
    import.meta.url,
  );
  return JSON.parse(readFileSync(url, "utf8")) as Record<string, unknown>;
}

function textAt(root: Record<string, unknown>, dottedKey: string): string {
  let node: unknown = root;
  for (const part of dottedKey.split(".")) {
    if (typeof node !== "object" || node === null) {
      throw new Error(`missing translation key ${dottedKey}`);
    }
    node = (node as Record<string, unknown>)[part];
  }
  if (typeof node !== "string") {
    throw new Error(`translation key ${dottedKey} is not a string`);
  }
  return node;
}

const EN = translation("en");
const label = (key: string): string => textAt(EN, key);

// Shipped English labels (the single source of truth for every locator below).
const AUTO_LABEL = label("settings.debug.autoUpdate.label");
const AUTO_CHECKS_OFF_REASON = label(
  "settings.debug.autoUpdate.checksOffDescription",
);
const CHECKS_LABEL = label("settings.debug.updateChecks.label");
const START_HIDDEN_LABEL = label("settings.advanced.startHidden.label");
const CHINESE_TITLE = label("settings.advanced.chineseScript.title");
const CHINESE_TRADITIONAL = label(
  "settings.advanced.chineseScript.options.traditional",
);
const CHINESE_SIMPLIFIED = label(
  "settings.advanced.chineseScript.options.simplified",
);
const VOLUME_TITLE = label("settings.sound.volume.title");
const RETENTION_TITLE = label("settings.debug.recordingRetention.title");
const RETENTION_NEVER = label("settings.debug.recordingRetention.never");
const RETENTION_PRESERVE_TEMPLATE = label(
  "settings.debug.recordingRetention.preserveLimit",
);
const TTS_LABEL = label("settings.advanced.tts.label");
// Literal: the Realtime duplicate's key was deleted in #172 (decision 2026-11-02).
const REALTIME_SPEAK_LABEL = "Speak replies aloud";

const AUTO_STATUS_FAILED_LINE =
  "Last automatic update did not finish. The newest version could not be verified on GitHub.";

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

const AUTO_STATUS_FAILED = {
  ...AUTO_STATUS_IDLE,
  enabled: true,
  effective: true,
  last_result: "failed",
  reason_code: "verify_failed",
  target_short: "abc12345",
  ts: 1700000000,
};

interface OpenOptions {
  settings?: Record<string, unknown>;
  commands?: Record<string, unknown>;
}

/** Opens the app at the narrow size. Mock settings are the defaults plus `settings`. */
async function openApp(page: Page, options: OpenOptions = {}): Promise<void> {
  await page.setViewportSize(NARROW);
  await installTauriMock(page, {
    settings: {
      brain_provider_id: "",
      brain_model: "",
      update_checks_enabled: true,
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

const navItem = (page: Page, text: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${text}$`) });

/** Opens a sidebar page and waits for its title (the only h1) to render. */
async function openPage(page: Page, sidebarText: string): Promise<void> {
  await navItem(page, sidebarText).click();
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
}

const searchBox = (page: Page): Locator =>
  page.getByRole("combobox", { name: /search settings/i });
const options = (page: Page): Locator => page.getByRole("option");
const checkbox = (page: Page, name: string): Locator =>
  page.getByRole("checkbox", { name, exact: true });
/*
 * Every ToggleSwitch input carries its label as aria-label. Counting the DOM (not the
 * accessibility tree) also sees controls inside collapsed groups, which getByRole skips.
 */
const toggleInput = (page: Page, name: string): Locator =>
  page.locator(`input[type="checkbox"][aria-label="${name}"]`);
/** A settings row, located by the title it renders (SettingContainer data-setting-title). */
const row = (page: Page, title: string): Locator =>
  page.locator(`[data-setting-title="${title}"]`);

test.describe("UI1 (#169): updates controls live on About, not Debug", () => {
  test("(a) About shows the update checks toggle, the automatic toggle and the status line without debug mode", async ({
    page,
  }) => {
    await openApp(page, {
      settings: { auto_update_enabled: true, debug_mode: false },
      commands: { get_auto_update_status: AUTO_STATUS_FAILED },
    });
    await openPage(page, label("sidebar.about"));

    await expect(checkbox(page, CHECKS_LABEL)).toBeVisible();
    await expect(checkbox(page, AUTO_LABEL)).toBeVisible();
    await expect(
      page.getByRole("status").filter({ hasText: AUTO_STATUS_FAILED_LINE }),
    ).toBeVisible();
  });

  test("(a2) with debug mode on, the Debug page contains none of the three update controls", async ({
    page,
  }) => {
    await openApp(page, {
      settings: { auto_update_enabled: true, debug_mode: true },
      commands: { get_auto_update_status: AUTO_STATUS_FAILED },
    });
    await openPage(page, label("sidebar.debug"));

    await expect(toggleInput(page, CHECKS_LABEL)).toHaveCount(0);
    await expect(toggleInput(page, AUTO_LABEL)).toHaveCount(0);
    await expect(
      page.getByRole("status").filter({ hasText: AUTO_STATUS_FAILED_LINE }),
    ).toHaveCount(0);
  });

  test("(a3) with update checks off, the automatic toggle is disabled and says to turn on Check for Updates first", async ({
    page,
  }) => {
    await openApp(page, {
      settings: { update_checks_enabled: false, debug_mode: false },
    });
    await openPage(page, label("sidebar.about"));

    const autoToggle = checkbox(page, AUTO_LABEL);
    await expect(autoToggle).toBeDisabled();
    await expect(autoToggle).not.toBeChecked();

    // The description sits behind the row's "More information" tooltip (hover opens it).
    const info = row(page, AUTO_LABEL).getByRole("button", {
      name: "More information",
    });
    if ((await info.count()) > 0) await info.hover();
    await expect(
      page.getByText(AUTO_CHECKS_OFF_REASON, { exact: true }),
    ).toBeVisible();
  });
});

test.describe("UI2 (#172): one Speak replies control", () => {
  test("(b) the speak-replies control exists exactly once across every sidebar page", async ({
    page,
  }) => {
    await openApp(page, {
      settings: {
        experimental_enabled: true,
        debug_mode: true,
        post_process_enabled: true,
      },
    });

    const pages = [
      "sidebar.general",
      "sidebar.appearance",
      "sidebar.history",
      "sidebar.models",
      "sidebar.brain",
      "sidebar.advanced",
      "sidebar.postProcessing",
      "sidebar.agents",
      "sidebar.debug",
      "sidebar.about",
    ];
    const speakLabels = [TTS_LABEL, REALTIME_SPEAK_LABEL];
    const found: string[] = [];
    for (const key of pages) {
      const pageName = label(key);
      await openPage(page, pageName);
      for (const speakLabel of speakLabels) {
        const count = await toggleInput(page, speakLabel).count();
        for (let i = 0; i < count; i += 1) {
          found.push(`${pageName}: ${speakLabel}`);
        }
      }
    }

    expect(found, "speak-replies controls across all pages").toHaveLength(1);
    expect(found[0]).toBe(`${label("sidebar.models")}: ${TTS_LABEL}`);
  });

  for (const experimental of [false, true]) {
    test(`(b2) searching "speak replies" returns only the Models text-to-speech toggle (experimental ${experimental ? "on" : "off"})`, async ({
      page,
    }) => {
      await openApp(page, { settings: { experimental_enabled: experimental } });

      await searchBox(page).fill("speak replies");
      const topResult = options(page).first();
      await expect(topResult).toContainText(TTS_LABEL);
      // The removed Realtime duplicate must not be listed at all (R3-6).
      await expect(
        options(page).filter({ hasText: REALTIME_SPEAK_LABEL }),
      ).toHaveCount(0);

      await topResult.click();
      await expect(
        page.getByRole("heading", { level: 1, name: label("sidebar.models") }),
      ).toBeVisible();
      await expect(checkbox(page, TTS_LABEL)).toBeVisible();
    });
  }
});

test.describe("UI3 (#184): controls show the Rust defaults and keep stored values", () => {
  test("(c1) with the volume and retention keys omitted, volume shows 100% and retention shows preserve limit", async ({
    page,
  }) => {
    await openApp(page);

    await openPage(page, label("sidebar.general"));
    await expect(
      row(page, VOLUME_TITLE).getByText("100%", { exact: true }),
    ).toBeVisible();

    await openPage(page, label("sidebar.advanced"));
    const preserveText = RETENTION_PRESERVE_TEMPLATE.replace("{{count}}", "5");
    await expect(
      row(page, RETENTION_TITLE).getByRole("button", {
        name: preserveText,
        exact: true,
      }),
    ).toBeVisible();
  });

  test("(c2) with chinese_script undefined, the Chinese script control is absent; it appears with the backend value", async ({
    page,
  }) => {
    await openApp(page);
    await openPage(page, label("sidebar.advanced"));
    await expect(row(page, CHINESE_TITLE)).toHaveCount(0);

    await setMockSetting(page, "chinese_script", "traditional");
    await expect(row(page, CHINESE_TITLE)).toBeVisible();
    await expect(
      row(page, CHINESE_TITLE).getByRole("button", {
        name: CHINESE_TRADITIONAL,
        exact: true,
      }),
    ).toBeVisible();
  });

  test("(c3) explicit stored values are preserved: volume 0.3, retention never, Chinese script simplified", async ({
    page,
  }) => {
    await openApp(page, {
      settings: {
        audio_feedback_volume: 0.3,
        recording_retention_period: "never",
        chinese_script: "simplified",
      },
    });

    await openPage(page, label("sidebar.general"));
    await expect(
      row(page, VOLUME_TITLE).getByText("30%", { exact: true }),
    ).toBeVisible();

    await openPage(page, label("sidebar.advanced"));
    await expect(
      row(page, RETENTION_TITLE).getByRole("button", {
        name: RETENTION_NEVER,
        exact: true,
      }),
    ).toBeVisible();
    await expect(
      row(page, CHINESE_TITLE).getByRole("button", {
        name: CHINESE_SIMPLIFIED,
        exact: true,
      }),
    ).toBeVisible();
  });
});

test.describe("UI4 (#189): Start Hidden follows Show Tray Icon", () => {
  test("(d) Start Hidden is absent when show_tray_icon is false", async ({
    page,
  }) => {
    await openApp(page, { settings: { show_tray_icon: false } });
    await openPage(page, label("sidebar.advanced"));
    await expect(row(page, START_HIDDEN_LABEL)).toHaveCount(0);
  });

  test("(d2) Start Hidden is present when show_tray_icon is true", async ({
    page,
  }) => {
    await openApp(page, { settings: { show_tray_icon: true } });
    await openPage(page, label("sidebar.advanced"));
    await expect(row(page, START_HIDDEN_LABEL)).toBeVisible();
  });
});

test.describe("settings search finds the About entries", () => {
  test("(e1) 'automatic updates' yields the automatic update toggle on About", async ({
    page,
  }) => {
    await openApp(page);

    await searchBox(page).fill("automatic updates");
    await options(page).filter({ hasText: AUTO_LABEL }).first().click();

    await expect(
      page.getByRole("heading", { level: 1, name: label("sidebar.about") }),
    ).toBeVisible();
    await expect(checkbox(page, AUTO_LABEL)).toBeVisible();
  });

  test("(e2) 'check for updates' yields the Check for Updates toggle on About", async ({
    page,
  }) => {
    await openApp(page);

    await searchBox(page).fill("check for updates");
    await options(page).filter({ hasText: CHECKS_LABEL }).first().click();

    await expect(
      page.getByRole("heading", { level: 1, name: label("sidebar.about") }),
    ).toBeVisible();
    await expect(checkbox(page, CHECKS_LABEL)).toBeVisible();
  });
});
