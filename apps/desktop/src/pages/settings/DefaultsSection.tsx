import { useQuery } from "@tanstack/react-query";
import BufferedSetting, { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import { getConfig, getProviders } from "../../lib/api";
import { useT } from "../../lib/i18n";
import useSettingsSave from "./useSettingsSave";

export default function DefaultsSection() {
  const t = useT();
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const saveSettings = useSettingsSave();

  const save = async (key: string, value: any) => {
    return Boolean(await saveSettings({ [key]: value }));
  };

  if (!config) return null;

  return (
    <div className="space-y-6 border border-border bg-surface rounded-xl p-5">
      <h2 className="text-page font-bold">{t("settings.defaults.title")}</h2>
      <div>
        <label className="text-body font-medium">{t("settings.defaults.provider")}</label>
        <select value={config.default_provider || ""} onChange={(e) => save("default_provider", e.target.value || null)}
          className={`mt-1 ${SETTINGS_INPUT_CLASS}`}>
          <option value="">{t("common.none")}</option>
          {providers?.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </div>
      <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 [&_label]:block [&_label]:min-h-10">
        <BufferedSetting label={t("settings.defaults.sourceLang")} value={config.default_source_lang} required normalize={value => value.trim()} onSave={value => save("default_source_lang", value)} />
        <BufferedSetting label={t("settings.defaults.targetLang")} value={config.default_target_lang} required normalize={value => value.trim()} onSave={value => save("default_target_lang", value)} />
      </div>
      <p className="text-caption text-text-muted -mt-2">{t("settings.defaults.langHint")}</p>
      <BufferedSetting label={value => t("settings.defaults.batchSize", { size: value || t("common.providerDefault") })} type="number" min={1} value={config.default_batch_size ?? ""} placeholder={t("common.providerDefault")} onSave={value => save("default_batch_size", value ? Number(value) : null)} />
      <BufferedSetting label={t("settings.defaults.costLimit")} type="number" min={0} step={0.01} value={config.default_cost_limit ?? ""} placeholder={t("settings.defaults.noLimit")} onSave={value => save("default_cost_limit", value ? Number(value) : null)} />
    </div>
  );
}
