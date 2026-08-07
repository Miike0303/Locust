import { useEffect, useState } from "react";
import {
  X, FolderOpen, FileArchive, Package, RotateCcw, AlertCircle, ShieldCheck, Archive,
} from "lucide-react";
import {
  getConfig,
  patchApply,
  patchPack,
  patchRollback,
  patchStatus,
  patchVerify,
  type PatchApplyResult,
  type PatchPackResult,
  type PatchStatusResult,
  type PatchVerifyResult,
} from "../lib/api";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

type Tab = "apply" | "pack";

interface PatchModalProps {
  open: boolean;
  onClose: () => void;
  /** Optional default game path (e.g. current project folder). */
  defaultGamePath?: string;
}

export default function PatchModal({ open, onClose, defaultGamePath }: PatchModalProps) {
  const [tab, setTab] = useState<Tab>("apply");
  const [gamePath, setGamePath] = useState(defaultGamePath ?? "");
  const [zipPath, setZipPath] = useState("");
  const [outputPath, setOutputPath] = useState("");
  const [languages, setLanguages] = useState("");
  const [pristine, setPristine] = useState(false);
  const [force, setForce] = useState(false);
  const [confirmLegacy, setConfirmLegacy] = useState(false);
  const [dryRun, setDryRun] = useState(false);
  const [loading, setLoading] = useState(false);
  const [verify, setVerify] = useState<PatchVerifyResult | null>(null);
  const [applyResult, setApplyResult] = useState<PatchApplyResult | null>(null);
  const [packResult, setPackResult] = useState<PatchPackResult | null>(null);
  const [status, setStatus] = useState<PatchStatusResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    if (defaultGamePath) setGamePath(defaultGamePath);
    setError(null);
    // Prefill pack language from config target lang
    getConfig()
      .then((cfg) => {
        if (cfg.default_target_lang) {
          setLanguages((prev) => (prev.trim() ? prev : cfg.default_target_lang));
        }
      })
      .catch(() => {
        /* config optional for apply tab */
      });
  }, [open, defaultGamePath]);

  if (!open) return null;

  const pickGame = async () => {
    if (IS_TAURI) {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const selected = await openDialog({
        title: tab === "pack" ? "Select recorded game folder to pack" : "Select game folder to patch",
        directory: true,
      });
      if (typeof selected === "string") setGamePath(selected);
    } else {
      const path = prompt("Game folder path:");
      if (path) setGamePath(path);
    }
  };

  const pickZip = async () => {
    if (IS_TAURI) {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const selected = await openDialog({
        title: "Select patch zip",
        multiple: false,
        filters: [{ name: "Patch zip", extensions: ["zip"] }],
      });
      if (typeof selected === "string") setZipPath(selected);
    } else {
      const path = prompt("Patch zip path:");
      if (path) setZipPath(path);
    }
  };

  const pickOutputZip = async () => {
    if (IS_TAURI) {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const selected = await save({
        title: "Save patch zip",
        defaultPath: "locust-patch.zip",
        filters: [{ name: "Patch zip", extensions: ["zip"] }],
      });
      if (typeof selected === "string" && selected) setOutputPath(selected);
    } else {
      const path = prompt("Output patch zip path:", outputPath || "locust-patch.zip");
      if (path) setOutputPath(path);
    }
  };

  const refreshStatus = async () => {
    if (!gamePath.trim()) return;
    try {
      const s = await patchStatus({ game_path: gamePath.trim() });
      setStatus(s);
    } catch (err: any) {
      setStatus(null);
      setError(err.message);
    }
  };

  const handleVerify = async () => {
    if (!gamePath.trim() || !zipPath.trim()) {
      addToast("error", "Select both game folder and patch zip");
      return;
    }
    setLoading(true);
    setError(null);
    setVerify(null);
    setApplyResult(null);
    try {
      const report = await patchVerify({
        game_path: gamePath.trim(),
        zip_path: zipPath.trim(),
      });
      setVerify(report);
      await refreshStatus();
      addLog("info", `Patch verify: ${report.outcome}`, report.messages?.join("\n") || "", "patch");
      addToast("success", `Verify: ${report.outcome}`);
    } catch (err: any) {
      setError(err.message);
      addLog("error", "Patch verify failed", err.message, "patch");
      addToast("error", `Verify failed: ${err.message}`);
    } finally {
      setLoading(false);
    }
  };

  const handleApply = async () => {
    if (!gamePath.trim() || !zipPath.trim()) {
      addToast("error", "Select both game folder and patch zip");
      return;
    }
    setLoading(true);
    setError(null);
    setApplyResult(null);
    try {
      const report = await patchApply({
        game_path: gamePath.trim(),
        zip_path: zipPath.trim(),
        force,
        confirm_legacy: confirmLegacy,
        dry_run: dryRun,
      });
      setApplyResult(report);
      await refreshStatus();
      addLog(
        "info",
        dryRun
          ? `Patch dry-run: ${report.patch_id}@${report.patch_version}`
          : `Patch applied: ${report.patch_id}@${report.patch_version}`,
        `replaced ${report.replaced}, added ${report.added}, baseline ${report.baseline}`,
        "patch"
      );
      addToast(
        "success",
        dryRun
          ? `Planned ${report.replaced + report.added} file(s)`
          : `Applied ${report.replaced + report.added} file(s)`
      );
    } catch (err: any) {
      setError(err.message);
      addLog("error", "Patch apply failed", err.message, "patch");
      addToast("error", `Apply failed: ${err.message}`);
    } finally {
      setLoading(false);
    }
  };

  const handleRollback = async () => {
    if (!gamePath.trim()) {
      addToast("error", "Select a game folder");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const report = await patchRollback({
        game_path: gamePath.trim(),
        force,
      });
      if (report.aborted_edited?.length) {
        addToast(
          "error",
          `Rollback aborted: ${report.aborted_edited.length} edited file(s) need Force`
        );
      } else {
        addToast(
          "success",
          `Rollback: restored ${report.restored}, deleted ${report.deleted}`
        );
      }
      addLog(
        "info",
        "Patch rollback",
        report.messages?.join("\n") ||
          `restored ${report.restored}, deleted ${report.deleted}`,
        "patch"
      );
      setApplyResult(null);
      setVerify(null);
      await refreshStatus();
    } catch (err: any) {
      setError(err.message);
      addLog("error", "Patch rollback failed", err.message, "patch");
      addToast("error", `Rollback failed: ${err.message}`);
    } finally {
      setLoading(false);
    }
  };

  const handlePack = async () => {
    if (!gamePath.trim()) {
      addToast("error", "Select the recorded game folder to pack");
      return;
    }
    if (!outputPath.trim()) {
      addToast("error", "Choose an output zip path");
      return;
    }
    setLoading(true);
    setError(null);
    setPackResult(null);
    try {
      const langs = languages
        .split(/[,\s]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      const report = await patchPack({
        game_path: gamePath.trim(),
        output_path: outputPath.trim(),
        languages: langs,
        pristine,
      });
      setPackResult(report);
      addLog(
        "info",
        `Patch packed: ${report.patch_id}@${report.patch_version}`,
        `${report.files_packed} file(s), ${report.size_bytes} bytes, tier ${report.tier}`,
        "patch"
      );
      addToast("success", `Packed ${report.files_packed} file(s) → ${report.tier}`);
    } catch (err: any) {
      setError(err.message);
      addLog("error", "Patch pack failed", err.message, "patch");
      addToast("error", `Pack failed: ${err.message}`);
    } finally {
      setLoading(false);
    }
  };

  const tabBtn = (id: Tab, label: string) => (
    <button
      type="button"
      onClick={() => {
        setTab(id);
        setError(null);
      }}
      className={`px-3 py-1.5 text-sm font-medium rounded-t border-b-2 ${
        tab === id
          ? "border-emerald-600 text-emerald-700 dark:text-emerald-400"
          : "border-transparent text-gray-500 hover:text-gray-700 dark:hover:text-gray-300"
      }`}
    >
      {label}
    </button>
  );

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-xl p-6 max-h-[90vh] overflow-y-auto">
        <div className="flex justify-between items-center mb-2">
          <h2 className="text-lg font-bold flex items-center gap-2">
            <Package size={20} /> Patch
          </h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X size={20} />
          </button>
        </div>

        <div className="flex gap-1 border-b dark:border-gray-700 mb-4">
          {tabBtn("apply", "Apply")}
          {tabBtn("pack", "Pack")}
        </div>

        <div className="space-y-4">
          <div>
            <label className="text-sm font-medium">
              {tab === "pack" ? "Recorded game folder" : "Game folder"}
            </label>
            <div className="flex gap-2 mt-1">
              <input
                value={gamePath}
                onChange={(e) => setGamePath(e.target.value)}
                placeholder={
                  tab === "pack"
                    ? "Root of the inject recording (game or *-lang copy)..."
                    : "Path to the game to patch..."
                }
                className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
              />
              <button
                onClick={pickGame}
                className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
                title="Browse"
              >
                <FolderOpen size={16} />
              </button>
            </div>
          </div>

          {tab === "apply" && (
            <>
              <div>
                <label className="text-sm font-medium">Patch zip</label>
                <div className="flex gap-2 mt-1">
                  <input
                    value={zipPath}
                    onChange={(e) => setZipPath(e.target.value)}
                    placeholder="locust-*-patch.zip..."
                    className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
                  />
                  <button
                    onClick={pickZip}
                    className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
                    title="Browse"
                  >
                    <FileArchive size={16} />
                  </button>
                </div>
              </div>

              <div className="flex flex-wrap gap-4 text-sm">
                <label className="flex items-center gap-2 cursor-pointer">
                  <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} />
                  Force (override mismatch / already-applied / unknown)
                </label>
                <label className="flex items-center gap-2 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={confirmLegacy}
                    onChange={(e) => setConfirmLegacy(e.target.checked)}
                  />
                  Confirm legacy / structural (no original hashes)
                </label>
                <label className="flex items-center gap-2 cursor-pointer">
                  <input type="checkbox" checked={dryRun} onChange={(e) => setDryRun(e.target.checked)} />
                  Dry run
                </label>
              </div>

              <div className="flex flex-wrap gap-2">
                <button
                  onClick={handleVerify}
                  disabled={loading}
                  className="flex items-center gap-1.5 px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium disabled:opacity-50"
                >
                  <ShieldCheck size={16} /> Verify
                </button>
                <button
                  onClick={handleApply}
                  disabled={loading}
                  className="flex items-center gap-1.5 px-3 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium disabled:opacity-50"
                >
                  <Package size={16} /> {dryRun ? "Plan apply" : "Apply"}
                </button>
                <button
                  onClick={handleRollback}
                  disabled={loading}
                  className="flex items-center gap-1.5 px-3 py-2 bg-amber-100 hover:bg-amber-200 text-amber-900 dark:bg-amber-900/40 dark:text-amber-100 dark:hover:bg-amber-900/60 rounded text-sm font-medium disabled:opacity-50"
                >
                  <RotateCcw size={16} /> Rollback
                </button>
                <button
                  onClick={refreshStatus}
                  disabled={loading || !gamePath.trim()}
                  className="px-3 py-2 text-sm text-gray-600 hover:underline disabled:opacity-50"
                >
                  Refresh status
                </button>
              </div>

              {status && (
                <div className="p-3 bg-gray-50 dark:bg-gray-800/60 rounded text-sm space-y-1">
                  <div className="font-medium">Status: {status.status}</div>
                  {status.status === "patched" && (
                    <div className="text-xs text-gray-600 dark:text-gray-400">
                      {status.patch_id}@{status.patch_version} · {status.engine} · {status.language} ·
                      baseline {status.baseline} · replaced {status.replaced} · added {status.added}
                    </div>
                  )}
                  {status.status === "interrupted" && (
                    <div className="text-xs text-amber-600">
                      Interrupted apply of {status.patch_id} — run rollback
                    </div>
                  )}
                </div>
              )}

              {verify && (
                <div className="p-3 border rounded dark:border-gray-700 text-sm space-y-1">
                  <div className="font-medium">Verify: {verify.outcome}</div>
                  {verify.tier && <div className="text-xs">Tier: {verify.tier}</div>}
                  <div className="text-xs text-gray-600 dark:text-gray-400">
                    plan: {verify.replaced?.length ?? 0} replace, {verify.added?.length ?? 0} add
                    {(verify.conflicts?.length ?? 0) > 0 && `, ${verify.conflicts.length} conflict(s)`}
                  </div>
                  {verify.backup_compromised && (
                    <div className="text-xs text-amber-600">Backup compromised warning</div>
                  )}
                  {verify.messages?.map((m, i) => (
                    <div key={i} className="text-xs text-gray-500">{m}</div>
                  ))}
                </div>
              )}

              {applyResult && (
                <div className="p-3 border border-emerald-200 dark:border-emerald-800 bg-emerald-50/50 dark:bg-emerald-950/30 rounded text-sm space-y-1">
                  <div className="font-medium">
                    {applyResult.dry_run ? "Planned" : "Applied"} {applyResult.patch_id}@
                    {applyResult.patch_version}
                  </div>
                  <div className="text-xs">
                    replaced {applyResult.replaced}, added {applyResult.added}, baseline{" "}
                    {applyResult.baseline}
                  </div>
                  {applyResult.user_edits_overwritten?.length > 0 && (
                    <div className="text-xs text-amber-700">
                      Overwrote user edits: {applyResult.user_edits_overwritten.join(", ")}
                    </div>
                  )}
                </div>
              )}

              <p className="text-xs text-gray-500">
                Verify is read-only. Apply writes under the game&apos;s{" "}
                <code className="px-1 bg-gray-100 dark:bg-gray-800 rounded">.locust/</code> backup so
                rollback can restore pristine files. Prefer packing with pristine hashes for strict
                verification.
              </p>
            </>
          )}

          {tab === "pack" && (
            <>
              <div>
                <label className="text-sm font-medium">Output zip</label>
                <div className="flex gap-2 mt-1">
                  <input
                    value={outputPath}
                    onChange={(e) => setOutputPath(e.target.value)}
                    placeholder="Where to write the patch zip..."
                    className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
                  />
                  <button
                    onClick={pickOutputZip}
                    className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded"
                    title="Save as"
                  >
                    <FileArchive size={16} />
                  </button>
                </div>
              </div>

              <div>
                <label className="text-sm font-medium">Language(s)</label>
                <input
                  value={languages}
                  onChange={(e) => setLanguages(e.target.value)}
                  placeholder="e.g. es (one language per zip; empty = auto)"
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
                />
                <p className="text-xs text-gray-500 mt-1">
                  One recording per zip. Leave empty when a single injection language was recorded.
                </p>
              </div>

              <label className="flex items-start gap-2 text-sm cursor-pointer">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={pristine}
                  onChange={(e) => setPristine(e.target.checked)}
                />
                <span>
                  <span className="font-medium">Pristine hashes</span>
                  <span className="block text-xs text-gray-500 mt-0.5">
                    Require original file hashes from a valid{" "}
                    <code className="px-0.5 bg-gray-100 dark:bg-gray-800 rounded">.locust/backup</code>{" "}
                    so apply can use strict-tier verification. Fails if no backup exists.
                  </span>
                </span>
              </label>

              <button
                onClick={handlePack}
                disabled={loading}
                className="flex items-center gap-1.5 px-3 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium disabled:opacity-50"
              >
                <Archive size={16} /> Pack
              </button>

              {packResult && (
                <div className="p-3 border border-emerald-200 dark:border-emerald-800 bg-emerald-50/50 dark:bg-emerald-950/30 rounded text-sm space-y-1">
                  <div className="font-medium">
                    Packed {packResult.patch_id}@{packResult.patch_version}
                  </div>
                  <div className="text-xs text-gray-600 dark:text-gray-400 space-y-0.5">
                    <div>
                      {packResult.files_packed} file(s) · {packResult.size_bytes} bytes · tier{" "}
                      <span className="font-medium">{packResult.tier}</span>
                    </div>
                    <div>
                      engine {packResult.engine} · language {packResult.language}
                      {packResult.translated_strings != null &&
                        ` · ${packResult.translated_strings} translated string(s)`}
                    </div>
                    <div className="break-all">{packResult.output_path}</div>
                  </div>
                  {packResult.messages?.map((m, i) => (
                    <div key={i} className="text-xs text-amber-700 dark:text-amber-300">
                      {m}
                    </div>
                  ))}
                </div>
              )}

              <p className="text-xs text-gray-500">
                Packs exactly the files a recorded inject wrote. Run inject first, then point the
                game folder at that recording root (project path for direct inject, or the{" "}
                <code className="px-1 bg-gray-100 dark:bg-gray-800 rounded">*-lang</code> copy when
                inject used an output folder).
              </p>
            </>
          )}

          {error && (
            <div className="flex gap-2 p-3 bg-red-50 dark:bg-red-950/40 border border-red-200 dark:border-red-800 rounded text-sm text-red-700 dark:text-red-300">
              <AlertCircle size={16} className="shrink-0 mt-0.5" />
              <pre className="whitespace-pre-wrap break-words font-sans">{error}</pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
