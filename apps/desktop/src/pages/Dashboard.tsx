import { useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle } from "lucide-react";
import { getCurrentProject, getStats, getFileStats, getTranslationRuns, getInjectionStatus, type StringStatus } from "../lib/api";
import { ApiError } from "../lib/apiError";
import { useT, getLocale } from "../lib/i18n";
import { buildProjectDashboard, formatDashboardCost } from "../lib/projectDashboard";
import { formatObservedCost } from "../lib/translationCost";
import { readSkipReviewPreference } from "../lib/workflowGuide";
import { draftProjectKey } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import EmptyState from "../components/EmptyState";
import InjectionRecoveryModal from "../components/InjectionRecoveryModal";

const CARD = "rounded-lg border border-border bg-surface p-4 space-y-3";
const ACTION = "inline-flex items-center justify-center rounded-md bg-accent-muted px-4 py-2 text-body font-medium text-accent-fg hover:bg-accent-muted/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg";
const SEGMENT_CLASS: Record<StringStatus, string> = {
  pending: "bg-border",
  translated: "bg-accent-muted",
  reviewed: "bg-warning",
  approved: "bg-success",
  error: "bg-danger",
};

export default function Dashboard() {
  const project = useProjectStore(s => s.project);
  const projectKey = project ? draftProjectKey(project) : "";
  // A recovery session belongs to the project that opened it.
  return <ProjectDashboard key={projectKey} projectKey={projectKey} gamePath={project?.path} />;
}

function ProjectDashboard({ projectKey, gamePath }: { projectKey: string; gamePath?: string }) {
  const t = useT();
  const [recoveryPath, setRecoveryPath] = useState<string | null>(null);
  const [skipReview] = useState(readSkipReviewPreference);
  const enabled = !!gamePath;
  const projectQuery = useQuery({ queryKey: ["current-project", projectKey], queryFn: getCurrentProject, enabled, staleTime: 10_000 });
  const hasProject = enabled && projectQuery.data !== null;
  const statsQuery = useQuery({ queryKey: ["stats", projectKey], queryFn: getStats, enabled: hasProject, staleTime: 10_000 });
  const filesQuery = useQuery({ queryKey: ["file-stats", projectKey], queryFn: getFileStats, enabled: hasProject, staleTime: 10_000 });
  const runsQuery = useQuery({ queryKey: ["translation-runs", projectKey], queryFn: getTranslationRuns, enabled: hasProject, staleTime: 10_000 });
  const injectionQuery = useQuery({ queryKey: ["injection-status", projectKey], queryFn: () => getInjectionStatus(gamePath!), enabled: hasProject, staleTime: 10_000 });
  const queries = [projectQuery, statsQuery, filesQuery, runsQuery, injectionQuery];
  const failed = queries.find(query => query.isError);
  const noProject = !hasProject || queries.some(query => query.error instanceof ApiError && query.error.key === "api.error.noProjectOpen");

  const heading = <h1 className="text-page font-bold">{t("dashboard.title")}</h1>;
  if (noProject) return (
    <div className="flex h-full flex-col bg-surface text-text">
      <header className="border-b border-border px-6 py-4">{heading}</header>
      <div className="flex flex-1 flex-col items-center gap-3 p-6">
        <EmptyState title={t("dashboard.noProject.title")} description={t("dashboard.noProject.description")} />
        <Link to="/" className="rounded text-body text-accent-fg underline focus-visible:ring-2 focus-visible:ring-accent-fg">{t("dashboard.home")}</Link>
      </div>
    </div>
  );
  if (failed) return (
    <div className="space-y-4 p-6 text-text">
      {heading}
      <div role="alert" className={`${CARD} text-center`}>
        <p className="text-body font-medium text-danger">{t("dashboard.loadError")}</p>
        <p className="text-body text-text-muted">{failed.error instanceof Error ? failed.error.message : t("common.tryAgain")}</p>
        <button type="button" className={ACTION} onClick={() => { for (const query of queries) void query.refetch(); }}>{t("common.retry")}</button>
      </div>
    </div>
  );
  const project = projectQuery.data;
  const stats = statsQuery.data;
  if (queries.some(query => query.isPending) || !project || !stats) return (
    <div className="space-y-4 p-6 text-text">{heading}<p role="status" className="text-body text-text-muted">{t("common.loading")}</p></div>
  );

  const model = buildProjectDashboard({ project, stats, files: filesQuery.data ?? [], skipReview });
  const lastRun = runsQuery.data?.[0];
  const pendingInjection = injectionQuery.data?.pending;
  const attention = model.warnings.length > 0 || model.errorFiles.length > 0 || !!pendingInjection;
  const progressLabel = model.segments.map(segment => t(`dashboard.status.${segment.status}`) + ": " + segment.count).join("; ");
  const runDate = lastRun ? new Date(lastRun.started_at) : null;
  const closeRecovery = () => {
    setRecoveryPath(null);
    void injectionQuery.refetch();
  };

  return (
    <div className="space-y-6 p-6 text-body text-text">
      <header className="space-y-2">
        {heading}
        <div className="flex flex-wrap items-center gap-2">
          <p className="font-medium break-all">{model.projectName}</p>
          <span className="rounded bg-accent-muted px-2 py-1 text-caption text-accent-fg">{project.format_id}</span>
        </div>
        <p className="break-all text-caption text-text-muted">{t("dashboard.database", { path: project.database_path ?? t("dashboard.unknown") })}</p>
      </header>

      <div className="grid gap-4 md:grid-cols-3">
        <section className={CARD} aria-labelledby="dashboard-progress">
          <h2 id="dashboard-progress" className="font-semibold">{t("dashboard.progress")}</h2>
          <p className="text-page font-bold tabular-nums">{model.percent}%</p>
          <div role="img" aria-label={progressLabel} className="flex h-3 overflow-hidden rounded-full bg-surface-muted">
            {model.segments.map(segment => <span key={segment.status} aria-hidden="true" title={t(`dashboard.status.${segment.status}`)} className={SEGMENT_CLASS[segment.status]} style={{ width: `${segment.width}%` }} />)}
          </div>
          <ul className="flex flex-wrap gap-x-3 gap-y-1 text-caption text-text-muted">
            {model.segments.map(segment => <li key={segment.status} className="inline-flex items-center gap-1.5"><span aria-hidden="true" className={`h-2 w-2 rounded-sm ${SEGMENT_CLASS[segment.status]}`} />{t(`dashboard.status.${segment.status}`)}: {segment.count}</li>)}
          </ul>
        </section>
        <section className={CARD} aria-labelledby="dashboard-cost">
          <h2 id="dashboard-cost" className="font-semibold">{t("dashboard.cost")}</h2>
          <p className="text-page font-bold tabular-nums">{formatDashboardCost(stats.total_cost_usd)}</p>
          <p className="text-caption text-text-muted">{t("dashboard.lastRun")}</p>
          {lastRun ? <div className="space-y-1">
            <p className="break-words">{lastRun.provider}</p>
            <p className="text-caption text-text-muted">{runDate && Number.isFinite(runDate.getTime()) ? <time dateTime={lastRun.started_at}>{runDate.toLocaleString(getLocale())}</time> : t("dashboard.unknown")}</p>
            <p className="text-caption">{formatObservedCost(lastRun.cost_usd, lastRun.cost_is_complete, t)}</p>
          </div> : <p className="text-caption text-text-muted">{t("dashboard.noRuns")}</p>}
        </section>
        <section className={CARD} aria-labelledby="dashboard-next">
          <h2 id="dashboard-next" className="font-semibold">{t("dashboard.nextStep")}</h2>
          {model.nextStep ? <>
            <p>{t(`workflow.${model.nextStep.step}`)}</p>
            <Link to={model.nextStep.path} className={ACTION}>{t(`workflow.${model.nextStep.step}Action`)}</Link>
          </> : <p className="text-text-muted">{t("dashboard.nothingPending")}</p>}
        </section>
      </div>

      {attention && <section className="rounded-lg border border-warning bg-warning-muted p-4 space-y-3" aria-labelledby="dashboard-attention">
        <h2 id="dashboard-attention" className="flex items-center gap-2 font-semibold text-warning"><AlertTriangle size={18} aria-hidden="true" />{t("dashboard.attention")}</h2>
        <ul className="space-y-2 break-words">
          {model.warnings.map((warning, index) => <li key={`warning-${index}`}>{warning}</li>)}
          {model.moreWarnings > 0 && <li className="text-caption text-text-muted">{t("dashboard.moreWarnings", { count: model.moreWarnings })}</li>}
          {model.errorFiles.map(file => <li key={file.file_path}>{t("dashboard.fileErrors", { path: file.relativePath, count: file.error })}</li>)}
          {pendingInjection && <li className="space-y-2">
            <p>{t("dashboard.interruptedInjection")}</p>
            <button type="button" className="rounded-md border border-border bg-surface px-3 py-2 text-body text-text hover:bg-surface-muted focus-visible:ring-2 focus-visible:ring-accent-fg" onClick={() => setRecoveryPath(gamePath!)}>{t("recovery.action")}</button>
          </li>}
        </ul>
      </section>}

      <section className="overflow-hidden rounded-lg border border-border bg-surface" aria-labelledby="dashboard-files">
        <header className="border-b border-border px-4 py-3"><h2 id="dashboard-files" className="font-semibold">{t("dashboard.files")}</h2><p className="text-caption text-text-muted">{t("dashboard.filesHint")}</p></header>
        <div className="overflow-x-auto">
          <table className="w-full text-body">
            <thead className="bg-surface-muted text-text-muted"><tr>
              <th scope="col" className="px-4 py-2 text-left font-medium">{t("dashboard.file")}</th>
              <th scope="col" className="px-4 py-2 text-right font-medium whitespace-nowrap">{t("dashboard.total")}</th>
              <th scope="col" className="px-4 py-2 text-right font-medium whitespace-nowrap">{t("dashboard.status.pending")}</th>
              <th scope="col" className="px-4 py-2 text-right font-medium whitespace-nowrap">{t("dashboard.donePercent")}</th>
            </tr></thead>
            <tbody>{model.topFiles.length ? model.topFiles.map(file => <tr key={file.file_path} className="border-t border-border">
              <th scope="row" className="max-w-0 w-full px-4 py-2 text-left font-normal"><span className="block truncate" title={file.relativePath}>{file.relativePath}</span></th>
              <td className="px-4 py-2 text-right tabular-nums">{file.total}</td>
              <td className="px-4 py-2 text-right tabular-nums">{file.pending}</td>
              <td className="px-4 py-2 text-right tabular-nums">{file.percent}%</td>
            </tr>) : <tr><td colSpan={4} className="px-4 py-6 text-center text-text-muted">{t("dashboard.noFiles")}</td></tr>}</tbody>
          </table>
        </div>
      </section>
      {recoveryPath !== null && <InjectionRecoveryModal defaultGamePath={recoveryPath} onClose={closeRecovery} />}
    </div>
  );
}
