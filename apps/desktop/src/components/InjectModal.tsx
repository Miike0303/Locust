import { useState } from "react";
import { X, FolderOpen, FileCheck, AlertCircle } from "lucide-react";
import { inject, type OutputMode } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const LANGUAGES: { code: string; name: string }[] = [
  { code: "es", name: "Español" },
  { code: "en", name: "English" },
  { code: "ja", name: "日本語" },
  { code: "zh-CN", name: "简体中文" },
  { code: "zh-TW", name: "繁體中文" },
  { code: "ko", name: "한국어" },
  { code: "fr", name: "Français" },
  { code: "de", name: "Deutsch" },
  { code: "it", name: "Italiano" },
  { code: "pt", name: "Português" },
  { code: "pt-BR", name: "Português BR" },
  { code: "ru", name: "Русский" },
  { code: "nl", name: "Nederlands" },
  { code: "pl", name: "Polski" },
  { code: "tr", name: "Türkçe" },
];

const INJECT_LANG_KEY = "locust.inject.langs";

interface InjectModalProps {
  open: boolean;
  onClose: () => void;
}

export default function InjectModal({ open, onClose }: InjectModalProps) {
  const { project } = useProjectStore();
  const [mode, setMode] = useState<OutputMode>("add");
  const savedLangs = (() => {
    try { return JSON.parse(localStorage.getItem(INJECT_LANG_KEY) || "null") as string[] | null; } catch { return null; }
  })();
  const [selectedLangs, setSelectedLangs] = useState<string[]>(savedLangs ?? ["es"]);
  const [outputDir, setOutputDir] = useState("");
  const [loading, setLoading] = useState(false);
  const [result, setResult] = useState<any>(null);

  const toggleLang = (code: string) => {
    setSelectedLangs((prev) =>
      prev.includes(code) ? prev.filter((l) => l !== code) : [...prev, code]
    );
  };

  if (!open || !project) return null;

  const handlePickFolder = async () => {
    if (IS_TAURI) {
      const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
      const selected = await openDialog({
        title: "Select output folder for injected game copies",
        directory: true,
      });
      if (typeof selected === "string") setOutputDir(selected);
    } else {
      const path = prompt("Enter output folder path:");
      if (path) setOutputDir(path);
    }
  };

  const canInject = selectedLangs.length > 0 && (mode === "add" || (mode === "replace" && outputDir.trim() !== ""));

  const handleInject = async () => {
    if (mode === "replace" && !outputDir.trim()) {
      addToast("error", "Select an output folder for Replace mode");
      return;
    }
    if (selectedLangs.length === 0) {
      addToast("error", "Select at least one language");
      return;
    }
    // Persist language selection
    try { localStorage.setItem(INJECT_LANG_KEY, JSON.stringify(selectedLangs)); } catch {}

    setLoading(true);
    setResult(null);
    try {
      const report = await inject({
        project_path: project.path,
        format_id: project.format_id,
        mode,
        languages: selectedLangs,
        output_dir: outputDir.trim() || undefined,
      });
      setResult(report);

      const destInfo = mode === "replace"
        ? `Output: ${outputDir}`
        : `Added translation folders in ${project.path}`;

      addLog(
        "info",
        `Inject complete: ${report.languages_processed.join(", ")} (${mode} mode)`,
        `${destInfo}\n${report.languages_failed.length > 0
          ? `Failed: ${report.languages_failed.map(([l, e]: [string, string]) => `${l}: ${e}`).join(", ")}`
          : "All languages succeeded"}`,
        "inject"
      );
      addToast("success", `Injected ${report.languages_processed.length} language(s)`);
    } catch (err: any) {
      addLog("error", "Inject failed", err.message, "inject");
      addToast("error", `Inject failed: ${err.message}`);
    } finally {
      setLoading(false);
    }
  };

  const gameName = project.path.split(/[\\/]/).filter(Boolean).pop() ?? project.name;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-lg p-6">
        <div className="flex justify-between items-center mb-4">
          <h2 className="text-lg font-bold">Inject Translations</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600"><X size={20} /></button>
        </div>

        {!result ? (
          <div className="space-y-4">
            <div>
              <label className="text-sm font-medium">Mode</label>
              <select value={mode} onChange={(e) => setMode(e.target.value as OutputMode)}
                className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm">
                <option value="replace">Replace — copy game to output folder with translations</option>
                <option value="add">Add — create translation folders inside original game</option>
              </select>
              <p className="text-xs text-gray-500 mt-1">
                {mode === "replace"
                  ? `Creates a translated copy at: [output]/${gameName}-[lang]/`
                  : "Adds a tl/[lang]/ folder inside the original game directory"}
              </p>
            </div>

            <div>
              <label className="text-sm font-medium">Languages</label>
              <div className="mt-1 grid grid-cols-3 gap-2 p-2 border rounded dark:border-gray-600 max-h-40 overflow-y-auto">
                {LANGUAGES.map((l) => (
                  <label key={l.code} className="flex items-center gap-1 text-sm cursor-pointer">
                    <input
                      type="checkbox"
                      checked={selectedLangs.includes(l.code)}
                      onChange={() => toggleLang(l.code)}
                    />
                    <span>{l.name}</span>
                  </label>
                ))}
              </div>
              <p className="text-xs text-gray-500 mt-1">
                Selected: {selectedLangs.length === 0 ? "none" : selectedLangs.join(", ")}
              </p>
            </div>

            {mode === "replace" && (
              <div>
                <label className="text-sm font-medium">
                  Output folder <span className="text-red-500">*</span>
                </label>
                <div className="flex gap-2 mt-1">
                  <input
                    value={outputDir}
                    onChange={(e) => setOutputDir(e.target.value)}
                    placeholder="Choose where to save translated copies..."
                    className="flex-1 p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm"
                  />
                  <button
                    onClick={handlePickFolder}
                    className="px-3 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm transition-colors"
                    title="Browse folders"
                  >
                    <FolderOpen size={16} />
                  </button>
                </div>
                {!outputDir.trim() && (
                  <p className="flex items-center gap-1 text-xs text-amber-500 mt-1">
                    <AlertCircle size={12} />
                    Required — select a folder to save the translated game copy
                  </p>
                )}
                {outputDir.trim() && (
                  <p className="text-xs text-gray-500 mt-1">
                    Will create: {outputDir}/{gameName}-{selectedLangs[0] || "lang"}/
                  </p>
                )}
              </div>
            )}

            <div className="pt-2 flex items-center gap-3 text-xs text-gray-500">
              <FileCheck size={14} />
              <span>Source: <strong>{project.name}</strong> ({project.format_id})</span>
            </div>

            <button
              onClick={handleInject}
              disabled={loading || !canInject}
              className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded font-medium transition-colors"
            >
              {loading ? "Injecting..." : "Inject Translations"}
            </button>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="p-3 bg-emerald-50 dark:bg-emerald-900/20 border border-emerald-200 dark:border-emerald-800 rounded text-sm">
              <p className="font-medium text-emerald-700 dark:text-emerald-300">Injection complete</p>
              <p className="text-emerald-600 dark:text-emerald-400 mt-1">
                Languages: {result.languages_processed.join(", ")}
              </p>
              <p className="text-emerald-600 dark:text-emerald-400">Mode: {result.mode}</p>
              {mode === "replace" && outputDir && (
                <p className="text-emerald-600 dark:text-emerald-400">
                  Output: {outputDir}/
                </p>
              )}
              {result.reports && Object.keys(result.reports).length > 0 && (
                <div className="mt-2 space-y-1">
                  {Object.entries(result.reports).map(([lang, report]: [string, any]) => (
                    <p key={lang} className="text-xs text-emerald-500">
                      {lang}: {report.strings_written ?? 0} strings written, {report.files_modified ?? 0} files modified
                    </p>
                  ))}
                </div>
              )}
            </div>

            {result.languages_failed?.length > 0 && (
              <div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 rounded text-sm">
                <p className="font-medium text-red-700">Failed languages:</p>
                {result.languages_failed.map(([lang, err]: [string, string]) => (
                  <p key={lang} className="text-red-600 text-xs mt-1">{lang}: {err}</p>
                ))}
              </div>
            )}

            <button onClick={onClose}
              className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium">
              Close
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
