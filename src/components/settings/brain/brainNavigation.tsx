import { createContext, useContext } from "react";

/** Opens the Brain page from anywhere inside the settings area (App provides it). */
const OpenBrainPageContext = createContext<() => void>(() => undefined);

export const OpenBrainPageProvider = OpenBrainPageContext.Provider;

export function useOpenBrainPage(): () => void {
  return useContext(OpenBrainPageContext);
}
