import type { ConfigUpdate, ModelPolicy, ModelPolicyPatch } from "./api";

export interface PolicyDraft {
  context: string;
  inputPrice: string;
  outputPrice: string;
  contextChanged: boolean;
}
export function hasUnsafeContext(policy?: ModelPolicy): boolean {
  return policy?.context_window != null && !Number.isSafeInteger(policy.context_window);
}
export function policyDraft(policy?: ModelPolicy): PolicyDraft {
  return {
    context: policy?.context_window != null && !hasUnsafeContext(policy) ? String(policy.context_window) : "",
    inputPrice: policy?.input_usd_per_million == null ? "" : String(policy.input_usd_per_million),
    outputPrice: policy?.output_usd_per_million == null ? "" : String(policy.output_usd_per_million),
    contextChanged: false,
  };
}
/** Only own keys are identities, including valid JSON keys such as __proto__. */
export function ownValue<T>(map: Record<string, T> | undefined, key: string): T | undefined {
  return map && Object.prototype.hasOwnProperty.call(map, key) ? map[key] : undefined;
}
export function policyPatch(provider: string, model: string, baseline: ModelPolicy | undefined, draft: PolicyDraft): {
  patch?: ConfigUpdate; error?: "identity" | "context" | "prices";
} {
  if (!provider.trim() || !model.trim()) return { error: "identity" };
  const initial = policyDraft(baseline);
  const fields: ModelPolicyPatch = {};
  // An untouched blank represents either absence or an unknown exact u64, not a reset.
  if (draft.contextChanged && (draft.context !== initial.context || !initial.context)) {
    if (draft.context === "") fields.context_window = null;
    else {
      const value = Number(draft.context);
      if (!/^[0-9]+$/.test(draft.context) || !Number.isSafeInteger(value) || value <= 0) return { error: "context" };
      fields.context_window = value;
    }
  }
  if (draft.inputPrice !== initial.inputPrice || draft.outputPrice !== initial.outputPrice) {
    const input = draft.inputPrice.trim(), output = draft.outputPrice.trim();
    if (!input && !output) {
      fields.input_usd_per_million = null;
      fields.output_usd_per_million = null;
    } else {
      const a = Number(input), b = Number(output);
      if (!input || !output || !Number.isFinite(a) || !Number.isFinite(b) || a < 0 || b < 0) return { error: "prices" };
      if (draft.inputPrice !== initial.inputPrice) fields.input_usd_per_million = a;
      if (draft.outputPrice !== initial.outputPrice) fields.output_usd_per_million = b;
    }
  }
  return Object.keys(fields).length ? { patch: { model_overrides: { [provider]: { [model]: fields } } } } : {};
}
