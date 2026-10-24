import { draftProjectKey, projectScopedQueryFn, useProjectScopedQueryEnabled } from "../../lib/projectScope";
import { useProjectStore } from "../../stores/projectStore";
import { useQuery } from "@tanstack/react-query";
import { Loader } from "lucide-react";
import { useMemo } from "react";
import type { TranslationRun } from "../../lib/api";
import { getTranslationRuns } from "../../lib/api";
import { useT, type TranslateFn } from "../../lib/i18n";
import { settingsQueryState } from "../../lib/settingsQueryState";
import { formatObservedCost } from "../../lib/translationCost";

function formatRunDate(iso: string): string {
  if (!iso) return "—";
  // Prefer local short form; fall back to first 16 chars of ISO.
  const d = new Date(iso);
  if (!Number.isNaN(d.getTime())) {
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "short",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
  return iso.length >= 16 ? iso.slice(0, 16) : iso;
}

function formatDuration(secs: number, t: TranslateFn): string {
  const s = Math.max(0, Math.floor(secs));
  if (s >= 3600) {
    return t("settings.history.durationHoursMinutes", {
      hours: Math.floor(s / 3600),
      minutes: Math.floor((s % 3600) / 60),
    });
  }
  if (s >= 60) {
    return t("settings.history.durationMinutesSeconds", {
      minutes: Math.floor(s / 60),
      seconds: s % 60,
    });
  }
  return t("settings.history.durationSeconds", { seconds: s });
}

export default function HistorySection() {
  const t = useT();
  const project = useProjectStore(state => state.project);
  const projectKey = project ? draftProjectKey(project) : "";
  const scopedEnabled = useProjectScopedQueryEnabled(projectKey);
  const { data: runs, isLoading, isError, error, refetch } = useQuery({
    queryKey: ["translation-runs", projectKey],
    enabled: scopedEnabled,
    queryFn: projectScopedQueryFn(projectKey, () => getTranslationRuns()),
  });
  const queryState = isError ? settingsQueryState(error) : null;

  const totals = useMemo(() => {
    const list = runs ?? [];
    return list.reduce(
      (acc, r) => {
        acc.strings += r.strings_translated;
        acc.tokens += r.tokens_used;
        acc.input += r.input_tokens;
        acc.output += r.output_tokens;
        acc.cost += r.cost_usd;
        acc.complete &&= r.cost_is_complete === true;
        acc.secs += r.duration_secs;
        return acc;
      },
      { strings: 0, tokens: 0, input: 0, output: 0, cost: 0, secs: 0, complete: true }
    );
  }, [runs]);

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-page font-bold">{t("settings.history.title")}</h2>
          <p className="text-body text-text-muted mt-1">
            {t("settings.history.description")}
          </p>
        </div>
        <button
          type="button"
          onClick={() => refetch()}
          className="text-body text-accent-fg hover:underline"
        >
          {t("common.refresh")}
        </button>
      </div>

      {isLoading && (
        <div className="flex items-center gap-2 text-body text-text-muted">
          <Loader size={16} className="animate-spin" /> {t("settings.history.loading")}
        </div>
      )}

      {queryState === "no_project" && (
        <div role="status" className="text-body text-text-muted">
          {t("settings.history.noProject")}
        </div>
      )}

      {queryState === "failed" && (
        <div className="text-body text-danger">
          {t("settings.history.loadFailed", {
            error: (error as Error)?.message ?? t("settings.history.unknownError"),
          })}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) === 0 && (
        <div className="border border-dashed border-border bg-surface rounded-xl p-8 text-center text-body text-text-muted">
          {t("settings.history.empty")}
        </div>
      )}

      {!isLoading && !isError && (runs?.length ?? 0) > 0 && (
        <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[56rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-caption font-semibold text-text-muted uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.history.col.date")}</th>
                <th className="px-3 py-2">{t("settings.history.col.provider")}</th>
                <th className="px-3 py-2">{t("settings.history.col.langs")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.strings")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.tokens")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.in")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.out")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.cost")}</th>
                <th className="px-3 py-2 text-right">{t("settings.history.col.duration")}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {runs!.map((r: TranslationRun) => (
                <tr key={r.id} className="hover:bg-surface-muted">
                  <td className="px-3 py-2 whitespace-nowrap text-text">
                    {formatRunDate(r.started_at)}
                  </td>
                  <td className="px-3 py-2 font-mono text-caption" title={r.provider}>
                    {r.provider}
                  </td>
                  <td className="px-3 py-2 whitespace-nowrap">
                    {r.source_lang}→{r.target_lang}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.strings_translated}</td>
                  <td className="px-3 py-2 text-right tabular-nums">{r.tokens_used}</td>
                  <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                    {r.input_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                    {r.output_tokens}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums">
                    {formatObservedCost(r.cost_usd, r.cost_is_complete, t)}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums whitespace-nowrap">
                    {formatDuration(r.duration_secs, t)}
                  </td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr className="bg-surface-muted font-semibold text-text border-t border-border">
                <td className="px-3 py-2" colSpan={3}>
                  {t("settings.history.totalRuns", { count: runs!.length })}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.strings}</td>
                <td className="px-3 py-2 text-right tabular-nums">{totals.tokens}</td>
                <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                  {totals.input}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-text-muted">
                  {totals.output}
                </td>
                <td className="px-3 py-2 text-right tabular-nums">
                  {formatObservedCost(totals.cost, totals.complete, t)}
                </td>
                <td className="px-3 py-2 text-right tabular-nums whitespace-nowrap">
                  {formatDuration(totals.secs, t)}
                </td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </div>
  );
}
