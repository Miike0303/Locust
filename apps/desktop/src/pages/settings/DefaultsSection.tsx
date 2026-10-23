import { useQuery } from "@tanstack/react-query";
import BufferedSetting, { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import { getConfig, getProviders } from "../../lib/api";
import { useT } from "../../lib/i18n";
import SectionState from "./SectionState";
import useSettingsSave from "./useSettingsSave";

export default function DefaultsSection() {
  const t = useT();
  const configQuery = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const providersQuery = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const { data: config } = configQuery;
  const { data: providers } = providersQuery;
  const saveSettings = useSettingsSave();

  const save = async (key: string, value: any) => {
    return Boolean(await saveSettings({ [key]: value }));
  };

  const queries = [configQuery, providersQuery];
  const failed = queries.find(query => query.isError);
  if (failed) return <SectionState title={t("settings.defaults.title")} state="error" error={failed.error} onRetry={() => {
    for (const query of queries) if (query.isError) void query.refetch();
  }} />;
  if (queries.some(query => query.isPending) || !config || !providers) return <SectionState title={t("settings.defaults.title")} state="loading" />;

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
