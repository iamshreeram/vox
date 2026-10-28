import React from "react";
import { useTranslation } from "react-i18next";
import { CollapsibleSettingsGroup } from "../ui/CollapsibleSettingsGroup";
import { useSettings } from "../../hooks/useSettings";
import { PLACED_RENDERERS } from "./placedRenderers";
import {
  visibleControls,
  visibleGroups,
  type ControlFlag,
  type ControlId,
  type PlacedSection,
} from "./settingsPlacement";

interface PlacedGroupsViewProps {
  section: PlacedSection;
  experimentalEnabled: boolean;
  flags: Record<ControlFlag, boolean>;
  render: Record<ControlId, () => React.ReactNode>;
  /** Translator; passed in so this view stays pure and server-renderable. */
  t: (key: string) => string;
}

// Presentational: renders the placed groups of one section. Experimental groups
// render only while the Experimental switch is on; the rest always render.
export const PlacedGroupsView: React.FC<PlacedGroupsViewProps> = ({
  section,
  experimentalEnabled,
  flags,
  render,
  t,
}) => {
  const groups = visibleGroups(section, experimentalEnabled);
  if (groups.length === 0) return null;

  return (
    <>
      {groups.map((group) => (
        <CollapsibleSettingsGroup
          key={group.id}
          id={group.id}
          title={t(group.titleKey)}
          description={
            group.descriptionKey ? t(group.descriptionKey) : undefined
          }
          defaultOpen={group.defaultOpen}
        >
          {visibleControls(group, flags, experimentalEnabled).map((id) => {
            const subTitleKey = group.controls.find(
              (entry) => entry.id === id,
            )?.subTitleKey;
            return (
              <React.Fragment key={id}>
                {subTitleKey ? (
                  <div className="px-4 pt-3 pb-1 text-xs font-medium uppercase tracking-wide text-mid-gray">
                    {t(subTitleKey)}
                  </div>
                ) : null}
                {render[id]()}
              </React.Fragment>
            );
          })}
        </CollapsibleSettingsGroup>
      ))}
    </>
  );
};

interface PlacedGroupsProps {
  section: PlacedSection;
}

// Connected: reads the live settings and renders the real controls for one section.
export const PlacedGroups: React.FC<PlacedGroupsProps> = ({ section }) => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const experimentalEnabled = getSetting("experimental_enabled") === true;
  const flags = {
    memoryEnabled: getSetting("memory_enabled") || false,
    agentBridgeEnabled: getSetting("agent_bridge_enabled") || false,
  };

  return (
    <PlacedGroupsView
      section={section}
      experimentalEnabled={experimentalEnabled}
      flags={flags}
      render={PLACED_RENDERERS}
      t={t}
    />
  );
};
