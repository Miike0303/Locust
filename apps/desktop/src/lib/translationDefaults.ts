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

import {
	resolveProviderReadiness,
	type ProviderReadinessConfig,
	type ProviderReadinessMeta,
} from "./providerReadiness";

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
  /** Parallel batches; queue and translate dialog share last-used when set. */
  maxConcurrent?: number;
  /** Raw input value; "" means an explicit "no limit". */
  costLimit?: string;
}

export interface TranslationDefaults {
  /** May be "" when neither last-used nor config name one — coerce against the provider list. */
  providerId: string;
  sourceLang: string;
  targetLang: string;
  batchSize: number;
  maxConcurrent: number;
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

/** What the open project says about its languages. */
export interface ProjectLanguageContext {
  /** From {@link detectSourceLanguage}; null when the sample is inconclusive. */
  detectedSource?: string | null;
  /** Current UI locale, used as the target when the configured one equals the source. */
  uiLocale?: string;
}

const MIN_LETTERS = 20;

/**
 * Guess a project's source language from a sample of its source strings by
 * script: kana -> ja, hangul -> ko, CJK ideographs without kana -> zh-CN,
 * Cyrillic -> ru, unaccented Latin -> en. Accented Latin (es/fr/de/...) and
 * small samples return null: the caller then keeps its other defaults.
 */
export function detectSourceLanguage(samples: readonly string[]): string | null {
  let kana = 0, hangul = 0, han = 0, cyrillic = 0, latin = 0, accented = 0, letters = 0;
  for (const text of samples) {
    for (const ch of text) {
      const c = ch.codePointAt(0)!;
      if (c >= 0x3040 && c <= 0x30ff) kana++;
      else if ((c >= 0xac00 && c <= 0xd7af) || (c >= 0x1100 && c <= 0x11ff)) hangul++;
      else if (c >= 0x4e00 && c <= 0x9fff) han++;
      else if (c >= 0x0400 && c <= 0x04ff) cyrillic++;
      else if ((c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a)) latin++;
      else if (c >= 0xc0 && c <= 0x24f && c !== 0xd7 && c !== 0xf7) { latin++; accented++; }
      else continue;
      letters++;
    }
  }
  if (letters < MIN_LETTERS) return null;
  const share = (n: number) => n / letters;
  if (kana > 0 && share(kana + han) >= 0.5) return "ja";
  if (share(hangul) >= 0.5) return "ko";
  if (share(han) >= 0.5) return "zh-CN";
  if (share(cyrillic) >= 0.5) return "ru";
  if (share(latin) >= 0.8 && accented / Math.max(latin, 1) < 0.005) return "en";
  return null;
}

function targetDifferentFrom(source: string, preferred: string, uiLocale?: string): string {
  if (preferred !== source) return preferred;
  for (const candidate of [uiLocale, "en", "es"]) {
    if (candidate && candidate !== source) return candidate;
  }
  return preferred;
}

export function resolveTranslationDefaults(
  config?: TranslationDefaultsConfig | null,
  lastUsed?: LastUsedTranslationPrefs | null,
  project?: ProjectLanguageContext | null,
): TranslationDefaults {
  const costLimit =
    typeof lastUsed?.costLimit === "string"
      ? lastUsed.costLimit
      : typeof config?.default_cost_limit === "number" && Number.isFinite(config.default_cost_limit)
        ? String(config.default_cost_limit)
        : "";

  // The open project's own text outranks a global default or another project's last run.
  const detected = nonEmpty(project?.detectedSource);
  const sourceLang =
    detected ?? nonEmpty(lastUsed?.source) ?? nonEmpty(config?.default_source_lang) ?? "auto";
  const preferredTarget =
    nonEmpty(lastUsed?.target) ?? nonEmpty(config?.default_target_lang) ?? "es";

  return {
    providerId: nonEmpty(lastUsed?.provider) ?? nonEmpty(config?.default_provider) ?? "",
    sourceLang,
    targetLang: detected
      ? targetDifferentFrom(sourceLang, preferredTarget, project?.uiLocale)
      : preferredTarget,
    batchSize: validBatch(lastUsed?.batchSize) ?? validBatch(config?.default_batch_size) ?? 40,
    // Translate dialog historically defaulted to 1; queue used 3. Prefer last-used, else 3
    // so batch runs keep the documented escape hatch for large projects.
    maxConcurrent: validBatch(lastUsed?.maxConcurrent) ?? 3,
    costLimit,
  };
}

/** Shape shared by TranslationModal and QueuePanel when calling startTranslation. */
export type TranslationStartForm = {
  providerId: string;
  fallbackIds: string[];
  sourceLang: string;
  targetLang: string;
  batchSize: number;
  maxConcurrent: number;
  /** Raw; empty string → null cost limit. */
  costLimit: string;
  gameContext: string;
};

export function buildTranslationStartParams(form: TranslationStartForm): {
  provider_id: string;
  fallback_provider_ids?: string[];
  options: {
    source_lang: string;
    target_lang: string;
    batch_size: number;
    max_concurrent: number;
    cost_limit_usd: number | null;
    game_context: string | null;
    use_glossary: boolean;
    use_memory: boolean;
    skip_approved: boolean;
  };
} {
  const rawCost = form.costLimit.trim();
  const cost_limit_usd = rawCost === "" ? null : Number(rawCost);
  if (cost_limit_usd !== null && (!Number.isFinite(cost_limit_usd) || cost_limit_usd < 0)) {
    throw new Error("cost limit must be finite and non-negative");
  }
  const max_concurrent = Math.max(1, validBatch(form.maxConcurrent) ?? 1);
  const batch_size = Math.max(1, validBatch(form.batchSize) ?? 40);
  const fallbacks = form.fallbackIds.filter(
    (id) => typeof id === "string" && id.length > 0 && id !== form.providerId,
  );
  return {
    provider_id: form.providerId,
    ...(fallbacks.length > 0 ? { fallback_provider_ids: fallbacks } : {}),
    options: {
      source_lang: form.sourceLang,
      target_lang: form.targetLang,
      batch_size,
      max_concurrent,
      cost_limit_usd,
      game_context: form.gameContext.trim() || null,
      use_glossary: true,
      use_memory: true,
      skip_approved: true,
    },
  };
}

function firstReadyProviderId(
	providers: readonly ProviderReadinessMeta[],
	config?: ProviderReadinessConfig | null,
): string {
	const ready = providers.find(
		(p) => resolveProviderReadiness(p.id, providers, config).ready,
	);
	return ready?.id ?? providers[0]?.id ?? "";
}

/** Keep the id when it is listed and ready; otherwise prefer the first ready provider. */
export function coerceProviderId(
	id: string,
	providers: readonly ProviderReadinessMeta[] | null | undefined,
	config?: ProviderReadinessConfig | null,
): string {
	if (!providers || providers.length === 0) return id;
	if (
		providers.some((p) => p.id === id) &&
		resolveProviderReadiness(id, providers, config).ready
	) {
		return id;
	}
	const fallback = firstReadyProviderId(providers, config);
	return fallback || id;
}

// ─── localStorage persistence (browser only) ───────────────────────────────

/** Historical key — legacy entries hold `{ source, target }` and still parse. */
export const TRANSLATION_PREFS_KEY = "locust.translation.langs";

/** Shared by TranslationModal and QueuePanel. */
export const FALLBACK_STORAGE_KEY = "locust.translation.fallbacks";

export function readTranslationFallbacks(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(FALLBACK_STORAGE_KEY) || "[]");
    return Array.isArray(v)
      ? v.filter((x: unknown): x is string => typeof x === "string" && x.length > 0)
      : [];
  } catch {
    return [];
  }
}

export function saveTranslationFallbacks(ids: string[]): void {
  try {
    localStorage.setItem(FALLBACK_STORAGE_KEY, JSON.stringify(ids));
  } catch {
    /* best effort */
  }
}

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
