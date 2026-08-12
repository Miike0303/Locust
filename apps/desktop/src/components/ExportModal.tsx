import { useState, useRef } from "react";
import { X, Download, Upload, FolderOpen } from "lucide-react";
import {
  exportTranslations,
  importTranslations,
  type ExportFormat,
} from "../lib/api";
import { LANGUAGES } from "../lib/languages";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import { useModalA11y, MODAL_BACKDROP_CLASS, modalPanelClass } from "../lib/modalA11y";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

type Mode = "export" | "import";

interface ExportModalProps {
  open: boolean;
  onClose: () => void;
  onImported?: () => void;
}

export default function ExportModal({ open, onClose, onImported }: ExportModalProps) {
  const { project } = useProjectStore();
  const [mode, setMode] = useState<Mode>("export");
  const [format, setFormat] = useState<ExportFormat>("po");
  const [lang, setLang] = useState("es");
  const [loading, setLoading] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { dialogRef, dialogProps, titleProps } = useModalA11y({ open, ownEscape: false });

  if (!open || !project) return null;

  const defaultName = `translation_${lang}.${format === "po" ? "po" : "xliff"}`;

  const handleExport = async () => {
    setLoading(true);
    try {
      let path: string | undefined;
      if (IS_TAURI) {
        const { save } = await import("@tauri-apps/plugin-dialog");
        const selected = await save({
          title: "Export translations",
          defaultPath: defaultName,
          filters: [
            format === "po"
              ? { name: "Gettext PO", extensions: ["po"] }
              : { name: "XLIFF", extensions: ["xliff", "xlf"] },
          ],
        });
        if (typeof selected !== "string" || !selected) {
          setLoading(false);
          return;
        }
        path = selected;
      }

      const result = await exportTranslations(format, lang, path);
      addToast("success", `Exported ${format.toUpperCase()} → ${result.path}`);
      addLog(
        "info",
        `Export ${format} (${lang}): ${result.bytes} bytes`,
        result.path,
        "export"
      );
      onClose();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      addToast("error", `Export failed: ${msg}`);
      addLog("error", "Export failed", msg, "export");
    } finally {
      setLoading(false);
    }
  };

  const runImportFromPath = async (path: string) => {
    setLoading(true);
    try {
      const result = await importTranslations(format, path);
      addToast(
        "success",
        `Imported ${result.imported} translation(s)${
          result.skipped ? ` (${result.skipped} skipped)` : ""
        }`
      );
      addLog(
        "info",
        `Import ${format}: ${result.imported} applied, ${result.skipped} skipped`,
        result.path,
        "import"
      );
      onImported?.();
      onClose();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      addToast("error", `Import failed: ${msg}`);
      addLog("error", "Import failed", msg, "import");
    } finally {
      setLoading(false);
    }
  };

  const handleImport = async () => {
    if (IS_TAURI) {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const selected = await openDialog({
        title: "Import translations",
        multiple: false,
        filters: [
          format === "po"
            ? { name: "Gettext PO", extensions: ["po"] }
            : { name: "XLIFF", extensions: ["xliff", "xlf", "xml"] },
        ],
      });
      if (typeof selected !== "string" || !selected) return;
      await runImportFromPath(selected);
      return;
    }
    fileInputRef.current?.click();
  };

  const handleBrowserFile = async (file: File | null) => {
    if (!file) return;
    const text = await file.text();
    setLoading(true);
    try {
      const result = await importTranslations(format, text);
      addToast("success", `Imported ${result.imported} translation(s)`);
      addLog(
        "info",
        `Import ${format}: ${result.imported} applied`,
        file.name,
        "import"
      );
      onImported?.();
      onClose();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      addToast("error", `Import failed: ${msg}`);
      addLog("error", "Import failed", msg, "import");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className={MODAL_BACKDROP_CLASS}>
      <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-md p-6")}>
        <div className="flex justify-between items-center mb-4">
          <h2 {...titleProps} className="text-lg font-bold flex items-center gap-2">
            {mode === "export" ? <Download size={18} /> : <Upload size={18} />}
            {mode === "export" ? "Export Translations" : "Import Translations"}
          </h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X size={20} />
          </button>
        </div>

        <div className="flex gap-1 mb-4 p-1 bg-gray-100 dark:bg-gray-800 rounded">
          <button
            type="button"
            onClick={() => setMode("export")}
            className={`flex-1 py-1.5 text-sm rounded transition-colors ${
              mode === "export"
                ? "bg-white dark:bg-gray-700 shadow font-medium"
                : "text-gray-500 hover:text-gray-700"
            }`}
          >
            Export
          </button>
          <button
            type="button"
            onClick={() => setMode("import")}
            className={`flex-1 py-1.5 text-sm rounded transition-colors ${
              mode === "import"
                ? "bg-white dark:bg-gray-700 shadow font-medium"
                : "text-gray-500 hover:text-gray-700"
            }`}
          >
            Import
          </button>
        </div>

        <div className="space-y-4">
          <div>
            <label className="text-sm font-medium">Format</label>
            <select
              value={format}
              onChange={(e) => setFormat(e.target.value as ExportFormat)}
              className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
            >
              <option value="po">Gettext PO (.po)</option>
              <option value="xliff">XLIFF 1.2 (.xliff)</option>
            </select>
          </div>

          {mode === "export" && (
            <div>
              <label className="text-sm font-medium">Target language</label>
              <select
                value={lang}
                onChange={(e) => setLang(e.target.value)}
                className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
              >
                {LANGUAGES.map((l) => (
                  <option key={l.code} value={l.code}>
                    {l.label} ({l.code})
                  </option>
                ))}
              </select>
            </div>
          )}

          {mode === "import" && (
            <p className="text-xs text-gray-500">
              Matches entries by Locust string id embedded in the file. Empty
              translations are skipped. Re-open strings in the editor after import.
            </p>
          )}

          <input
            ref={fileInputRef}
            type="file"
            accept={format === "po" ? ".po,text/plain" : ".xliff,.xlf,.xml,application/xml"}
            className="hidden"
            onChange={(e) => {
              void handleBrowserFile(e.target.files?.[0] ?? null);
              e.target.value = "";
            }}
          />

          <div className="flex justify-end gap-2 pt-2">
            <button
              onClick={onClose}
              className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
            >
              Cancel
            </button>
            <button
              onClick={() => {
                void (mode === "export" ? handleExport() : handleImport());
              }}
              disabled={loading}
              className="flex items-center gap-1.5 px-4 py-2 text-sm font-medium rounded bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white"
            >
              {loading ? (
                mode === "export" ? "Exporting..." : "Importing..."
              ) : mode === "export" ? (
                <>
                  <FolderOpen size={16} />
                  {IS_TAURI ? "Save as…" : "Download"}
                </>
              ) : (
                <>
                  <Upload size={16} />
                  {IS_TAURI ? "Choose file…" : "Upload file"}
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
