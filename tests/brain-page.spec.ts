import { test, expect, type Locator, type Page } from "@playwright/test";
import { getMockCalls, installTauriMock } from "./fixtures/tauriMock";

/*
 * Brain page (DESIGN.md Rev 6, P3b). Mounted against the Vite dev server with the Tauri mock.
 * The mock records every invoke; `getMockCalls(page, command)` (added to tauriMock.ts) returns the
 * argument objects of that command in call order.
 *
 * Contract: sidebar item "Brain" (always visible, also with Experimental off) opens
 * [data-settings-section="brain"] with: Endpoint (provider select, base URL, quick fills for
 * Ollama and LM Studio, token, model + Fetch models), a Test panel behind an explicit
 * acknowledgement, and one card per feature (Reply style, Real-time conversation,
 * Post-processing) showing whether it uses the Brain or its own setup.
 * data-testid contract: brain-page, brain-provider, brain-base-url, brain-token, brain-model,
 * brain-fetch-models, brain-quickfill-ollama, brain-quickfill-lmstudio, brain-ack, brain-test,
 * brain-test-result, brain-url-error, brain-token-status, brain-feature-<composer|realtime_stream|post_process>
 * (with -state text, -use-brain, -use-own), brain-consent-dialog (with -confirm, -cancel, -destination).
 */

const section = (page: Page, id: string): Locator =>
  page.locator(`[data-settings-section="${id}"]`);
const navItem = (page: Page, text: string): Locator =>
  page.locator("p", { hasText: new RegExp(`^${text}$`) });
const tid = (page: Page, id: string): Locator => page.getByTestId(id);

const FEATURE_LABEL = {
  composer: "Reply style",
  realtime_stream: "Real-time conversation",
  post_process: "Post Process",
};

const FRESH = {
  brain_schema_v1: true,
  brain_provider_id: "",
  brain_base_url: "",
  brain_model: "",
  brain_token_set: false,
  brain_source_composer: "inherit",
  brain_source_realtime: "inherit",
  brain_source_postprocess: "inherit",
  brain_test_ack_host: "",
  brain_transcript_consent_dest: "",
};
const LOCAL = {
  ...FRESH,
  brain_provider_id: "custom",
  brain_base_url: "http://127.0.0.1:11434/v1",
  brain_model: "llama3",
};
const LOCAL_ACKED = {
  ...LOCAL,
  brain_test_ack_host: "custom|http://127.0.0.1:11434/v1",
};
const LOCAL_CONSENTED = {
  ...LOCAL_ACKED,
  brain_transcript_consent_dest: "custom|http://127.0.0.1:11434/v1",
};

async function open(
  page: Page,
  settings: Record<string, unknown>,
  commands = {},
) {
  await installTauriMock(page, { settings, commands });
  await page.goto("/");
  await navItem(page, "Brain").click();
  await expect(section(page, "brain")).toBeVisible();
}

test.describe("Brain page", () => {
  for (const experimental of [false, true]) {
    test(`1. is a sidebar page without Experimental (experimental ${experimental})`, async ({
      page,
    }) => {
      await open(page, { ...FRESH, experimental_enabled: experimental });
      await expect(tid(page, "brain-page")).toBeVisible();
      await expect(tid(page, "brain-provider")).toBeVisible();
      await expect(tid(page, "brain-feature-composer")).toBeVisible();
      await expect(tid(page, "brain-feature-realtime_stream")).toBeVisible();
      await expect(tid(page, "brain-feature-post_process")).toBeVisible();
    });
  }

  test("2. fresh install: nothing is configured, Test and Fetch models are disabled, features need setup", async ({
    page,
  }) => {
    await open(page, FRESH);
    await expect(tid(page, "brain-test")).toBeDisabled();
    await expect(tid(page, "brain-fetch-models")).toBeDisabled();
    for (const f of ["composer", "realtime_stream", "post_process"]) {
      await expect(tid(page, `brain-feature-${f}-state`)).toContainText(
        /set up the brain/i,
      );
      await expect(tid(page, `brain-feature-${f}-use-brain`)).toBeDisabled();
    }
  });

  test("3. quick fill writes the endpoint through the validated command", async ({
    page,
  }) => {
    await open(page, FRESH);
    await tid(page, "brain-quickfill-ollama").click();
    const calls = await getMockCalls(page, "change_brain_connection_setting");
    expect(calls.at(-1)).toMatchObject({
      providerId: "custom",
      baseUrl: "http://127.0.0.1:11434/v1",
    });
  });

  test("4. an invalid or unsafe URL is rejected inline and never sent", async ({
    page,
  }) => {
    await open(page, LOCAL);
    const url = tid(page, "brain-base-url");
    for (const bad of [
      "http://example.com/v1",
      "https://user:pw@example.com/v1",
      "ftp://x",
    ]) {
      await url.fill(bad);
      await url.blur();
      await expect(tid(page, "brain-url-error")).toBeVisible();
    }
    const calls = await getMockCalls(page, "change_brain_connection_setting");
    expect(
      calls.some(
        (c) =>
          String(c.baseUrl).includes("example.com") ||
          String(c.baseUrl).startsWith("ftp"),
      ),
    ).toBe(false);
  });

  test("5. the token is write-only: saved through its own command, never displayed", async ({
    page,
  }) => {
    await open(page, { ...LOCAL, brain_token_set: true });
    await expect(tid(page, "brain-token")).toHaveValue("");
    await expect(tid(page, "brain-token-status")).toContainText(/saved/i);
    await tid(page, "brain-token").fill("sk-SENTINEL-123");
    await tid(page, "brain-token").blur();
    const calls = await getMockCalls(page, "set_brain_token");
    expect(calls.at(-1)).toMatchObject({ token: "sk-SENTINEL-123" });
    expect(await page.locator("body").innerText()).not.toContain(
      "sk-SENTINEL-123",
    );
    expect(await page.content()).not.toContain("sk-SENTINEL-123");
  });

  test("6. Test requires the acknowledgement; it names the destination and the fixed prompt", async ({
    page,
  }) => {
    await open(page, LOCAL, {
      brain_test_connection: {
        ok: true,
        kind: "Ok",
        latency_ms: 42,
        model_count: null,
      },
    });
    await expect(tid(page, "brain-test")).toBeDisabled();
    await expect(tid(page, "brain-ack")).toContainText("127.0.0.1:11434");
    await expect(tid(page, "brain-ack")).toContainText(/Reply with OK/);
    await tid(page, "brain-ack").getByRole("checkbox").check();
    expect(await getMockCalls(page, "brain_acknowledge_test")).toHaveLength(1);
    expect(await getMockCalls(page, "brain_test_connection")).toHaveLength(0);
  });

  test("7. a successful test shows the latency; each failure kind shows its own message and no raw text", async ({
    page,
  }) => {
    await open(page, LOCAL_ACKED, {
      brain_test_connection: {
        ok: true,
        kind: "Ok",
        latency_ms: 42,
        model_count: null,
      },
    });
    await tid(page, "brain-test").click();
    await expect(tid(page, "brain-test-result")).toContainText(/42/);
    const kinds: Array<[string, RegExp]> = [
      ["Unauthorized", /token/i],
      ["NotFound", /address|endpoint|path/i],
      ["Unreachable", /reach/i],
      ["Redirect", /redirect/i],
      ["BadResponse", /unexpected|valid/i],
      ["Timeout", /time/i],
    ];
    for (const [kind, re] of kinds) {
      await page.evaluate(
        (k) =>
          (
            window as unknown as {
              __voxMock: { setCommand: (c: string, v: unknown) => void };
            }
          ).__voxMock.setCommand("brain_test_connection", {
            ok: false,
            kind: k,
            latency_ms: 5,
            model_count: null,
          }),
        kind,
      );
      await tid(page, "brain-test").click();
      await expect(tid(page, "brain-test-result")).toContainText(re);
    }
  });

  test("8. Fetch models fills a creatable list and keeps typing possible", async ({
    page,
  }) => {
    await open(page, LOCAL_ACKED, {
      brain_fetch_models: ["llama3", "mistral"],
    });
    await tid(page, "brain-fetch-models").click();
    await tid(page, "brain-model").click();
    await expect(page.getByRole("option", { name: "mistral" })).toBeVisible();
    await page.getByRole("option", { name: "mistral" }).click();
    const calls = await getMockCalls(page, "change_brain_connection_setting");
    expect(calls.at(-1)).toMatchObject({ model: "mistral" });
  });

  test("9. feature cards reflect their real state", async ({ page }) => {
    await open(page, {
      ...LOCAL_CONSENTED,
      brain_source_realtime: "own",
      brain_transcript_consent_dest: "custom|http://127.0.0.1:11434/v1",
    });
    await expect(tid(page, "brain-feature-composer-state")).toContainText(
      /using the brain/i,
    );
    await expect(
      tid(page, "brain-feature-realtime_stream-state"),
    ).toContainText(/own setup/i);
    await expect(tid(page, "brain-feature-post_process-state")).toContainText(
      /using the brain/i,
    );
  });

  test("10. without consent a feature says it needs permission and does not claim to use the Brain", async ({
    page,
  }) => {
    await open(page, LOCAL_ACKED);
    await expect(tid(page, "brain-feature-composer-state")).toContainText(
      /permission|allow/i,
    );
    await expect(tid(page, "brain-feature-composer-state")).not.toContainText(
      /using the brain/i,
    );
  });

  test("11. Use Brain asks for explicit consent naming the destination; cancel changes nothing", async ({
    page,
  }) => {
    await open(page, { ...LOCAL_ACKED, brain_source_composer: "own" });
    await tid(page, "brain-feature-composer-use-brain").click();
    const dialog = tid(page, "brain-consent-dialog");
    await expect(dialog).toBeVisible();
    await expect(tid(page, "brain-consent-destination")).toContainText(
      "127.0.0.1:11434",
    );
    await tid(page, "brain-consent-cancel").click();
    expect(await getMockCalls(page, "brain_use_for_feature")).toHaveLength(0);
    await tid(page, "brain-feature-composer-use-brain").click();
    await tid(page, "brain-consent-confirm").click();
    const calls = await getMockCalls(page, "brain_use_for_feature");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({ feature: "composer", useBrain: true });
  });

  test("12. Use own setup needs no consent and calls the command once", async ({
    page,
  }) => {
    await open(page, LOCAL_CONSENTED);
    await tid(page, "brain-feature-post_process-use-own").click();
    const calls = await getMockCalls(page, "brain_use_for_feature");
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatchObject({
      feature: "post_process",
      useBrain: false,
    });
  });

  test("13. an existing user (no migration marker) sees every feature on its own setup", async ({
    page,
  }) => {
    await open(page, {
      ...LOCAL_CONSENTED,
      brain_schema_v1: false,
      brain_source_composer: "own",
      brain_source_realtime: "own",
      brain_source_postprocess: "own",
    });
    for (const f of ["composer", "realtime_stream", "post_process"]) {
      await expect(tid(page, `brain-feature-${f}-state`)).toContainText(
        /own setup/i,
      );
    }
  });

  test("14. the page is reachable from settings search", async ({ page }) => {
    await installTauriMock(page, { settings: FRESH });
    await page.goto("/");
    const search = page.getByRole("combobox", { name: /search settings/i });
    await search.fill("brain");
    await expect(page.getByRole("option").first()).toBeVisible();
    await expect(
      page.getByRole("option", { name: /brain/i }).first(),
    ).toBeVisible();
  });

  test("15. no hard-coded English leaks into other locales' page (keys exist)", async ({
    page,
  }) => {
    await open(page, { ...FRESH, app_language: "de" });
    await expect(tid(page, "brain-page")).not.toContainText("settings.brain.");
  });

  // The existing per-feature pages must not silently show fields that no longer apply.
  test("16. Reply style on General says it uses the Brain (and links to it); hidden when it uses its own setup", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: LOCAL_CONSENTED });
    await page.goto("/");
    await expect(navItem(page, "General")).toBeVisible();
    // S4a: the one setup-status banner replaces the per-feature notice on General.
    const notice = tid(page, "setup-status-banner");
    await expect(notice).toBeVisible();
    await expect(notice).toContainText(/brain/i);
    await expect(notice).toContainText(FEATURE_LABEL.composer);
    await notice.getByRole("button", { name: /open brain/i }).click();
    await expect(section(page, "brain")).toBeVisible();
  });

  test("17. Reply style shows no Brain notice when the feature uses its own setup", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: { ...LOCAL_CONSENTED, brain_source_composer: "own" },
    });
    await page.goto("/");
    await expect(navItem(page, "General")).toBeVisible();
    await expect(tid(page, "brain-notice-composer")).toHaveCount(0);
  });

  test("18. Post Process page and the real-time controls show their Brain notice", async ({
    page,
  }) => {
    await installTauriMock(page, {
      settings: {
        ...LOCAL_CONSENTED,
        post_process_enabled: true,
        experimental_enabled: true,
      },
    });
    await page.goto("/");
    await expect(tid(page, "brain-notice-realtime_stream")).toBeVisible();
    await expect(tid(page, "brain-notice-realtime_stream")).toContainText(
      FEATURE_LABEL.realtime_stream,
    );
    await navItem(page, "Post Process").click();
    await expect(tid(page, "brain-notice-post_process")).toBeVisible();
    await expect(tid(page, "brain-notice-post_process")).toContainText(
      FEATURE_LABEL.post_process,
    );
  });

  test("19. a notice offers 'Use own setup' which calls the command", async ({
    page,
  }) => {
    await installTauriMock(page, { settings: LOCAL_CONSENTED });
    await page.goto("/");
    await tid(page, "setup-status-banner")
      .getByRole("button", { name: /own setup/i })
      .click();
    const calls = await getMockCalls(page, "brain_use_for_feature");
    expect(calls.at(-1)).toMatchObject({
      feature: "composer",
      useBrain: false,
    });
  });
});
