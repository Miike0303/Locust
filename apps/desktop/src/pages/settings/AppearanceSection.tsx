import { useQuery } from "@tanstack/react-query";
import BufferedSetting, { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import { getConfig } from "../../lib/api";
import { TABLE_ROW_HEIGHT_MAX, TABLE_ROW_HEIGHT_MIN, clampTableRowHeight } from "../../lib/appearance";
import { useLocale, useT, type Locale } from "../../lib/i18n";
import useSettingsSave from "./useSettingsSave";

export default function AppearanceSection() {
  const t = useT();
  const { locale, setLocale } = useLocale();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const saveSettings = useSettingsSave();

  const setTheme = async (theme: string) => {
    await saveSettings({ ui: { theme } });
  };

  const setFontSize = async (size: number) => {
    return Boolean(await saveSettings({ ui: { font_size: size } }));
  };

  const setShowSourceColumn = async (show: boolean) => {
    await saveSettings({ ui: { show_source_column: show } });
  };

  const setTableRowHeight = async (height: number) => {
    return Boolean(await saveSettings({ ui: { table_row_height: height } }));
  };

  if (!config) return null;

  return (
    <div className="space-y-6 border border-border bg-surface rounded-xl p-5">
      <h2 className="text-page font-bold">{t("settings.appearance.title")}</h2>
      <div>
        <label className="text-body font-medium">{t("settings.appearance.theme")}</label>
        <div className="flex flex-wrap gap-4 mt-2">
          {(
            [
              ["system", "settings.appearance.theme.system"],
              ["light", "settings.appearance.theme.light"],
              ["dark", "settings.appearance.theme.dark"],
            ] as const
          ).map(([theme, labelKey]) => (
            <label key={theme} className="flex items-center gap-2 cursor-pointer">
              <input type="radio" name="theme" className="accent-accent" checked={config.ui.theme === theme} onChange={() => setTheme(theme)} />
              <span className="text-body">{t(labelKey)}</span>
            </label>
          ))}
        </div>
      </div>
      <BufferedSetting label={value => t("settings.appearance.fontSize", { size: value })} type="range" min={12} max={18} value={config.ui.font_size} onSave={value => setFontSize(Number(value))} />
      <label className="flex items-start gap-2 cursor-pointer">
        <input
          type="checkbox"
          className="mt-0.5 accent-accent"
          checked={config.ui.show_source_column !== false}
          onChange={(e) => setShowSourceColumn(e.target.checked)}
        />
        <span>
          <span className="text-body font-medium">{t("settings.appearance.showSourceColumn")}</span>
          <span className="block text-caption text-text-muted mt-0.5">
            {t("settings.appearance.showSourceColumnHint")}
          </span>
        </span>
      </label>
      <div>
        <BufferedSetting label={value => t("settings.appearance.tableRowHeight", { size: value })} type="range" min={TABLE_ROW_HEIGHT_MIN} max={TABLE_ROW_HEIGHT_MAX} value={clampTableRowHeight(config.ui.table_row_height)} onSave={value => setTableRowHeight(Number(value))} />
        <p className="text-caption text-text-muted mt-0.5">
          {t("settings.appearance.tableRowHeightHint")}
        </p>
      </div>
      <div>
        <label className="text-body font-medium" htmlFor="ui-language">
          {t("settings.appearance.interfaceLanguage")}
        </label>
        <select
          id="ui-language"
          value={locale}
          onChange={(e) => setLocale(e.target.value as Locale)}
          className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
        >
          <option value="en">{t("settings.appearance.locale.en")}</option>
          <option value="es">{t("settings.appearance.locale.es")}</option>
        </select>
      </div>
    </div>
  );
}
