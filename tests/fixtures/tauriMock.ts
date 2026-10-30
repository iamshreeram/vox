import type { Page } from "@playwright/test";

/**
 * Minimal Tauri runtime mock for running the Vite-served frontend in a plain
 * browser (no native backend). It is injected with `page.addInitScript`, so it
 * is in place before the app's first module runs.
 *
 * Contract with the frontend:
 * - `@tauri-apps/api/core` `invoke` -> `window.__TAURI_INTERNALS__.invoke`.
 *   tauri-specta bindings wrap the RAW return value as `{status:"ok", data}`,
 *   so this mock returns raw command payloads (e.g. the AppSettings object),
 *   not Result-shaped objects.
 * - `@tauri-apps/plugin-os` `platform()` -> `window.__TAURI_OS_PLUGIN_INTERNALS__`.
 * - `@tauri-apps/api/event` `listen` -> `plugin:event|listen` + transformCallback.
 * - Unknown commands resolve to `null` (a harmless ok value).
 *
 * Test-side controls are exposed on `window.__voxMock`:
 * - `setSetting(key, value)` updates the in-memory settings and emits
 *   `settings-changed` so the settings store refreshes.
 * - `emit(event, payload)` delivers an event to `listen` subscribers.
 */

export type MockPlatform = "macos" | "windows" | "linux";
export type MockSettings = Record<string, unknown>;

export interface TauriMockOptions {
  /** OS reported by plugin-os. Defaults to "macos" (permission checks are mocked to true). */
  platform?: MockPlatform;
  /** Overrides merged on top of DEFAULT_MOCK_SETTINGS. */
  settings?: MockSettings;
  /** When true, localStorage getItem/setItem throw (simulates a blocked storage). */
  storageThrows?: boolean;
  /**
   * Fixed raw results per command name, overriding the defaults (JSON-serializable).
   * Use `setMockCommand` to change one at runtime.
   */
  commands?: Record<string, unknown>;
  /**
   * Window label reported by the runtime (`getCurrentWindow().label`). Defaults to "main".
   * Use "chat", "orb" or "orb-bubbles" for the listen-only consumer pages (UI quality gate).
   */
  label?: string;
}

interface MockInitArg {
  platform: MockPlatform;
  defaults: MockSettings;
  settings: MockSettings;
  storageThrows: boolean;
  commands: Record<string, unknown>;
  label: string;
}

/** One `appearance` delivery the mock recorded (the window label it was addressed to). */
export interface MockEmit {
  label: string;
  event: string;
  payload: unknown;
}

/** The committed appearance snapshot (`AppearanceSnapshot` wire shape). */
export interface MockAppearance {
  rev: number;
  theme: unknown;
  custom_colors: unknown;
  accent: unknown;
  chat_theme: unknown;
  visual_style: unknown;
}

/** Consumer window labels the Rust side addresses with `emit_to` (appearance.rs CONSUMER_LABELS). */
export const APPEARANCE_CONSUMER_LABELS = [
  "chat",
  "orb",
  "orb-bubbles",
] as const;

export const DEFAULT_MOCK_SETTINGS: Readonly<MockSettings> = {
  settings_schema_version: 1,
  onboarding_completed: true,
  app_language: "en",
  post_process_enabled: false,
  debug_mode: false,
  experimental_enabled: false,
  update_checks_enabled: false,
  show_whats_new_on_update: false,
  whats_new_last_seen_version: null,
  // Fresh install: the Brain is inherited by every feature, and nothing is configured yet.
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

/**
 * Runs inside the page before any app code. Must be self-contained: it cannot
 * reference anything from this module except its argument.
 */
function installTauriMockInPage(arg: MockInitArg): void {
  interface Handlers {
    [command: string]: (args: Record<string, unknown>) => unknown;
  }
  interface Listener {
    eventName: string;
    handlerId: number;
  }
  interface RecordedCall {
    command: string;
    args: Record<string, unknown>;
  }
  interface VoxMockWindow {
    __TAURI_INTERNALS__: unknown;
    __TAURI_EVENT_PLUGIN_INTERNALS__: unknown;
    __TAURI_OS_PLUGIN_INTERNALS__: unknown;
    __voxMock: {
      setSetting: (key: string, value: unknown) => void;
      setCommand: (command: string, value: unknown) => void;
      emit: (eventName: string, payload: unknown) => void;
      calls: RecordedCall[];
      label: string;
      appearance: () => unknown;
      emitted: Array<{ label: string; event: string; payload: unknown }>;
    };
  }

  const win = window as unknown as VoxMockWindow;
  const settings: MockSettings = { ...arg.settings };
  const defaults: MockSettings = { ...arg.defaults };
  const overrides: Record<string, unknown> = { ...arg.commands };
  const callbacks = new Map<number, (message: unknown) => void>();
  const listeners: Listener[] = [];
  // Every invoke, in order. Read by getMockCalls.
  const calls: RecordedCall[] = [];
  let nextId = 1;

  // Committed appearance (additive): the five appearance writers persist into `settings`, bump a
  // monotonic rev and publish the full snapshot to every consumer label through `emitted`. The
  // delivery to THIS page happens locally when its own label is a consumer label; other labels
  // are delivered by the test harness through emitMockEvent (one page per window).
  const APPEARANCE_ARG: Record<string, [keyof MockSettings, string]> = {
    change_theme_setting: ["theme", "theme"],
    update_custom_theme_colors: ["custom_theme_colors", "colors"],
    change_accent_setting: ["accent", "accent"],
    change_chat_theme_setting: ["chat_theme", "chatTheme"],
    change_visual_style_setting: ["visual_style", "visualStyle"],
  };
  const CONSUMER_LABELS = ["chat", "orb", "orb-bubbles"];
  const emitted: Array<{ label: string; event: string; payload: unknown }> = [];
  let appearanceRev = 1;
  const committedAppearance = (): Record<string, unknown> => ({
    rev: appearanceRev,
    theme: settings.theme ?? "system",
    custom_colors:
      settings.theme === "custom"
        ? (settings.custom_theme_colors ?? null)
        : null,
    accent: settings.accent ?? "amber",
    chat_theme: settings.chat_theme ?? "dark",
    visual_style: settings.visual_style ?? "vibrant",
  });
  const commitAppearance = (
    command: string,
    args: Record<string, unknown>,
  ): void => {
    const mapping = APPEARANCE_ARG[command];
    if (mapping === undefined) return;
    const [key, argName] = mapping;
    settings[key] = clone(args[argName]);
    appearanceRev += 1;
    const snapshot = committedAppearance();
    for (const label of CONSUMER_LABELS) {
      emitted.push({ label, event: "appearance", payload: clone(snapshot) });
    }
    if (CONSUMER_LABELS.includes(arg.label)) {
      for (const listener of listeners) {
        if (listener.eventName === "appearance") {
          callbacks.get(listener.handlerId)?.({
            event: "appearance",
            id: 0,
            payload: clone(snapshot),
          });
        }
      }
    }
  };

  const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

  if (arg.storageThrows) {
    const blocked = (): never => {
      throw new Error("storage blocked by test");
    };
    Storage.prototype.getItem = blocked;
    Storage.prototype.setItem = blocked;
  }

  const handlers: Handlers = {
    get_app_settings: () => clone(settings),
    get_default_settings: () => clone(defaults),
    get_app_dir_path: () => "/tmp/vox-test",
    // Brain connection test and model listing: a healthy endpoint by default.
    brain_test_connection: () => ({
      ok: true,
      kind: "Ok",
      latency_ms: 1,
      model_count: null,
    }),
    brain_fetch_models: () => [],
    get_available_models: () => [],
    list_personas: () => [],
    list_tts_voices: () => [],
    list_natural_voices: () => [],
    // Typed idle/empty answers for commands whose result the Experimental,
    // Advanced, Post Process and Debug pages dereference. Shapes follow
    // src/bindings.ts. Do NOT fall back to null for these.
    list_tts_composer_providers: () => [],
    // Spoken Replies voice picker (TtsSettings) dereferences the list: an
    // unhandled command resolves to null and crashes the Experimental page.
    list_tts_voices: () => [],
    // Settings store device lists (Vec<AudioDevice> payloads).
    get_available_microphones: () => [],
    get_available_output_devices: () => [],
    agent_job_status: () => ({
      state: "idle",
      project: null,
      elapsed_secs: 0,
      result: null,
    }),
    agent_job_cancel: () => false,
    converse_status: () => "idle",
    converse_start: () => null,
    converse_stop: () => false,
    check_apple_intelligence_available: () => false,
    get_available_accelerators: () => ({
      transcribe: [],
      ort: [],
      gpu_devices: [],
    }),
    get_history_entries: () => ({ entries: [], has_more: false }),
    search_history_entries: () => [],
    memory_list_all: () => [],
    is_laptop: () => false,
    get_log_dir_path: () => "/tmp/vox-test/logs",
    get_audio_file_path: () => "/tmp/vox-test/audio.wav",
    get_microphone_channels: () => 1,
    check_custom_sounds: () => ({ start: false, stop: false }),
    get_model_load_status: () => ({ is_loaded: false, current_model: null }),
    get_windows_microphone_permission_status: () => ({
      supported: false,
      overall_access: "unknown",
      device_access: "unknown",
      app_access: "unknown",
      desktop_app_access: "unknown",
    }),
    "plugin:macos-permissions|check_accessibility_permission": () => true,
    "plugin:macos-permissions|check_microphone_permission": () => true,
    "plugin:macos-permissions|request_accessibility_permission": () => true,
    "plugin:macos-permissions|request_microphone_permission": () => true,
    // Engine catalog as the macOS backend reports it (same order as the Rust catalog).
    list_tts_engines: () => [
      {
        id: "phonon",
        recommended: true,
        runtime: "pure_rust",
        requires_download: true,
        size_bytes: 65051089,
        english_only: true,
        supports_voice_choice: false,
        supports_rate: false,
        supported_on_platform: true,
        license: "Apache-2.0",
      },
      {
        id: "natural",
        recommended: false,
        runtime: "onnx_runtime",
        requires_download: true,
        size_bytes: 120673052,
        english_only: false,
        supports_voice_choice: true,
        supports_rate: true,
        supported_on_platform: true,
        license: "Apache-2.0",
      },
      {
        id: "system",
        recommended: false,
        runtime: "operating_system",
        requires_download: false,
        size_bytes: 0,
        english_only: false,
        supports_voice_choice: true,
        supports_rate: true,
        supported_on_platform: true,
        license: "OS-provided",
      },
    ],
    "plugin:os|locale": () => "en-US",
    "plugin:event|listen": (args) => {
      const handlerId = Number(args.handler);
      const eventName = String(args.event);
      listeners.push({ eventName, handlerId });
      return handlerId;
    },
    "plugin:event|unlisten": (args) => {
      const eventId = Number(args.eventId);
      const index = listeners.findIndex((l) => l.handlerId === eventId);
      if (index >= 0) listeners.splice(index, 1);
      return null;
    },
  };

  const invoke = (
    command: string,
    args: Record<string, unknown> = {},
  ): Promise<unknown> => {
    calls.push({ command, args: clone(args) });
    if (command in APPEARANCE_ARG) {
      commitAppearance(command, args);
      return Promise.resolve(null);
    }
    if (command.startsWith("change_") && command.endsWith("_setting")) {
      return Promise.resolve(null);
    }
    if (command in overrides) {
      // `{ "__pending__": true }` makes the command never answer (a stalled request).
      const override = overrides[command] as { __pending__?: boolean } | null;
      if (override !== null && override?.__pending__ === true) {
        return new Promise<unknown>(() => undefined);
      }
      return Promise.resolve(clone(overrides[command]));
    }
    const handler = handlers[command];
    return Promise.resolve(handler ? handler(args) : null);
  };

  const transformCallback = (
    callback?: (message: unknown) => void,
    _once?: boolean,
  ): number => {
    const id = nextId;
    nextId += 1;
    if (callback) callbacks.set(id, callback);
    return id;
  };

  win.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback,
    unregisterCallback: (id: number) => {
      callbacks.delete(id);
    },
    runCallback: (id: number, data: unknown) => {
      callbacks.get(id)?.(data);
    },
    convertFileSrc: (filePath: string) => filePath,
    metadata: {
      currentWindow: { label: arg.label },
      currentWebview: { windowLabel: arg.label, label: arg.label },
    },
    plugins: {},
  };

  // @tauri-apps/api 2.11 unlistens through this object on unmount; the real runtime injects it.
  // Without it, a StrictMode unlisten throws and the whole settings window goes blank.
  win.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => {},
  };

  win.__TAURI_OS_PLUGIN_INTERNALS__ = {
    platform: arg.platform,
    arch: "aarch64",
    family: arg.platform === "windows" ? "windows" : "unix",
    os_type: arg.platform,
    version: "0.0.0",
    eol: "\n",
  };

  // The Vite dev server serves no favicon; without an icon link Chrome requests /favicon.ico and
  // logs a 404 that the real Tauri webview never produces. An empty data: icon removes that artifact.
  document.addEventListener("DOMContentLoaded", () => {
    if (document.querySelector('link[rel="icon"]') === null) {
      const icon = document.createElement("link");
      icon.rel = "icon";
      icon.href = "data:,";
      document.head.appendChild(icon);
    }
  });

  win.__voxMock = {
    setSetting: (key: string, value: unknown) => {
      settings[key] = value;
      const notify = listeners
        .filter((l) => l.eventName === "settings-changed")
        .map((l) => l.handlerId);
      for (const id of notify) {
        callbacks.get(id)?.({
          event: "settings-changed",
          id: 0,
          payload: { setting: key },
        });
      }
    },
    setCommand: (command: string, value: unknown) => {
      overrides[command] = clone(value);
    },
    emit: (eventName: string, payload: unknown) => {
      const notify = listeners
        .filter((l) => l.eventName === eventName)
        .map((l) => l.handlerId);
      for (const id of notify) {
        callbacks.get(id)?.({ event: eventName, id: 0, payload });
      }
    },
    calls,
    label: arg.label,
    appearance: () => committedAppearance(),
    emitted,
  };
}

/**
 * Installs the Tauri mock. Call BEFORE `page.goto`.
 */
export async function installTauriMock(
  page: Page,
  options: TauriMockOptions = {},
): Promise<void> {
  const arg: MockInitArg = {
    platform: options.platform ?? "macos",
    defaults: { ...DEFAULT_MOCK_SETTINGS },
    settings: { ...DEFAULT_MOCK_SETTINGS, ...options.settings },
    storageThrows: options.storageThrows ?? false,
    commands: { ...options.commands },
    label: options.label ?? "main",
  };
  await page.addInitScript(installTauriMockInPage, arg);
}

/**
 * Replaces the raw result of one command at runtime (for example a model status
 * that changes after an install). Does not emit anything; pair it with `emitMockEvent`.
 */
export async function setMockCommand(
  page: Page,
  command: string,
  value: unknown,
): Promise<void> {
  await page.evaluate(
    ({ c, v }) => {
      const mock = (
        window as unknown as {
          __voxMock: { setCommand: (c: string, v: unknown) => void };
        }
      ).__voxMock;
      mock.setCommand(c, v);
    },
    { c: command, v: value },
  );
}

/** Delivers a backend event to the frontend's `listen` subscribers. */
export async function emitMockEvent(
  page: Page,
  eventName: string,
  payload: unknown,
): Promise<void> {
  await page.evaluate(
    ({ e, p }) => {
      const mock = (
        window as unknown as {
          __voxMock: { emit: (e: string, p: unknown) => void };
        }
      ).__voxMock;
      mock.emit(e, p);
    },
    { e: eventName, p: payload },
  );
}

/**
 * Flips one setting at runtime (for example post_process_enabled, debug_mode,
 * experimental_enabled, app_language). Emits `settings-changed` so the app
 * refreshes its settings store.
 */
export async function setMockSetting(
  page: Page,
  key: string,
  value: unknown,
): Promise<void> {
  await page.evaluate(
    ({ k, v }) => {
      const mock = (
        window as unknown as {
          __voxMock: { setSetting: (k: string, v: unknown) => void };
        }
      ).__voxMock;
      mock.setSetting(k, v);
    },
    { k: key, v: value },
  );
}

/**
 * The argument objects of every invoke of `command`, in call order. Each call is recorded
 * even when the command resolves to null (change_* commands included).
 */
export async function getMockCalls(
  page: Page,
  command: string,
): Promise<Record<string, unknown>[]> {
  return page.evaluate((name) => {
    const mock = (
      window as unknown as {
        __voxMock: {
          calls: Array<{ command: string; args: Record<string, unknown> }>;
        };
      }
    ).__voxMock;
    return mock.calls
      .filter((call) => call.command === name)
      .map((call) => call.args);
  }, command);
}

/** The committed appearance snapshot the mock would publish now (rev starts at 1). */
export async function getMockAppearance(page: Page): Promise<MockAppearance> {
  return page.evaluate(
    () =>
      (
        window as unknown as { __voxMock: { appearance: () => unknown } }
      ).__voxMock.appearance() as MockAppearance,
  );
}

/**
 * Every `appearance` delivery the page recorded from the appearance writers, in commit order.
 * The harness forwards each entry to the page whose window label matches (`emitMockEvent`).
 */
export async function getMockEmits(page: Page): Promise<MockEmit[]> {
  return page.evaluate(() =>
    (
      window as unknown as { __voxMock: { emitted: MockEmit[] } }
    ).__voxMock.emitted.map((entry) => ({ ...entry })),
  );
}
