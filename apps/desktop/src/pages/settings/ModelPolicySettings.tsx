import { useRef, useState } from "react";
import type { FormEvent } from "react";
import { SETTINGS_INPUT_CLASS } from "../../components/BufferedSetting";
import type { AppConfig, ConfigUpdate, ModelPolicy } from "../../lib/api";
import { useT } from "../../lib/i18n";
import { hasUnsafeContext, ownValue, policyDraft, policyPatch } from "../../lib/modelPolicySettings";
import type { PolicyDraft } from "../../lib/modelPolicySettings";

export interface ModelPolicySettingsProps {
  providers?: { id: string }[];
  config?: AppConfig;
  onSave: (patch: ConfigUpdate) => Promise<AppConfig | null | false>;
}
interface Edit {
  provider: string;
  model: string;
  baseline: ModelPolicy;
  draft: PolicyDraft;
}
const BUTTON = "rounded-md border border-border bg-surface-muted px-3 py-2 text-body text-text hover:bg-surface focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-50";

export default function ModelPolicySettings({ providers, config, onSave }: ModelPolicySettingsProps) {
  const t = useT();
  const [provider, setProvider] = useState("");
  const [model, setModel] = useState("");
  const [edit, setEdit] = useState<Edit | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<"identity" | "context" | "prices" | "save" | null>(null);
  const pending = useRef(false);
  const restoreFocus = useRef(false);
  const providerIds = [...new Set([...(providers ?? []).map(p => p.id), ...Object.keys(config?.providers ?? {}), ...Object.keys(config?.model_overrides ?? {})])];
  const policies = ownValue(config?.model_overrides, provider);
  const configuredModel = ownValue(config?.providers, provider)?.model;
  const models = [...new Set([...Object.keys(policies ?? {}), ...(typeof configuredModel === "string" ? [configuredModel] : [])])];
  const result = edit ? policyPatch(edit.provider, edit.model, edit.baseline, edit.draft) : {};

  const begin = () => {
    if (pending.current || edit || !config) return;
    if (!provider.trim() || !model.trim()) { setError("identity"); return; }
    const policy = ownValue(policies, model);
    // Snapshot just the editable fields; query refreshes cannot move this edit's baseline.
    const baseline = { context_window: policy?.context_window,
      input_usd_per_million: policy?.input_usd_per_million, output_usd_per_million: policy?.output_usd_per_million };
    setEdit({ provider, model, baseline, draft: policyDraft(baseline) });
    setError(null);
  };
  const change = (field: "context" | "inputPrice" | "outputPrice", value: string) => {
    if (pending.current) return;
    setEdit(current => current && ({ ...current, draft: { ...current.draft, [field]: value,
      contextChanged: current.draft.contextChanged || field === "context" } }));
    setError(null);
  };
  const close = () => { restoreFocus.current = true; setEdit(null); setError(null); };
  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (pending.current || !edit) return;
    if (result.error) { setError(result.error); return; }
    if (!result.patch) return;
    pending.current = true; setSaving(true); setError(null);
    try {
      const saved = await onSave(result.patch);
      if (saved) close();
      else setError("save"); // Shared Settings save returns null on failure.
    } catch {
      setError("save");
    } finally { pending.current = false; setSaving(false); }
  };

  return <section aria-labelledby="model-policy-title" className="rounded-xl border border-border bg-surface p-5 space-y-4">
    <h3 id="model-policy-title" className="text-section font-semibold">{t("settings.modelPolicy.title")}</h3>
    <p id="model-policy-hint" className="text-body text-text-muted">{t("settings.modelPolicy.hint")}</p>
    <div className="grid gap-3 sm:grid-cols-2">
      <label htmlFor="model-policy-provider" className="text-body font-medium">{t("settings.modelPolicy.provider")}
        <select id="model-policy-provider" value={provider} disabled={saving || !!edit || !config}
          onChange={event => { setProvider(event.target.value); setModel(""); setError(null); }} className={`mt-1 ${SETTINGS_INPUT_CLASS}`}>
          <option value="">{t("settings.modelPolicy.chooseProvider")}</option>
          {providerIds.map(id => <option key={id} value={id}>{id}</option>)}
        </select>
      </label>
      <label htmlFor="model-policy-model" className="text-body font-medium">{t("settings.modelPolicy.model")}
        <input id="model-policy-model" list="model-policy-models" value={model} disabled={saving || !!edit || !config}
          aria-describedby="model-policy-hint" onChange={event => { setModel(event.target.value); setError(null); }} className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
        <datalist id="model-policy-models">{models.map(key => <option key={key} value={key} />)}</datalist>
      </label>
    </div>
    <button id="model-policy-edit" ref={node => {
      if (node && !edit && !saving && restoreFocus.current) { restoreFocus.current = false; node.focus(); }
    }} type="button" disabled={saving || !!edit || !config} onClick={begin} className={BUTTON}>{t("settings.modelPolicy.edit")}</button>
    {edit && <form onSubmit={save} className="space-y-3">
      <p className="text-caption text-text-muted">{t("settings.modelPolicy.locked")}</p>
      {hasUnsafeContext(edit.baseline) && <p role="status" id="model-policy-unsafe" className="text-body text-warning">{t("settings.modelPolicy.unsafe")}</p>}
      <label htmlFor="model-policy-context" className="block text-body font-medium">{t("settings.modelPolicy.context")}
        <input id="model-policy-context" autoFocus inputMode="numeric" value={edit.draft.context} disabled={saving}
          aria-describedby={`model-policy-contextHint${hasUnsafeContext(edit.baseline) ? " model-policy-unsafe" : ""}`}
          aria-invalid={error === "context" || undefined} onChange={event => change("context", event.target.value)} className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
      </label>
      <p id="model-policy-contextHint" className="text-caption text-text-muted">{t("settings.modelPolicy.contextHint")}</p>
      <button id="model-policy-resetContext" type="button" disabled={saving} onClick={() => change("context", "")} className={BUTTON}>{t("settings.modelPolicy.resetContext")}</button>
      <div className="grid gap-3 sm:grid-cols-2">
        {(["inputPrice", "outputPrice"] as const).map(field => <label key={field} htmlFor={`model-policy-${field}`} className="text-body font-medium">{t(`settings.modelPolicy.${field}`)}
          <input id={`model-policy-${field}`} inputMode="decimal" value={edit.draft[field]} disabled={saving}
            aria-describedby="model-policy-priceHint" aria-invalid={error === "prices" || undefined}
            onChange={event => change(field, event.target.value)} className={`mt-1 ${SETTINGS_INPUT_CLASS}`} />
        </label>)}
      </div>
      <p id="model-policy-priceHint" className="text-caption text-text-muted">{t("settings.modelPolicy.priceHint")}</p>
      <div className="flex flex-wrap gap-3">
        <button id="model-policy-save" type="submit" disabled={saving || (!result.patch && !result.error)} className={BUTTON}>{t("settings.modelPolicy.save")}</button>
        <button id="model-policy-cancel" type="button" disabled={saving} onClick={() => { if (!pending.current) close(); }} className={BUTTON}>{t("common.cancel")}</button>
      </div>
      {saving && <p role="status" className="text-body text-text-muted">{t("settings.field.saving")}</p>}
    </form>}
    {error && <p role="alert" className="text-body text-danger">{t(`settings.modelPolicy.error.${error}`)}</p>}
  </section>;
}
