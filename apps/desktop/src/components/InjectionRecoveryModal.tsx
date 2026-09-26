import { useEffect, useRef, useState } from "react";
import { FolderOpen, Loader2, RotateCcw, X } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import {
  getInjectionStatus, recoverInjection,
  type InjectionStatus, type InjectionRecoveryReport,
} from "../lib/api";
import { pickGameFolder } from "../lib/openProjectFlow";
import { useT } from "../lib/i18n";
import { useModalA11y, MODAL_BACKDROP_CLASS, modalPanelClass } from "../lib/modalA11y";

/** Mounted for one recovery session; close/reopen always drops prior consent. */
export default function InjectionRecoveryModal({
  defaultGamePath = "", onClose,
}: { defaultGamePath?: string; onClose: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const [gamePath, setGamePath] = useState(defaultGamePath);
  const [status, setStatus] = useState<InjectionStatus | null>(null);
  const [report, setReport] = useState<InjectionRecoveryReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmedConflicts, setConfirmedConflicts] = useState(false);
  const active = useRef(true);
  const running = useRef(false);
  const { dialogRef, dialogProps, titleProps } = useModalA11y({
    open: true, ownEscape: true,
    onClose: () => { if (!running.current) onClose(); },
  });
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  const resetPath = (value: string) => {
    setGamePath(value);
    setStatus(null);
    setReport(null);
    setError(null);
    setConfirmedConflicts(false);
  };
  const inspect = async () => {
    if (running.current || !gamePath.trim()) return;
    running.current = true;
    setBusy(true);
    setStatus(null);
    setReport(null);
    setError(null);
    setConfirmedConflicts(false);
    try {
      const next = await getInjectionStatus(gamePath.trim());
      if (active.current) setStatus(next);
    } catch (err) {
      if (active.current) setError(err instanceof Error ? err.message : String(err));
    } finally {
      running.current = false;
      if (active.current) setBusy(false);
    }
  };
  const pending = status?.pending;
  const conflicts = pending?.conflicts ?? [];
  const recover = async () => {
    if (running.current || !status || !pending || (conflicts.length > 0 && !confirmedConflicts)) return;
    running.current = true;
    setBusy(true);
    setError(null);
    setConfirmedConflicts(false);
    try {
      // Canonical root and reviewed operation ID come from the same inspection.
      const next = await recoverInjection(status.game_root, pending.transaction_id, conflicts.length > 0);
      if (!active.current) return;
      setReport(next);
      setStatus(null);
      // Recordings may now reference reverted bytes; never continue using cached
      // availability. The backend still validates their hashes before packing.
      void queryClient.invalidateQueries({ queryKey: ["patch-recordings"] });
    } catch (err) {
      if (active.current) {
        setError(err instanceof Error ? err.message : String(err));
        // A failed or stale request requires a fresh review before any retry.
        setStatus(null);
      }
    } finally {
      running.current = false;
      if (active.current) setBusy(false);
    }
  };
  const browse = async () => {
    if (running.current) return;
    try {
      const selected = await pickGameFolder(t);
      if (selected && active.current && !running.current) resetPath(selected);
    } catch (err) {
      if (active.current) setError(err instanceof Error ? err.message : String(err));
    }
  };

  return (
    <div className={MODAL_BACKDROP_CLASS}>
      <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-2xl mx-4 max-h-[90vh] overflow-y-auto")}>
        <header className="flex items-center justify-between border-b border-gray-200 dark:border-gray-700 px-5 py-4">
          <h2 {...titleProps} className="font-semibold flex items-center gap-2"><RotateCcw size={18} />{t("recovery.title")}</h2>
          <button type="button" onClick={onClose} disabled={busy} aria-label={t("recovery.close")} className="p-1 disabled:opacity-40"><X size={20} /></button>
        </header>
        <div className="p-5 space-y-4 text-sm">
          <p className="text-gray-600 dark:text-gray-300">{t("recovery.description")}</p>
          <label className="block">
            <span className="block mb-1 font-medium">{t("recovery.gamePath")}</span>
            <input value={gamePath} onChange={e => resetPath(e.target.value)} disabled={busy} className="w-full px-3 py-2 border rounded bg-transparent dark:border-gray-600" />
          </label>
          <div className="flex gap-2">
            <button type="button" onClick={() => void browse()} disabled={busy} className="flex gap-2 items-center px-3 py-2 rounded bg-gray-100 dark:bg-gray-800 disabled:opacity-40"><FolderOpen size={16} />{t("recovery.browse")}</button>
            <button type="button" onClick={() => void inspect()} disabled={busy || !gamePath.trim()} className="px-3 py-2 rounded bg-gray-100 dark:bg-gray-800 disabled:opacity-40">{t("recovery.inspect")}</button>
          </div>
          {busy && <p role="status" className="flex gap-2 items-center"><Loader2 size={16} className="animate-spin" />{t("recovery.working")}</p>}
          {error && <p role="alert" className="text-red-700 dark:text-red-300 whitespace-pre-wrap break-words">{error}</p>}
          {status && !pending && <p role="status" className="text-emerald-700 dark:text-emerald-300">{t("recovery.clean")}</p>}
          {pending && (
            <section className="space-y-3 border rounded p-3 dark:border-gray-700" aria-label={t("recovery.pending")}>
              <h3 className="font-semibold">{t("recovery.pending")}</h3>
              <p className="break-all text-xs">{status.game_root}</p>
              <p>{t("recovery.phase", { phase: t(`recovery.phase.${pending.phase}`) })}</p>
              <p>{t("recovery.summary", { format: pending.format, language: pending.language ?? "—", count: pending.changed_files })}</p>
              <p className="text-gray-600 dark:text-gray-300">{t("recovery.effect")}</p>
              {conflicts.length > 0 && (
                <div className="space-y-2">
                  <p className="font-medium text-amber-800 dark:text-amber-200">{t("recovery.conflicts")}</p>
                  <ul className="list-disc pl-5 max-h-40 overflow-y-auto break-words">{conflicts.map(c => <li key={c.path}><span className="font-mono">{c.path}</span>: {c.reason}</li>)}</ul>
                  <label className="flex gap-2 items-start"><input type="checkbox" checked={confirmedConflicts} onChange={e => setConfirmedConflicts(e.target.checked)} disabled={busy} className="mt-1" /><span>{t("recovery.confirmConflicts")}</span></label>
                </div>
              )}
              <button type="button" onClick={() => void recover()} disabled={busy || (conflicts.length > 0 && !confirmedConflicts)} className="px-4 py-2 bg-amber-700 hover:bg-amber-800 text-white rounded disabled:opacity-40">{conflicts.length ? t("recovery.restoreWithCopies") : t("recovery.restore")}</button>
            </section>
          )}
          {report && <section role="status" className="space-y-2 text-emerald-800 dark:text-emerald-200">
            <h3 className="font-semibold">{t("recovery.done")}</h3>
            <p>{t("recovery.result", { restored: report.restored, removed: report.removed })}</p>
            <p>{t("recovery.reopen")}</p>
            {report.preserved_conflicts.length > 0 && <><p>{t("recovery.copies")}</p><ul className="max-h-40 overflow-y-auto break-all text-xs">{report.preserved_conflicts.map(p => <li key={p}>{p}</li>)}</ul></>}
            {report.messages.length > 0 && <details className="text-gray-600 dark:text-gray-300"><summary>{t("recovery.details")}</summary>{report.messages.map((m, i) => <p key={i} className="break-words">{m}</p>)}</details>}
          </section>}
        </div>
      </div>
    </div>
  );
}
