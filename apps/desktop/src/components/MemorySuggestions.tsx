import { useQuery } from "@tanstack/react-query";
import { getConfig, getMemorySuggestions, type StringEntry } from "../lib/api";
import { useLocale, useT } from "../lib/i18n";
import { memoryLanguagePair, memoryOriginLabel, memoryScorePercent } from "../lib/memorySuggestions";
import { readLastUsedTranslationPrefs } from "../lib/translationDefaults";
import { draftProjectKey } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import DiffView from "./DiffView";

interface MemorySuggestionsProps {
  projectKey: string;
  entry: StringEntry;
  sampleSources?: readonly string[];
  busy: boolean;
  onUse: (translation: string) => void;
}

export default function MemorySuggestions({ projectKey, entry, sampleSources, busy, onUse }: MemorySuggestionsProps) {
  const t = useT();
  const { locale } = useLocale();
  const project = useProjectStore(s => s.project);
  const active = !!project && draftProjectKey(project) === projectKey && !!entry.id;
  const config = useQuery({ queryKey: ["config"], queryFn: getConfig, enabled: active });
  // DetailPanel has no stored language pair. Reuse the translation form's
  // project sample, last-used languages and Settings defaults once loaded.
  const langPair = config.isFetched || config.isError
    ? memoryLanguagePair(config.data, readLastUsedTranslationPrefs(), sampleSources ?? [entry.source], locale) : null;
  const suggestions = useQuery({
    queryKey: ["memory-suggestions", projectKey, entry.id, langPair, entry.source, entry.translation],
    queryFn: ({ signal }) => getMemorySuggestions(entry.id, langPair!, signal),
    enabled: active && langPair !== null,
    staleTime: 0,
  });
  if (!active || !langPair) return null;
  if (suggestions.isLoading) return <p role="status" className="text-caption text-text-muted">{t("detail.memory.loading")}</p>;
  if (!suggestions.data?.suggestions.length) return null;

  return <section aria-label={t("detail.memory.title")} className="space-y-2 text-caption text-text">
    <h3 className="font-medium text-text-muted">{t("detail.memory.title")}</h3>
    {suggestions.data.suggestions.map(suggestion => <article key={JSON.stringify([suggestion.source, suggestion.translation])} className="space-y-1.5">
      <div className="flex items-center gap-2">
        <span className="rounded-full bg-accent-muted px-2 py-0.5 font-semibold text-accent-fg">{memoryScorePercent(suggestion.score)}</span>
        <span className="text-text-muted">{memoryOriginLabel(suggestion.origin, t)}</span>
      </div>
      <DiffView entryId={entry.id} originalText={entry.source} translatedText={suggestion.source} compact
        originalLabel={t("detail.memory.currentSource")} translatedLabel={t("detail.memory.matchedSource")} />
      <p className="whitespace-pre-wrap break-words">{suggestion.translation}</p>
      <button type="button" disabled={busy} className="text-accent-fg hover:underline disabled:opacity-50"
        onMouseDown={event => event.preventDefault()}
        onClick={() => onUse(suggestion.translation)}>{t("detail.memory.use")}</button>
    </article>)}
  </section>;
}
