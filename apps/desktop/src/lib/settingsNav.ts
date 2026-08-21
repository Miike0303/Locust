import type { MessageKey } from "./i18n";

export const SETTINGS_SECTIONS = [
  { id: "providers", labelKey: "settings.nav.providers" },
  { id: "defaults", labelKey: "settings.nav.defaults" },
  { id: "appearance", labelKey: "settings.nav.appearance" },
  { id: "glossary", labelKey: "settings.nav.glossary" },
  { id: "history", labelKey: "settings.nav.history" },
  { id: "data", labelKey: "settings.nav.data" },
] as const satisfies ReadonlyArray<{ id: string; labelKey: MessageKey }>;

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
