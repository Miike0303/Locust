import type { TranslationEstimateState } from "../lib/useTranslationEstimate";
import { useT } from "../lib/i18n";
import { formatUsdCost } from "../lib/translationCost";

export default function TranslationEstimate({ estimate, queue = false }: {
  estimate: TranslationEstimateState; queue?: boolean;
}) {
  const t = useT();
  const data = estimate.data;
  return <section aria-label={t("estimate.title")} className="rounded border border-border p-3 space-y-2 text-caption">
    <div className="flex items-center justify-between gap-2">
      <h3 className="font-medium">{t("estimate.title")}</h3>
      <button type="button" onClick={estimate.refresh} disabled={estimate.disabled}
        className="rounded px-2 py-1 border border-border hover:bg-surface-muted disabled:opacity-50 focus:outline-none focus:ring-2 focus:ring-accent-fg">
        {t(estimate.status === "notRequested" || estimate.status === "noProject" ? "estimate.request" : "estimate.refresh")}
      </button>
    </div>
    <p className="text-text-muted">{t("estimate.scope")}</p>
    {queue && <p className="text-text-muted">{t("estimate.queueScope")}</p>}
    <div role="status" aria-live="polite" aria-busy={estimate.pending}>
      {estimate.status !== "ready" ? t(`estimate.${estimate.status}`) : data && <>
        <p className="font-medium">{data.cost_known && data.estimated_cost_usd !== null
          ? `$${formatUsdCost(data.estimated_cost_usd)}` : t("estimate.unknownCost")}</p>
        <dl className="grid grid-cols-2 gap-x-2">
          <dt>{t("estimate.model")}</dt><dd className="break-all">{data.effective_model ?? t("estimate.unspecified")}</dd>
          <dt>{t("estimate.window")}</dt><dd>{data.context_window ?? t("estimate.unspecified")}</dd>
          <dt>{t("estimate.allowance")}</dt><dd>{data.approximate_input_allowance ?? t("estimate.unspecified")}</dd>
          <dt>{t("estimate.cap")}</dt><dd>{data.max_batch_tokens ?? t("estimate.unspecified")}</dd>
        </dl>
        <p>{t("estimate.rows", { count: data.entry_count, bytes: data.source_bytes, batch: data.batch_size })}</p>
      </>}
    </div>
    <p className="text-text-muted">{t("estimate.disclaimer")}</p>
  </section>;
}
