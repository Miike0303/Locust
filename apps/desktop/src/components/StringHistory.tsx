import { projectScopedQueryFn, useProjectScopedQueryEnabled } from "../lib/projectScope";
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight } from "lucide-react";
import { getStringHistory } from "../lib/api";
import { useLocale, useT } from "../lib/i18n";
import { canRestoreRevision, historyStatusLabel } from "../lib/stringHistory";
import DiffView from "./DiffView";

interface StringHistoryProps {
  projectKey: string;
  entryId: string;
  translation: string | null;
  busy: boolean;
  onRestore: (translation: string) => Promise<unknown>;
}

export default function StringHistory({ projectKey, entryId, translation, busy, onRestore }: StringHistoryProps) {
  const t = useT();
  const { locale } = useLocale();
  const [open, setOpen] = useState(false);
  const scopedEnabled = useProjectScopedQueryEnabled(projectKey, open);
  const history = useQuery({
    queryKey: ["string-history", projectKey, entryId],
    queryFn: projectScopedQueryFn(projectKey, () => getStringHistory(entryId)),
    enabled: scopedEnabled,
    staleTime: 0,
  });

  return (
    <section>
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}
        className="flex items-center gap-1 text-caption font-medium text-text-muted hover:text-text">
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        {t("detail.history.title")}
      </button>
      {open && <div className="mt-2 space-y-3 text-caption text-text">
        {history.isPending && <p role="status" className="text-text-muted">{t("common.loading")}</p>}
        {history.isError && <div role="alert" className="text-danger">
          <p>{history.error.message}</p>
          <button type="button" className="underline" onClick={() => history.refetch()}>{t("common.retry")}</button>
        </div>}
        {history.data?.length === 0 && <p className="text-text-muted">{t("detail.history.empty")}</p>}
        {history.data?.map(revision => <article key={revision.id} className="space-y-1.5">
          <time dateTime={revision.changed_at} className="text-text-muted">
            {new Date(revision.changed_at).toLocaleString(locale)}
          </time>
          <p>{historyStatusLabel(revision.old_status, t)} → {historyStatusLabel(revision.new_status, t)}</p>
          {revision.provider && <p className="break-words text-text-muted">{t("detail.provider")}: {revision.provider}</p>}
          <DiffView entryId={entryId} originalText={revision.old_translation ?? ""}
            translatedText={revision.new_translation ?? ""} compact
            originalLabel={t("detail.history.before")} translatedLabel={t("detail.history.after")} />
          {canRestoreRevision(revision.new_translation, translation) && <button type="button" disabled={busy}
            className="text-accent-fg hover:underline disabled:opacity-50" onClick={() => onRestore(revision.new_translation!)}>
            {t("detail.history.restore")}
          </button>}
        </article>)}
      </div>}
    </section>
  );
}
