import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type OrbSize } from "@/bindings";
import { Button } from "../../ui/Button";
import { SettingContainer } from "../../ui/SettingContainer";
import { useSettings } from "../../../hooks/useSettings";
import {
  ORB_SIZES,
  sizeValue,
  toggleState,
  type OrbSizeValue,
  type OrbSupportState,
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

/**
 * The Floating orb rows in Settings > General > Conversation: the Show orb toggle, the orb size,
 * and Reset position. Support is asked once on mount; orb settings go only through their own
 * commands.
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
    const [pendingSize, setPendingSize] =
      useState<Pending<OrbSizeValue> | null>(null);

    const storedVisible = getSetting("orb_visible") ?? null;
    const storedSize = sizeValue(getSetting("orb_size"));
    const orbPosition = getSetting("orb_position");

    const visible = shownValue(storedVisible, pendingVisible);
    const size = shownValue(storedSize, pendingSize);
    const toggle = toggleState(support, visible);

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

    const setSize = (next: OrbSizeValue): Promise<void> => {
      setPendingSize({ from: storedSize, value: next });
      return runOrbCommand(
        () => commands.changeOrbSizeSetting(next as OrbSize),
        () => setPendingSize(null),
      );
    };

    const resetPosition = (): Promise<void> =>
      runOrbCommand(
        () => commands.resetOrbPosition(),
        () => undefined,
      );

    const controlsDisabled = toggle.disabled || busy;

    return (
      <div data-testid="floating-orb-group" className="flex flex-col">
        <p className="text-xs text-mid-gray px-4">
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
            <div className="relative pointer-events-none h-6 w-11 rounded-full bg-mid-gray/20 peer-focus:ring-4 peer-focus:ring-logo-primary peer-checked:bg-logo-primary after:absolute after:start-[2px] after:top-[2px] after:h-5 after:w-5 after:rounded-full after:border after:border-gray-300 after:bg-white after:transition-all after:content-[''] peer-checked:after:translate-x-full rtl:peer-checked:after:-translate-x-full" />
          </label>
        </SettingContainer>

        {toggle.reasonKey !== null && (
          <p
            data-testid="floating-orb-reason"
            className="text-xs text-mid-gray px-4"
          >
            {t(toggle.reasonKey)}
          </p>
        )}

        <SettingContainer
          title={t("settings.floatingOrb.size.title")}
          description={t("settings.floatingOrb.size.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
          disabled={controlsDisabled}
        >
          <select
            data-testid="floating-orb-size"
            aria-label={t("settings.floatingOrb.size.title")}
            className="px-2 py-1 text-sm rounded-md border border-mid-gray/80 bg-mid-gray/10"
            value={size}
            disabled={controlsDisabled}
            onChange={(event) => void setSize(sizeValue(event.target.value))}
          >
            {ORB_SIZES.map((option) => (
              <option key={option} value={option}>
                {t(`settings.floatingOrb.size.${option}`)}
              </option>
            ))}
          </select>
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
