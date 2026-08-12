export const SETTINGS_SECTIONS = [
  { id: "providers", label: "Providers" },
  { id: "defaults", label: "Translation Defaults" },
  { id: "appearance", label: "Appearance" },
  { id: "glossary", label: "Glossary" },
  { id: "history", label: "History" },
  { id: "data", label: "Data" },
] as const;

export type SettingsSectionId = (typeof SETTINGS_SECTIONS)[number]["id"];
export type OperationalShortcut =
  | "provider-settings"
  | "manage-glossary"
  | "manage-backups";

const DEFAULT_SECTION: SettingsSectionId = "providers";

export function parseSettingsSectionParam(search: string): SettingsSectionId {
  const candidate = new URLSearchParams(search).get("section");
  return SETTINGS_SECTIONS.some(({ id }) => id === candidate)
    ? (candidate as SettingsSectionId)
    : DEFAULT_SECTION;
}

export function buildSettingsPath(section: SettingsSectionId): string {
  return `/settings?section=${encodeURIComponent(section)}`;
}

export function operationalShortcutTarget(shortcut: OperationalShortcut): {
  section: SettingsSectionId;
  path: string;
} {
  const section: SettingsSectionId =
    shortcut === "manage-glossary"
      ? "glossary"
      : shortcut === "manage-backups"
        ? "data"
        : "providers";
  return { section, path: buildSettingsPath(section) };
}
