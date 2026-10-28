import { projectScopedQueryFn, useProjectScopedQueryEnabled } from "../lib/projectScope";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft, ChevronRight, ShieldCheck, ShieldQuestion } from "lucide-react";
import { getValidationIssues, getValidationSummary, setValidationIssueIgnored, validate, type ProjectInfo } from "../lib/api";
import { ApiError } from "../lib/apiError";
import { useLocale, useT } from "../lib/i18n";
import { buildQaSummary, qaHasBeenChecked, qaIsAdvisoryKind, qaKindBadgeClass, qaLastRunKey, qaPagination, qaRelativePath, QA_PAGE_SIZE } from "../lib/qaDashboard";
import { validationBadgeLabel, validationKindHint, validationMessage } from "../lib/validationLabels";
import { draftProjectKey } from "../stores/draftStore";
import { useProjectStore } from "../stores/projectStore";
import { useEditorStore } from "../stores/editorStore";
import EmptyState from "../components/EmptyState";

const ACTION_BASE = "inline-flex items-center justify-center gap-1 rounded-md px-3 py-2 text-body font-medium disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg";
const ACTION = `${ACTION_BASE} bg-accent-muted text-accent-fg hover:bg-accent-muted/80`;
const PRIMARY_ACTION = `${ACTION_BASE} bg-accent text-white hover:bg-accent-hover`;

export default function Qa() {
  const project = useProjectStore(s => s.project);
  const projectKey = project ? draftProjectKey(project) : "";
  return <ProjectQa key={projectKey} project={project} projectKey={projectKey} />;
}

function ProjectQa({ project, projectKey }: { project: ProjectInfo | null; projectKey: string }) {
  const t = useT();
  const { locale } = useLocale();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const startValidationWorklist = useEditorStore(s => s.startValidationWorklist);
  const [kind, setKind] = useState<string | undefined>();
  const [includeIgnored, setIncludeIgnored] = useState(false);
  const [offset, setOffset] = useState(0);
  const scopedEnabled = useProjectScopedQueryEnabled(projectKey);
  const issuesQuery = useQuery({
    queryKey: ["validation-issues", projectKey, kind, includeIgnored, offset],
    queryFn: projectScopedQueryFn(projectKey, ({ signal }) => getValidationIssues({ kind, include_ignored: includeIgnored, offset, limit: QA_PAGE_SIZE }, signal)),
    enabled: scopedEnabled,
    refetchOnMount: "always",
  });
  const summaryQuery = useQuery({
    queryKey: ["validation-summary", projectKey],
    queryFn: projectScopedQueryFn(projectKey, ({ signal }) => getValidationSummary(signal)),
    enabled: scopedEnabled,
    refetchOnMount: "always",
  });
  // The API has no last-run metadata. Retain only successful QA runs in this
  // QueryClient session, keyed by database as well as game path. Never persist
  // an inferred clean result across app restarts.
  const lastRunQuery = useQuery<number | null>({
    queryKey: qaLastRunKey(projectKey),
    queryFn: () => null,
    initialData: null,
    enabled: false,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  const lastCheckedAt = lastRunQuery.data ?? null;
  const refresh = async () => {
    await Promise.all([
      qc.invalidateQueries({ queryKey: ["validation-issues", projectKey] }),
      qc.invalidateQueries({ queryKey: ["validation-summary", projectKey] }),
    ]);
  };
  const run = useMutation({
    mutationKey: ["validation-run", projectKey],
    mutationFn: validate,
    onSuccess: async () => {
      const completedAt = Date.now();
      setOffset(0);
      await refresh();
      qc.setQueryData(qaLastRunKey(projectKey), completedAt);
    },
  });
  const ignore = useMutation({
    mutationFn: ({ id, ignored }: { id: number; ignored: boolean }) => setValidationIssueIgnored(id, ignored),
    onSuccess: refresh,
  });
  const data = issuesQuery.data;
  const pagination = qaPagination(offset, data?.total ?? 0);
  useEffect(() => {
    if (data && !issuesQuery.isError) setOffset(qaPagination(offset, data.total).offset);
  }, [data, offset, issuesQuery.isError]);

  const queries = [issuesQuery, summaryQuery];
  const noProject = !project || queries.some(q => q.error instanceof ApiError && q.error.key === "api.error.noProjectOpen");
  const failed = queries.find(q => q.isError);
  const actionError = run.error ?? ignore.error;
  const busy = run.isPending || ignore.isPending;
  const summary = buildQaSummary(summaryQuery.data ?? {}, kind);
  const neverChecked = !qaHasBeenChecked(summaryQuery.data ?? {}, lastCheckedAt) && !data?.total;
  const checkedEmpty = lastCheckedAt !== null && summary.open === 0 && (!includeIgnored || summary.kinds.every(counts => counts.ignored === 0));
  const runValidation = () => { ignore.reset(); run.mutate(); };
  const chipClass = (active: boolean, chipKind?: string) => `rounded-full border px-3 py-1 text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg ${chipKind && qaIsAdvisoryKind(chipKind) ? `${active ? "border-warning" : "border-border"} bg-warning-muted text-warning` : active ? "border-accent-fg bg-accent-muted text-accent-fg" : "border-border bg-surface text-text-muted hover:bg-surface-muted"}`;

  return (
    <div className="flex h-full flex-col bg-surface text-text">
      <header className="space-y-4 border-b border-border px-6 py-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2"><ShieldCheck size={20} className="text-accent-fg" /><h1 className="text-page font-bold">{t("qa.title")}</h1></div>
          <button type="button" className={`${neverChecked ? PRIMARY_ACTION : ACTION} disabled:cursor-not-allowed`} disabled={noProject || busy} onClick={runValidation}>{t(run.isPending ? "qa.running" : "qa.run")}</button>
        </div>
        {!noProject && <div className="flex flex-wrap items-center gap-2" role="group" aria-label={t("qa.filterKind")}>
          <button type="button" className={chipClass(kind === undefined)} aria-pressed={kind === undefined} onClick={() => { setKind(undefined); setOffset(0); }}>{t("qa.all")} ({summary.open})</button>
          {summary.kinds.map(counts => <button key={counts.kind} type="button" className={chipClass(kind === counts.kind, counts.kind)} title={validationKindHint(counts.kind, t)} aria-pressed={kind === counts.kind} onClick={() => { setKind(counts.kind); setOffset(0); }}>{validationBadgeLabel(counts.kind, t)} ({counts.open})</button>)}
          <label className="ml-2 inline-flex items-center gap-2 text-body"><input type="checkbox" checked={includeIgnored} onChange={e => { setIncludeIgnored(e.target.checked); setOffset(0); }} />{t("qa.showIgnored")}</label>
        </div>}
      </header>
      {actionError && <div role="alert" className="px-6 py-3 text-body text-danger">{t("qa.actionError")} {actionError.message}</div>}
      <div className="flex-1 overflow-auto">
        {noProject ? <EmptyState title={t("qa.noProject.title")} description={t("qa.noProject.description")} actionLabel={t("common.goHome")} actionTo="/" />
          : failed ? <div role="alert" className="m-6 space-y-3 rounded-lg border border-border bg-surface p-4 text-center"><p className="text-danger">{t("qa.loadError")}</p><p className="text-body text-text-muted">{failed.error?.message}</p><button type="button" className={ACTION} onClick={() => { void refresh(); }}>{t("common.retry")}</button></div>
          : queries.some(q => q.isPending) ? <p role="status" className="p-6 text-body text-text-muted">{t("common.loading")}</p>
          : run.isPending ? <p role="status" className="p-6 text-body text-text-muted">{t("qa.running")}</p>
          : !data?.issues.length ? neverChecked
            ? <EmptyState icon={<ShieldQuestion size={32} aria-hidden="true" className="text-text-muted" />} title={t("qa.notChecked")} description={t("qa.notCheckedDescription")} action={<button type="button" className={PRIMARY_ACTION} disabled={busy} onClick={runValidation}>{t("qa.run")}</button>} />
            : checkedEmpty
              ? <EmptyState icon={<ShieldCheck size={32} aria-hidden="true" className="text-success" />} title={t(includeIgnored ? "qa.emptyAll" : "qa.empty")} description={t("qa.checkedAt", { time: new Date(lastCheckedAt!).toLocaleString(locale) })} />
              : <EmptyState title={t("qa.noMatchingIssues")} description={t("qa.emptyDescription")} />
          : <table className="w-full text-body">
            <thead className="sticky top-0 border-b border-border bg-surface-muted"><tr className="text-left text-caption uppercase text-text-muted">
              <th scope="col" className="px-4 py-2">{t("qa.col.kind")}</th><th scope="col" className="px-4 py-2">{t("qa.col.file")}</th><th scope="col" className="px-4 py-2">{t("qa.col.source")}</th><th scope="col" className="px-4 py-2">{t("qa.col.message")}</th><th scope="col" className="px-4 py-2">{t("qa.col.actions")}</th>
            </tr></thead>
            <tbody>{data.issues.map(issue => <tr key={issue.id} className="border-b border-border hover:bg-surface-muted">
              <td className="px-4 py-2"><span className={`whitespace-nowrap rounded px-2 py-1 text-caption ${qaKindBadgeClass(issue.kind)}`} title={validationKindHint(issue.kind, t)}>{validationBadgeLabel(issue.kind, t)}</span>{issue.ignored && <span className="ml-2 text-caption text-text-muted">{t("qa.ignored")}</span>}</td>
              <td className="max-w-48 truncate px-4 py-2" title={issue.file_path ?? undefined}>{issue.file_path ? qaRelativePath(issue.file_path, project!.path) : t("qa.unavailable")}</td>
              <td className="max-w-xs truncate px-4 py-2" title={issue.source ?? undefined}>{issue.source ?? t("qa.unavailable")}</td>
              <td className="px-4 py-2">{validationMessage(issue, t)}</td>
              <td className="px-4 py-2"><div className="flex flex-wrap gap-2">
                <button type="button" className={ACTION} disabled={issue.source === null} onClick={() => { startValidationWorklist([issue.entry_id]); navigate("/editor"); }}>{t("qa.openEditor")}</button>
                <button type="button" className={ACTION} disabled={busy || issuesQuery.isFetching} onClick={() => { run.reset(); ignore.mutate({ id: issue.id, ignored: !issue.ignored }); }}>{t(issue.ignored ? "qa.restore" : "qa.ignore")}</button>
              </div></td>
            </tr>)}</tbody>
          </table>}
      </div>
      {!noProject && !failed && data && pagination.pages > 1 && <div className="flex items-center justify-between border-t border-border bg-surface px-6 py-3">
        <span className="text-body text-text-muted">{t("qa.showing", { from: pagination.from, to: pagination.to, total: data.total })}</span>
        <div className="flex gap-2">
          <button type="button" className={ACTION} disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - QA_PAGE_SIZE))}><ChevronLeft size={14} />{t("common.prev")}</button>
          <span className="px-3 py-2 text-body text-text-muted">{pagination.page} / {pagination.pages}</span>
          <button type="button" className={ACTION} disabled={offset + QA_PAGE_SIZE >= data.total} onClick={() => setOffset(offset + QA_PAGE_SIZE)}>{t("common.next")}<ChevronRight size={14} /></button>
        </div>
      </div>}
    </div>
  );
}
