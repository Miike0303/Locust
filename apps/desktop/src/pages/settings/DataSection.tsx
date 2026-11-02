import { useQuery } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";
import { useRef, useState } from "react";
import ConfirmDialog from "../../components/ConfirmDialog";
import type { BackupRestoreReport } from "../../lib/api";
import { deleteBackup, getBackupsReport, previewBackupRestore, restoreBackup } from "../../lib/api";
import { ApiError } from "../../lib/apiError";
import { useT } from "../../lib/i18n";
import {
  useModalA11y, MODAL_BACKDROP_CLASS, MODAL_HEADER_CLASS,
  MODAL_BODY_CLASS, MODAL_FOOTER_CLASS, modalPanelClass,
} from "../../lib/modalA11y";
import { addToast } from "../../stores/toastStore";

/** Last folder (or file) name from a backup source path. Handles Windows and POSIX. */
function gameFolderName(sourcePath: string): string {
  const trimmed = sourcePath.replace(/[\\/]+$/, "").trim();
  if (!trimmed) return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

type PendingAction = {
  kind: "restore" | "delete"; id: string; game: string; sourcePath: string; confirmUnverified?: boolean;
};
type PreviewState =
  | { status: "loading" }
  | { status: "ready"; report: BackupRestoreReport }
  | { status: "error"; error: string };

function RestoreConfirmation({ open, action, preview, onConfirm, onCancel, onRetry }: {
  open: boolean; action: PendingAction | null; preview: PreviewState;
  onConfirm: () => void; onCancel: () => void; onRetry: () => void;
}) {
  const t = useT();
  const { dialogRef, dialogProps, titleProps } = useModalA11y({ open, onClose: onCancel, ownEscape: true });
  if (!open || !action) return null;
  const report = preview.status === "ready" ? preview.report : null;
  const categories = report ? [
    { label: t("settings.data.preview.replaced", { count: report.replaced.length }), paths: report.replaced },
    { label: t("settings.data.preview.recreated", { count: report.recreated.length }), paths: report.recreated },
    { label: t("settings.data.preview.identical", { count: report.identical.length }), paths: report.identical },
    { label: t("settings.data.preview.removed", { count: report.removed.length }), paths: report.removed },
    { label: t("settings.data.preview.removedDirectories", { count: report.removed_directories.length }), paths: report.removed_directories },
    { label: t("settings.data.preview.kept", { count: report.kept.length }), paths: report.kept.map(entry => entry.path), reasons: report.kept.map(entry => entry.reason) },
  ] : [];
  return (
    <div className={MODAL_BACKDROP_CLASS} onClick={onCancel}>
      <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-2xl")} onClick={event => event.stopPropagation()}>
        <header className={`${MODAL_HEADER_CLASS} px-5 pt-5 pb-2`}>
          <h2 {...titleProps} className="text-section font-bold">{t("settings.data.confirm.restoreTitle")}</h2>
        </header>
        <div className={`${MODAL_BODY_CLASS} px-5 pb-4 space-y-3`}>
          <p className="text-body text-text-muted break-words">
            {t("settings.data.confirm.restoreMessage", {
              id: action.id, game: action.game, path: report?.destination || action.sourcePath || action.game,
            })}
          </p>
          {preview.status === "loading" && <p role="status">{t("settings.data.preview.loading")}</p>}
          {preview.status === "error" && (
            <div role="alert" className="text-danger space-y-2 break-words">
              <p>{t("settings.data.preview.failed", { error: preview.error })}</p>
              <p>{t("settings.data.preview.recovery")}</p>
              <button type="button" onClick={onRetry} className="px-3 py-2 rounded border border-border text-text hover:bg-surface-muted">{t("common.retry")}</button>
            </div>
          )}
          {report && <>
            <p className="text-body text-text-muted">{t("settings.data.preview.advisory")}</p>
            {categories.map(category => (
              <section key={category.label} className="space-y-1">
                <h3 className="text-body font-semibold">{category.label}</h3>
                <ul className="text-caption space-y-1">
                  {category.paths.slice(0, 20).map((path, index) => (
                    <li key={path}>
                      <div className="font-mono break-all">{path}</div>
                      {category.reasons && <div className="text-text-muted break-words">{category.reasons[index]}</div>}
                    </li>
                  ))}
                </ul>
                {category.paths.length > 20 && <p className="text-caption">{t("settings.data.restoreKeptMore", { count: category.paths.length - 20 })}</p>}
              </section>
            ))}
          </>}
        </div>
        <footer className={MODAL_FOOTER_CLASS}>
          <button type="button" onClick={onCancel} className="px-3 py-2 text-body rounded border border-border bg-surface text-text hover:bg-surface-muted">{t("common.cancel")}</button>
          <button type="button" disabled={!report} onClick={onConfirm} className="px-4 py-2 text-body font-medium rounded bg-danger text-danger-contrast hover:bg-danger/90 disabled:opacity-50 disabled:cursor-not-allowed">{t("settings.data.restore")}</button>
        </footer>
      </div>
    </div>
  );
}

export default function DataSection() {
  const t = useT();
  const { data: listing, isPending, isError, error, refetch } = useQuery({
    queryKey: ["backups", "report"], queryFn: getBackupsReport,
  });
  const backups = listing?.entries ?? [];
  const unreadable = listing?.unreadable ?? [];
  const [pendingAction, setPendingAction] = useState<PendingAction | null>(null);
  const [preview, setPreview] = useState<PreviewState>({ status: "loading" });
  const previewRequest = useRef(0);
  const previewTask = useRef<Promise<BackupRestoreReport> | null>(null);
  const cancelPendingAction = () => {
    previewRequest.current++;
    setPendingAction(null);
  };
  const openRestore = async (action: PendingAction) => {
    const request = ++previewRequest.current;
    setPendingAction(action);
    setPreview({ status: "loading" });
    try {
      const task = previewBackupRestore(action.id);
      previewTask.current = task;
      const report = await task;
      if (request === previewRequest.current) setPreview({ status: "ready", report });
    } catch (error) {
      if (request === previewRequest.current) {
        setPreview({ status: "error", error: error instanceof Error ? error.message : String(error) });
      }
    }
  };
  const [restoreResult, setRestoreResult] = useState<{
    id: string; restored: number; kept: BackupRestoreReport["kept"];
  } | null>(null);

  const gameLabel = (sourcePath: string) =>
    gameFolderName(sourcePath) || t("settings.data.unknownGame");

  const runPendingAction = async () => {
    if (!pendingAction) return;
    const { kind, id, confirmUnverified = false } = pendingAction;
    if (kind === "restore") {
      const request = previewRequest.current;
      let restoring = false;
      setRestoreResult(null);
      try {
        // The button stays disabled until preview succeeds. Also protect callers
        // already awaiting it if Cancel or a different preview supersedes it.
        if (!previewTask.current) return;
        await previewTask.current;
        if (request !== previewRequest.current) return;
        cancelPendingAction();
        restoring = true;
        const report = await restoreBackup(id);
        // Core writes every inventoried file, including those already identical.
        const restored = report.replaced.length + report.recreated.length + report.identical.length;
        if (report.kept.length > 0) {
          setRestoreResult({ id, restored, kept: report.kept });
        } else {
          addToast("success", t("settings.data.toast.restored", { id, count: restored }));
        }
      } catch (e: any) {
        if (restoring || request === previewRequest.current) {
          addToast("error", t("settings.data.toast.restoreFailed", { error: e.message }));
        }
      }
    } else {
      cancelPendingAction();
      try {
        await deleteBackup(id, confirmUnverified);
        refetch();
      } catch (e: any) {
        if (!confirmUnverified && e instanceof ApiError && e.key === "api.error.backupUnverified") {
          setPendingAction({ ...pendingAction, confirmUnverified: true });
          return;
        }
        addToast("error", t("settings.data.toast.deleteFailed", { error: e.message }));
      }
    }
  };

  return (
    <div className="space-y-6">
      <h2 className="text-page font-bold">{t("settings.data.title")}</h2>
      {restoreResult && (
        <div role="alert" className="p-4 border border-warning bg-warning-muted text-warning rounded-xl space-y-2">
          <p className="font-semibold">
            {t("settings.data.toast.restored", { id: restoreResult.id, count: restoreResult.restored })}
          </p>
          <p>{t("settings.data.restoreKept")}</p>
          <ul className="space-y-2 text-body">
            {restoreResult.kept.slice(0, 10).map(({ path, reason }) => (
              <li key={path}>
                <div className="font-mono text-caption break-all">{path}</div>
                <div className="text-caption break-words">{reason}</div>
              </li>
            ))}
          </ul>
          {restoreResult.kept.length > 10 && (
            <p className="text-caption">{t("settings.data.restoreKeptMore", { count: restoreResult.kept.length - 10 })}</p>
          )}
        </div>
      )}
      <div>
        <h3 className="text-section font-semibold mb-3">{t("settings.data.backups")}</h3>
        {isPending ? (
          <p className="text-body text-text-muted" role="status">{t("common.loading")}</p>
        ) : isError ? (
          <p className="text-body text-danger" role="alert">
            {t("settings.data.loadFailed", { error: error.message })}
          </p>
        ) : backups.length === 0 && unreadable.length === 0 ? (
          <p className="text-body text-text-muted">{t("settings.data.noBackups")}</p>
        ) : backups.length > 0 ? (
          <div className="overflow-x-auto border border-border bg-surface rounded-xl">
          <table className="w-full min-w-[48rem] text-body">
            <thead>
              <tr className="bg-surface-muted text-left text-text-muted text-caption uppercase tracking-wide">
                <th className="px-3 py-2">{t("settings.data.col.game")}</th>
                <th className="px-3 py-2">{t("settings.data.col.id")}</th>
                <th className="px-3 py-2">{t("settings.data.col.created")}</th>
                <th className="px-3 py-2">{t("settings.data.col.files")}</th>
                <th className="px-3 py-2">{t("settings.data.col.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {backups.map((b) => {
                const game = gameLabel(b.source_path);
                const pending = { id: b.id, game, sourcePath: b.source_path };
                return (
                  <tr key={b.id} className="border-t border-border hover:bg-surface-muted">
                    <td className="px-3 py-2 max-w-[24rem]" title={b.source_path || undefined}>
                      <div className="font-medium break-words">{game}</div>
                      {b.source_path ? (
                        <div className="text-caption text-text-muted break-all">{b.source_path}</div>
                      ) : null}
                    </td>
                    <td className="px-3 py-2 font-mono text-caption">{b.id}</td>
                    <td className="px-3 py-2 whitespace-nowrap">{new Date(b.created_at).toLocaleString()}</td>
                    <td className="px-3 py-2 tabular-nums">{b.file_count}</td>
                    <td className="px-3 py-2 flex gap-2">
                      <button onClick={() => openRestore({ kind: "restore", ...pending })} className="inline-flex items-center justify-center rounded-md p-2 text-accent-fg hover:bg-accent-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg" title={t("settings.data.restore")}><RotateCcw size={14} /></button>
                      <button onClick={() => setPendingAction({ kind: "delete", ...pending })} className="inline-flex items-center justify-center rounded-md p-2 text-danger hover:bg-danger-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-danger" title={t("settings.data.delete")}><Trash2 size={14} /></button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          </div>
        ) : null}
        {!isPending && !isError && unreadable.length > 0 && (
          <div className="mt-4 p-4 border border-warning bg-warning-muted text-warning rounded-xl space-y-2">
            <h4 className="text-section font-semibold">{t("settings.data.damagedBackups")}</h4>
            <p className="text-caption">{t("settings.data.damagedHint")}</p>
            <ul className="space-y-2 text-body">
              {unreadable.map((backup) => {
                const diagnostic = backup.error.replace(/\s+/g, " ").trim();
                const shortError = diagnostic.length > 200 ? `${diagnostic.slice(0, 200)}…` : diagnostic;
                return (
                  <li key={backup.id}>
                    <div className="font-mono text-caption break-all">{backup.id}</div>
                    <div className="text-caption break-words">{shortError}</div>
                  </li>
                );
              })}
            </ul>
          </div>
        )}
      </div>

      <RestoreConfirmation
        open={pendingAction?.kind === "restore"}
        action={pendingAction}
        preview={preview}
        onConfirm={runPendingAction}
        onCancel={cancelPendingAction}
        onRetry={() => { if (pendingAction?.kind === "restore") return openRestore(pendingAction); }}
      />
      <ConfirmDialog
        open={pendingAction?.kind === "delete"}
        title={t("settings.data.confirm.deleteTitle")}
        message={t(pendingAction?.confirmUnverified ? "settings.data.confirm.unverifiedMessage" : "settings.data.confirm.deleteMessage", { id: pendingAction?.id ?? "" })}
        confirmLabel={t("common.delete")}
        destructive
        onConfirm={runPendingAction}
        onCancel={cancelPendingAction}
      />
    </div>
  );
}
