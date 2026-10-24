import React from "react";
import { useTranslation } from "react-i18next";
import { CollapsibleSettingsGroup } from "../ui/CollapsibleSettingsGroup";
import { useSettings } from "../../hooks/useSettings";
import { PLACED_RENDERERS } from "./placedRenderers";
import {
  groupsForSection,
  visibleControls,
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

// Presentational: renders the placed groups of one section. Renders nothing while
// the Experimental gate is off, so the moved controls stay hidden as before.
export const PlacedGroupsView: React.FC<PlacedGroupsViewProps> = ({
  section,
  experimentalEnabled,
  flags,
  render,
  t,
}) => {
  if (experimentalEnabled !== true) return null;
  const groups = groupsForSection(section);
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
          {visibleControls(group, flags).map((id) => (
            <React.Fragment key={id}>{render[id]()}</React.Fragment>
          ))}
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
