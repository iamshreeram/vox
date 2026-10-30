import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands } from "@/bindings";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import { useSettings } from "../../../hooks/useSettings";
import {
  ORB_PRESETS,
  ORB_PX_MAX,
  ORB_PX_MIN,
  ORB_PX_STEP,
  ORB_SIZES,
  SURFACE_OPTIONS,
  effectiveOrbPx,
  presetFor,
  surfaceHints,
  surfaceToPresentation,
  surfaceValue,
  toggleState,
  type OrbSupportState,
  type SurfaceOption,
} from "./floatingOrbForm";

interface FloatingOrbSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

/**
 * A value the user just chose, kept until the stored value moves on from the one it was chosen
 * over. The orb settings are written by their own commands, not by `updateSetting`, so until the
 * refreshed settings show the change the choice is shown from here.
 */
interface Pending<T> {
  from: T;
  value: T;
}

function shownValue<T>(stored: T, pending: Pending<T> | null): T {
  return pending !== null && pending.from === stored ? pending.value : stored;
}

/** A free size to store in logical pixels, or `null` to return to the preset. */
interface SizeChoice {
  px: number | null;
}

/** Quiet time after the last slider change before the size is stored. */
const SIZE_DEBOUNCE_MS = 150;

/**
 * The size commits waiting to be stored. Only one `changeOrbSizePxSetting` call is in flight at a
 * time; the latest choice always wins.
 */
interface SizeQueue {
  wanted: SizeChoice | null;
  timer: ReturnType<typeof setTimeout> | null;
  sending: boolean;
}

/** Sends the newest queued size now, unless a call is already in flight (it picks it up). */
function flushSizeQueue(queue: SizeQueue): void {
  if (queue.timer !== null) {
    clearTimeout(queue.timer);
    queue.timer = null;
  }
  if (queue.wanted === null || queue.sending) return;
  const next = queue.wanted;
  queue.wanted = null;
  commands.changeOrbSizePxSetting(next.px).catch(() => undefined);
}

/**
 * The Floating orb rows in Settings > General > Conversation: the Show orb toggle, the orb size
 * (slider, presets and reset), and Reset position. Support is asked once on mount; orb settings go
 * only through their own commands.
 */
export const FloatingOrbSettings: React.FC<FloatingOrbSettingsProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, refreshSettings } = useSettings();

    const [support, setSupport] = useState<OrbSupportState | null>(null);
    const [busy, setBusy] = useState(false);
    const [pendingVisible, setPendingVisible] = useState<Pending<
      boolean | null
    > | null>(null);
    const [pendingSurface, setPendingSurface] =
      useState<Pending<SurfaceOption> | null>(null);
    const [sizePreview, setSizePreview] = useState<SizeChoice | null>(null);
    const sizeQueue = useRef<SizeQueue>({
      wanted: null,
      timer: null,
      sending: false,
    });

    const storedVisible = getSetting("orb_visible") ?? null;
    const storedPx = getSetting("orb_size_px") ?? null;
    const orbPosition = getSetting("orb_position");

    const visible = shownValue(storedVisible, pendingVisible);
    const shownPx = effectiveOrbPx(
      getSetting("orb_size"),
      sizePreview !== null ? sizePreview.px : storedPx,
    );
    const sizeLabel = t("settings.floatingOrb.size.valueLabel", {
      px: shownPx,
    });
    const toggle = toggleState(support, visible);

    const storedSurface = surfaceValue(getSetting("chat_presentation"));
    const surface = shownValue(storedSurface, pendingSurface);
    const hints = surfaceHints({
      converseChatLog: getSetting("converse_chat_log"),
      surface,
      supported: support === null ? null : support.supported,
    });

    useEffect(() => {
      let cancelled = false;
      commands.getOrbSupport().then(
        (value) => {
          if (!cancelled) setSupport(value);
        },
        () => {
          if (!cancelled) setSupport({ supported: false, block: "unknown" });
        },
      );
      return () => {
        cancelled = true;
      };
    }, []);

    // A size still queued when the page unmounts is sent rather than dropped.
    useEffect(() => {
      const queue = sizeQueue.current;
      return () => flushSizeQueue(queue);
    }, []);

    // Runs one orb command, drops the pending choice if it failed, then refreshes settings.
    const runOrbCommand = async (
      command: () => Promise<unknown>,
      revert: () => void,
    ): Promise<void> => {
      setBusy(true);
      try {
        await command();
      } catch {
        revert();
      } finally {
        setBusy(false);
      }
      await refreshSettings();
    };

    const setVisible = (enabled: boolean): Promise<void> => {
      setPendingVisible({ from: storedVisible, value: enabled });
      return runOrbCommand(
        () => commands.changeOrbVisibleSetting(enabled),
        () => setPendingVisible(null),
      );
    };

    // The surface choice goes only through its gated presentation command.
    const setSurface = (option: SurfaceOption): Promise<void> => {
      setPendingSurface({ from: storedSurface, value: option });
      return runOrbCommand(
        () =>
          commands.changeChatPresentationSetting(surfaceToPresentation(option)),
        () => setPendingSurface(null),
      );
    };

    const clearSizeTimer = (queue: SizeQueue): void => {
      if (queue.timer !== null) {
        clearTimeout(queue.timer);
        queue.timer = null;
      }
    };

    // Sends queued sizes one at a time, then refreshes the stored settings once.
    const sendSizes = async (): Promise<void> => {
      const queue = sizeQueue.current;
      clearSizeTimer(queue);
      if (queue.sending) return;
      queue.sending = true;
      try {
        while (queue.wanted !== null) {
          const next = queue.wanted;
          queue.wanted = null;
          try {
            await commands.changeOrbSizePxSetting(next.px);
          } catch {
            // The refreshed settings below show what was actually stored.
          }
        }
      } finally {
        queue.sending = false;
      }
      await refreshSettings();
      if (!queue.sending && queue.wanted === null && queue.timer === null) {
        setSizePreview(null);
      }
    };

    // Shows a size at once and queues it: after a quiet period, or at once when `immediate`.
    const chooseSize = (choice: SizeChoice, immediate: boolean): void => {
      setSizePreview(choice);
      const queue = sizeQueue.current;
      queue.wanted = choice;
      clearSizeTimer(queue);
      if (immediate) {
        void sendSizes();
        return;
      }
      queue.timer = setTimeout(() => {
        queue.timer = null;
        void sendSizes();
      }, SIZE_DEBOUNCE_MS);
    };

    const resetPosition = (): Promise<void> =>
      runOrbCommand(
        () => commands.resetOrbPosition(),
        () => undefined,
      );

    const controlsDisabled = toggle.disabled || busy;
    const sizeDisabled = toggle.disabled;

    return (
      <div data-testid="floating-orb-group" className="flex flex-col">
        <p className="text-xs text-muted px-3">
          {t("settings.floatingOrb.description")}
        </p>

        <SettingContainer
          title={t("settings.floatingOrb.show.title")}
          description={t("settings.floatingOrb.show.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          disabled={controlsDisabled}
        >
          <label
            className={`relative inline-flex h-6 w-11 ${controlsDisabled ? "cursor-not-allowed" : "cursor-pointer"}`}
          >
            <input
              type="checkbox"
              data-testid="floating-orb-show"
              className="peer absolute inset-0 z-10 m-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-not-allowed"
              checked={toggle.checked}
              disabled={controlsDisabled}
              onChange={(event) => void setVisible(event.target.checked)}
            />
            <div className="relative pointer-events-none h-6 w-11 rounded-full bg-mid-gray/20 peer-focus-visible:ring-4 peer-focus-visible:ring-logo-primary peer-checked:bg-logo-primary after:absolute after:start-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-mid-gray/40 after:bg-white after:transition-all after:content-[''] peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full" />
          </label>
        </SettingContainer>

        {toggle.reasonKey !== null && (
          <p
            data-testid="floating-orb-reason"
            className="text-xs text-muted px-3"
          >
            {t(toggle.reasonKey)}
          </p>
        )}

        <SettingContainer
          title={t("settings.floatingOrb.surface.title")}
          description={t("settings.floatingOrb.surface.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          disabled={controlsDisabled}
        >
          <div className="flex flex-col items-end gap-2">
            <select
              data-testid="floating-orb-surface"
              aria-label={t("settings.floatingOrb.surface.title")}
              className="rounded-md border border-mid-gray/30 bg-transparent px-2 py-1 text-sm cursor-pointer disabled:cursor-not-allowed"
              value={surface}
              disabled={controlsDisabled}
              onChange={(event) => {
                const option = SURFACE_OPTIONS.find(
                  (candidate) => candidate === event.target.value,
                );
                if (option !== undefined) void setSurface(option);
              }}
            >
              <option value="auto">
                {t("settings.floatingOrb.surface.auto")}
              </option>
              <option value="bubbles">
                {t("settings.floatingOrb.surface.bubbles")}
              </option>
              <option value="window">
                {t("settings.floatingOrb.surface.window")}
              </option>
            </select>
            {hints.chatLogOff && (
              <p
                data-testid="floating-orb-surface-hint-chatlog"
                className="text-xs text-muted text-end"
              >
                {t("settings.floatingOrb.hints.chatLogOff")}
              </p>
            )}
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.floatingOrb.size.title")}
          description={t("settings.floatingOrb.size.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          disabled={sizeDisabled}
        >
          <div className="flex flex-col items-end gap-2">
            <div className="flex items-center gap-3">
              <input
                type="range"
                data-testid="floating-orb-size-slider"
                aria-label={t("settings.floatingOrb.size.title")}
                aria-valuetext={sizeLabel}
                className="w-40 cursor-pointer disabled:cursor-not-allowed"
                min={ORB_PX_MIN}
                max={ORB_PX_MAX}
                step={ORB_PX_STEP}
                value={shownPx}
                disabled={sizeDisabled}
                onChange={(event) =>
                  chooseSize({ px: Number(event.target.value) }, false)
                }
                onPointerUp={() => {
                  // Releasing the pointer sends a pending change at once.
                  if (sizeQueue.current.wanted !== null) void sendSizes();
                }}
              />
              <span
                data-testid="floating-orb-size-value"
                className="w-16 text-end text-sm tabular-nums"
              >
                {sizeLabel}
              </span>
            </div>
            <div
              role="group"
              aria-label={t("settings.floatingOrb.size.presets")}
              className="flex items-center gap-2"
            >
              {ORB_SIZES.map((preset) => (
                <Button
                  key={preset}
                  variant="secondary"
                  size="sm"
                  data-testid={`floating-orb-size-${preset}`}
                  aria-pressed={presetFor(shownPx) === preset}
                  disabled={sizeDisabled}
                  onClick={() => chooseSize({ px: ORB_PRESETS[preset] }, true)}
                >
                  {t(`settings.floatingOrb.size.${preset}`)}
                </Button>
              ))}
              <Button
                variant="secondary"
                size="sm"
                data-testid="floating-orb-size-reset"
                disabled={sizeDisabled || storedPx === null}
                onClick={() => chooseSize({ px: null }, true)}
              >
                {t("settings.floatingOrb.size.reset")}
              </Button>
            </div>
          </div>
        </SettingContainer>

        <SettingContainer
          title={t("settings.floatingOrb.resetPosition.title")}
          description={t("settings.floatingOrb.resetPosition.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Button
            variant="secondary"
            size="md"
            data-testid="floating-orb-reset"
            disabled={orbPosition === null || orbPosition === undefined || busy}
            onClick={() => void resetPosition()}
          >
            {t("settings.floatingOrb.resetPosition.button")}
          </Button>
        </SettingContainer>
      </div>
    );
  });

FloatingOrbSettings.displayName = "FloatingOrbSettings";
