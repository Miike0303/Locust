import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { CheckCircle, Loader, XCircle } from "lucide-react";
import { useState } from "react";
import { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import { checkProviderHealth, getConfig, getProviders } from "../../lib/api";
import { useT } from "../../lib/i18n";
import { resolveProviderReadiness } from "../../lib/providerReadiness";
import { GROK_SUB_PROVIDER_ID } from "../../lib/xaiAuth";
import GrokSubCard from "./GrokSubCard";
import useSettingsSave from "./useSettingsSave";

export default function ProvidersSection() {
  const t = useT();
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders });
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });
  const saveSettings = useSettingsSave();
  const [testing, setTesting] = useState<Record<string, boolean>>({});
  const [results, setResults] = useState<Record<string, { ok: boolean; message: string }>>({});

  const handleTest = async (id: string) => {
    setTesting((p) => ({ ...p, [id]: true }));
    try {
      const r = await checkProviderHealth(id);
      setResults((p) => ({ ...p, [id]: r }));
    } catch (e: any) {
      setResults((p) => ({ ...p, [id]: { ok: false, message: e.message } }));
    }
    setTesting((p) => ({ ...p, [id]: false }));
  };

  const saveKey = async (providerId: string, key: string, value: string) => {
    await saveSettings({ providers: { [providerId]: { [key]: value } } });
  };

  const grokSub = providers?.find((p) => p.id === GROK_SUB_PROVIDER_ID);

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.providers.title")}</h2>
      <GrokSubCard provider={grokSub} model={config?.providers?.[GROK_SUB_PROVIDER_ID]?.model} onModelChange={(model) => void saveKey(GROK_SUB_PROVIDER_ID, "model", model)} onTest={() => handleTest(GROK_SUB_PROVIDER_ID)} testing={!!testing[GROK_SUB_PROVIDER_ID]} testResult={results[GROK_SUB_PROVIDER_ID]} />
      {providers?.filter((p) => p.id !== GROK_SUB_PROVIDER_ID).map((p) => {
        const readiness = resolveProviderReadiness(p.id, providers, config);
        return (
        <div key={p.id} className="border border-border bg-surface rounded-xl p-5">
          <div className="flex flex-wrap items-center gap-2 mb-3">
            <h3 className="text-section font-semibold break-words">{p.name}</h3>
            <span className={clsx("px-2 py-0.5 rounded-full text-caption", p.is_free ? "bg-success-muted text-success" : "bg-warning-muted text-warning")}>
              {p.is_free ? t("settings.providers.free") : t("settings.providers.paid")}
            </span>
            {p.requires_api_key && (
              <span className={clsx(
                "px-2 py-0.5 rounded-full text-caption font-medium",
                readiness.ready
                  ? "bg-success-muted text-success"
                  : "bg-warning-muted text-warning"
              )}>
                {readiness.ready ? t("settings.providers.configured") : t("settings.providers.needsApiKey")}
              </span>
            )}
          </div>

          {p.requires_api_key && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.apiKey")}</label>
              <input
                type="password"
                defaultValue={config?.providers?.[p.id]?.api_key === "***" ? "" : config?.providers?.[p.id]?.api_key || ""}
                onBlur={(e) => saveKey(p.id, "api_key", e.target.value)}
                placeholder={t("settings.providers.enterApiKey")}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {(p.id === "argos" || p.id === "ollama") && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.baseUrl")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.base_url || (p.id === "argos" ? "http://localhost:5000" : "http://localhost:11434")}
                onBlur={(e) => saveKey(p.id, "base_url", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {p.id === "ollama" && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.model")}</label>
              <input
                defaultValue={config?.providers?.[p.id]?.model || "llama3.2"}
                onBlur={(e) => saveKey(p.id, "model", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              />
            </div>
          )}

          {p.id === "grok" && (
            <label className="block mb-3 text-body text-text-muted">
              {t("settings.providers.model")}
              <input key={config?.providers?.[p.id]?.model ?? "default"}
                defaultValue={config?.providers?.[p.id]?.model || "grok-4.6"}
                onBlur={(e) => { const value = e.target.value.trim(); if (value && value !== (config?.providers?.[p.id]?.model || "grok-4.6")) void saveKey(p.id, "model", value); }}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
            </label>
          )}
          {(p.id === "openai" || p.id === "claude") && (
            <div className="mb-3">
              <label className="text-body text-text-muted">{t("settings.providers.model")}</label>
              <select
                defaultValue={config?.providers?.[p.id]?.model || ""}
                onChange={(e) => saveKey(p.id, "model", e.target.value)}
                className={`mt-1 ${SETTINGS_INPUT_CLASS}`}
              >
                {p.id === "openai" && <>
                  <option value="gpt-4o-mini">gpt-4o-mini</option>
                  <option value="gpt-4o">gpt-4o</option>
                  <option value="gpt-4-turbo">gpt-4-turbo</option>
                </>}
                {p.id === "claude" && <>
                  <option value="claude-haiku-4-5-20251001">Haiku</option>
                  <option value="claude-sonnet-4-6">Sonnet</option>
                  <option value="claude-opus-4-6">Opus</option>
                </>}
              </select>
            </div>
          )}

          <div className="flex flex-wrap items-center gap-3">
            <button onClick={() => handleTest(p.id)}
              className="px-3 py-2 border border-border bg-surface-muted hover:bg-surface text-text rounded-md text-body font-medium">
              {testing[p.id] ? <Loader size={14} className="animate-spin" /> : t("settings.providers.testConnection")}
            </button>
            {results[p.id] && (
              <span className={clsx("flex items-center gap-1 text-body break-words", results[p.id].ok ? "text-success" : "text-danger")}>
                {results[p.id].ok ? <CheckCircle size={14} /> : <XCircle size={14} />}
                {results[p.id].ok ? t("settings.providers.connected") : results[p.id].message.slice(0, 60)}
              </span>
            )}
          </div>
        </div>
        );
      })}
    </div>
  );
}
