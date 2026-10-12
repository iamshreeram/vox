import { createContext } from "react";

export type Density = "comfortable" | "compact";

export interface SettingsGroupContextValue {
  density: Density;
  visible: boolean;
}

export const SettingsGroupContext = createContext<SettingsGroupContextValue>({
  density: "comfortable",
  visible: true,
});

/** Padding/height classes for rows inside a grouped container. Comfortable matches the historic grouped row. */
export function groupedRowClass(d: Density): string {
  return d === "compact" ? "min-h-10 px-3 py-1" : "min-h-12 px-4 p-2";
}

/** Tooltips only render while their group body is visible (a hidden group must not leave a portal open). */
export function shouldShowTooltip(
  showTooltip: boolean,
  visible: boolean,
): boolean {
  return showTooltip && visible;
}
