import type { MemorySuggestion } from "./api";
import type { TranslateFn } from "./i18n";
import {
  detectSourceLanguage, resolveTranslationDefaults,
  type LastUsedTranslationPrefs, type TranslationDefaultsConfig,
} from "./translationDefaults";

export function memoryScorePercent(score: number): string {
  return `${Math.round(Math.min(1, Math.max(0, Number.isFinite(score) ? score : 0)) * 100)}%`;
}

export function memoryOriginLabel(origin: MemorySuggestion["origin"], t: TranslateFn): string {
  return t(origin === "project" ? "detail.memory.project" : "detail.memory.global");
}

/** Match the translation form's defaults, including the engine's `auto-target` memory keys. */
export function memoryLanguagePair(config: TranslationDefaultsConfig | undefined, prefs: LastUsedTranslationPrefs, samples: readonly string[], locale: string): string | null {
  const { sourceLang, targetLang } = resolveTranslationDefaults(config, prefs, {
    detectedSource: detectSourceLanguage(samples), uiLocale: locale,
  });
  return sourceLang && targetLang && targetLang !== "auto" && sourceLang !== targetLang
    ? `${sourceLang}-${targetLang}` : null;
}
