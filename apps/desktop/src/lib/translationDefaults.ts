/**
 * Shared translation-run defaults for TranslationModal and QueuePanel.
 *
 * Priority per field: last-used values (localStorage) > Settings config
 * defaults (default_provider, default_source_lang, …) > sane fallbacks.
 *
 * The resolve/coerce functions are pure so they run under node for
 * `npm run test:unit`; localStorage access is isolated in the
 * read/save helpers below (browser only).
 */

/** Subset of AppConfig (src/lib/api.ts) this module needs — kept structural so tests don't import api.ts. */
export interface TranslationDefaultsConfig {
  default_provider?: string | null;
  default_source_lang?: string;
  default_target_lang?: string;
  default_batch_size?: number;
  default_cost_limit?: number | null;
}

/** Last-used values persisted in localStorage (all optional; legacy entries only carry source/target). */
export interface LastUsedTranslationPrefs {
  provider?: string;
  source?: string;
  target?: string;
  batchSize?: number;
  /** Raw input value; "" means an explicit "no limit". */
  costLimit?: string;
}

export interface TranslationDefaults {
  /** May be "" when neither last-used nor config name one — coerce against the provider list. */
  providerId: string;
  sourceLang: string;
  targetLang: string;
  batchSize: number;
  /** Raw input value; "" = no limit. */
  costLimit: string;
}

function nonEmpty(value: unknown): string | null {
  return typeof value === "string" && value !== "" ? value : null;
}

function validBatch(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 1) return null;
  return Math.floor(value);
}

export function resolveTranslationDefaults(
  config?: TranslationDefaultsConfig | null,
  lastUsed?: LastUsedTranslationPrefs | null
): TranslationDefaults {
  const costLimit =
    typeof lastUsed?.costLimit === "string"
      ? lastUsed.costLimit
      : typeof config?.default_cost_limit === "number" && Number.isFinite(config.default_cost_limit)
        ? String(config.default_cost_limit)
        : "";

  return {
    providerId: nonEmpty(lastUsed?.provider) ?? nonEmpty(config?.default_provider) ?? "",
    sourceLang: nonEmpty(lastUsed?.source) ?? nonEmpty(config?.default_source_lang) ?? "auto",
    targetLang: nonEmpty(lastUsed?.target) ?? nonEmpty(config?.default_target_lang) ?? "es",
    batchSize: validBatch(lastUsed?.batchSize) ?? validBatch(config?.default_batch_size) ?? 40,
    costLimit,
  };
}

/** Keep the id if the fetched provider list contains it; otherwise fall back to the first available. */
export function coerceProviderId(
  id: string,
  providers: readonly { id: string }[] | null | undefined
): string {
  if (!providers || providers.length === 0) return id;
  return providers.some((p) => p.id === id) ? id : providers[0].id;
}

// ─── localStorage persistence (browser only) ───────────────────────────────

/** Historical key — legacy entries hold `{ source, target }` and still parse. */
export const TRANSLATION_PREFS_KEY = "locust.translation.langs";

export function readLastUsedTranslationPrefs(): LastUsedTranslationPrefs {
  try {
    const v = JSON.parse(localStorage.getItem(TRANSLATION_PREFS_KEY) || "{}");
    return v && typeof v === "object" ? (v as LastUsedTranslationPrefs) : {};
  } catch {
    return {};
  }
}

export function saveLastUsedTranslationPrefs(prefs: LastUsedTranslationPrefs): void {
  try {
    localStorage.setItem(
      TRANSLATION_PREFS_KEY,
      JSON.stringify({ ...readLastUsedTranslationPrefs(), ...prefs })
    );
  } catch {
    /* best effort */
  }
}
