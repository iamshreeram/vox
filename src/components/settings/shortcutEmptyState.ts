// The empty and not-found states of a shortcut row (UI enterprise S4a, U7). When the binding list is
// empty, every shortcut row used to show the same generic title ("Vox Shortcuts") and the same
// sentence, so two identical rows appeared. Each row now titles itself with its own binding name
// and explains the state. Pure: the components pass the result through i18next.

export interface ShortcutEmptyModel {
  /** Translation key of the row title: this shortcut's own name. */
  titleKey: string;
  /** Translation key of the explanation shown under the title. */
  bodyKey: string;
}

export function shortcutEmptyModel(shortcutId: string): ShortcutEmptyModel {
  return {
    titleKey: `settings.general.shortcut.bindings.${shortcutId}.name`,
    bodyKey: "settings.general.shortcut.noneHint",
  };
}
