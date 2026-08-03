import { useState } from "react";
import { X, Download, FolderOpen } from "lucide-react";
import { exportTranslations, type ExportFormat } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const LANGUAGES: { code: string; name: string }[] = [
  { code: "es", name: "Español" },
  { code: "en", name: "English" },
  { code: "ja", name: "日本語" },
  { code: "zh-CN", name: "简体中文" },
  { code: "ko", name: "한국어" },
  { code: "fr", name: "Français" },
  { code: "de", name: "Deutsch" },
  { code: "pt-BR", name: "Português BR" },
  { code: "ru", name: "Русский" },
];

interface ExportModalProps {
  open: boolean;
  onClose: () => void;
}

export default function ExportModal({ open, onClose }: ExportModalProps) {
  const { project } = useProjectStore();
  const [format, setFormat] = useState<ExportFormat>("po");
  const [lang, setLang] = useState("es");
  const [loading, setLoading] = useState(false);

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

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-md p-6">
        <div className="flex justify-between items-center mb-4">
          <h2 className="text-lg font-bold flex items-center gap-2">
            <Download size={18} /> Export Translations
          </h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X size={20} />
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
            <p className="text-xs text-gray-500 mt-1">
              For external CAT tools or handoff. Re-import via CLI when needed.
            </p>
          </div>

          <div>
            <label className="text-sm font-medium">Target language</label>
            <select
              value={lang}
              onChange={(e) => setLang(e.target.value)}
              className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
            >
              {LANGUAGES.map((l) => (
                <option key={l.code} value={l.code}>
                  {l.name} ({l.code})
                </option>
              ))}
            </select>
          </div>

          <div className="flex justify-end gap-2 pt-2">
            <button
              onClick={onClose}
              className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
            >
              Cancel
            </button>
            <button
              onClick={() => {
                void handleExport();
              }}
              disabled={loading}
              className="flex items-center gap-1.5 px-4 py-2 text-sm font-medium rounded bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white"
            >
              {loading ? (
                "Exporting..."
              ) : (
                <>
                  <FolderOpen size={16} />
                  {IS_TAURI ? "Save as…" : "Download"}
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
