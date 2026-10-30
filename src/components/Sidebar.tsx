import React from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  Brain,
  Palette,
  Cog,
  FlaskConical,
  History,
  Info,
  Sparkles,
  Cpu,
  Mic,
} from "lucide-react";
import VoxTextLogo from "./icons/VoxTextLogo";
import { useSettings } from "../hooks/useSettings";
import {
  SECTION_ICON_CLASS,
  sidebarIconClass,
  sidebarItemClass,
  type SidebarStyle,
} from "./sidebarTheme";
import {
  AppearanceSettings,
  GeneralSettings,
  AdvancedSettings,
  HistorySettings,
  DebugSettings,
  AboutSettings,
  PostProcessingSettings,
  ModelsSettings,
  AgentsSettings,
  BrainSettings,
} from "./settings";
import { SettingsSearchBox } from "./settings/search/SettingsSearchBox";
import { cancelReveal } from "./settings/search/settingsReveal";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

// Compile-time check that every sidebar section has an icon color class. If a
// section is added to SECTIONS_CONFIG without a matching SECTION_ICON_CLASS
// entry, this assignment fails to type-check.
const _iconCoverage: Record<SidebarSection, string> = SECTION_ICON_CLASS;
void _iconCoverage;

interface IconProps {
  width?: number | string;
  height?: number | string;
  size?: number | string;
  className?: string;
  [key: string]: any;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
  enabled: (settings: any) => boolean;
}

export const SECTIONS_CONFIG = {
  general: {
    labelKey: "sidebar.general",
    icon: Mic,
    component: GeneralSettings,
    enabled: () => true,
  },
  appearance: {
    labelKey: "sidebar.appearance",
    icon: Palette,
    component: AppearanceSettings,
    enabled: () => true,
  },
  history: {
    labelKey: "sidebar.history",
    icon: History,
    component: HistorySettings,
    enabled: () => true,
  },
  models: {
    labelKey: "sidebar.models",
    icon: Cpu,
    component: ModelsSettings,
    enabled: () => true,
  },
  brain: {
    labelKey: "sidebar.brain",
    icon: Brain,
    component: BrainSettings,
    enabled: () => true,
  },
  advanced: {
    labelKey: "sidebar.advanced",
    icon: Cog,
    component: AdvancedSettings,
    enabled: () => true,
  },
  postprocessing: {
    labelKey: "sidebar.postProcessing",
    icon: Sparkles,
    component: PostProcessingSettings,
    enabled: (settings) => settings?.post_process_enabled ?? false,
  },
  agents: {
    labelKey: "sidebar.agents",
    icon: Bot,
    component: AgentsSettings,
    enabled: (settings) => settings?.experimental_enabled ?? false,
  },
  debug: {
    labelKey: "sidebar.debug",
    icon: FlaskConical,
    component: DebugSettings,
    enabled: (settings) => settings?.debug_mode ?? false,
  },
  about: {
    labelKey: "sidebar.about",
    icon: Info,
    component: AboutSettings,
    enabled: () => true,
  },
} as const satisfies Record<string, SectionConfig>;

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const style: SidebarStyle =
    settings?.visual_style === "restrained" ? "restrained" : "vibrant";

  const availableSections = Object.entries(SECTIONS_CONFIG)
    .filter(([_, config]) => config.enabled(settings))
    .map(([id, config]) => ({ id: id as SidebarSection, ...config }));

  // Same visibility rules as the nav list (enabled predicates + settings).
  const isSectionEnabled = (section: SidebarSection): boolean =>
    SECTIONS_CONFIG[section].enabled(settings);

  return (
    <div className="flex flex-col w-40 h-full border-e border-mid-gray/20 items-center px-2">
      <VoxTextLogo width={120} className="m-4" />
      <div className="flex flex-col w-full items-center gap-1 pt-2 border-t border-mid-gray/20">
        <SettingsSearchBox
          isSectionEnabled={isSectionEnabled}
          experimentalEnabled={settings?.experimental_enabled === true}
          onNavigate={onSectionChange}
        />
        {availableSections.map((section) => {
          const Icon = section.icon;
          const isActive = activeSection === section.id;

          return (
            <div
              key={section.id}
              className={sidebarItemClass(isActive, style)}
              onClick={() => {
                // Normal sidebar navigation cancels any pending search reveal.
                cancelReveal();
                onSectionChange(section.id);
              }}
            >
              <Icon
                width={24}
                height={24}
                className={`shrink-0 ${sidebarIconClass(
                  section.id,
                  isActive,
                  style,
                )}`}
              />
              <p
                className="text-sm font-medium truncate"
                title={t(section.labelKey)}
              >
                {t(section.labelKey)}
              </p>
            </div>
          );
        })}
      </div>
    </div>
  );
};
