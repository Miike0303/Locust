import { useQuery } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";
import { useState } from "react";
import ConfirmDialog from "../../components/ConfirmDialog";
import type { BackupRestoreReport } from "../../lib/api";
import { deleteBackup, getBackupsReport, restoreBackup } from "../../lib/api";
import { ApiError } from "../../lib/apiError";
import { useT } from "../../lib/i18n";
import { addToast } from "../../stores/toastStore";

/** Last folder (or file) name from a backup source path. Handles Windows and POSIX. */
function gameFolderName(sourcePath: string): string {
  const trimmed = sourcePath.replace(/[\\/]+$/, "").trim();
  if (!trimmed) return "";
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

export default function DataSection() {
  const t = useT();
  const { data: listing, isPending, isError, error, refetch } = useQuery({
    queryKey: ["backups", "report"], queryFn: getBackupsReport,
  });
  const backups = listing?.entries ?? [];
  const unreadable = listing?.unreadable ?? [];
  const [pendingAction, setPendingAction] = useState<
    { kind: "restore" | "delete"; id: string; game: string; sourcePath: string; confirmUnverified?: boolean } | null
  >(null);
  const [restoreResult, setRestoreResult] = useState<{
    id: string; restored: number; kept: BackupRestoreReport["kept"];
  } | null>(null);

  const gameLabel = (sourcePath: string) =>
    gameFolderName(sourcePath) || t("settings.data.unknownGame");

  const runPendingAction = async () => {
    if (!pendingAction) return;
    const { kind, id, confirmUnverified = false } = pendingAction;
    setPendingAction(null);
    if (kind === "restore") {
      setRestoreResult(null);
      try {
        const report = await restoreBackup(id);
        // Core writes every inventoried file, including those already identical.
        const restored = report.replaced.length + report.recreated.length + report.identical.length;
        if (report.kept.length > 0) {
          setRestoreResult({ id, restored, kept: report.kept });
        } else {
          addToast("success", t("settings.data.toast.restored", { id, count: restored }));
        }
      } catch (e: any) {
        addToast("error", t("settings.data.toast.restoreFailed", { error: e.message }));
      }
    } else {
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
                      <button onClick={() => setPendingAction({ kind: "restore", ...pending })} className="inline-flex items-center justify-center rounded-md p-2 text-accent-fg hover:bg-accent-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent-fg" title={t("settings.data.restore")}><RotateCcw size={14} /></button>
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

      <ConfirmDialog
        open={pendingAction !== null}
        title={pendingAction?.kind === "restore" ? t("settings.data.confirm.restoreTitle") : t("settings.data.confirm.deleteTitle")}
        message={
          pendingAction?.kind === "restore"
            ? t("settings.data.confirm.restoreMessage", {
                id: pendingAction.id,
                game: pendingAction.game,
                path: pendingAction.sourcePath || pendingAction.game,
              })
            : t(pendingAction?.confirmUnverified ? "settings.data.confirm.unverifiedMessage" : "settings.data.confirm.deleteMessage", { id: pendingAction?.id ?? "" })
        }
        confirmLabel={pendingAction?.kind === "restore" ? t("settings.data.restore") : t("common.delete")}
        destructive
        onConfirm={runPendingAction}
        onCancel={() => setPendingAction(null)}
      />
    </div>
  );
}
