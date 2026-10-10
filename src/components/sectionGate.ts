interface GatedSectionConfig {
  enabled: (settings: any) => boolean;
}

export const shouldFallBackToGeneral = (
  section: string,
  settings: unknown,
  sections: Record<string, GatedSectionConfig>,
): boolean => {
  if (settings === null || settings === undefined) {
    return false;
  }

  return !(sections[section]?.enabled(settings) ?? true);
};
