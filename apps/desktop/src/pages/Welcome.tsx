import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  FolderOpen, File, Globe, Swords, Heart, Box, Shield, Code,
  Clock, X, Plus, Wand2,
} from "lucide-react";
import { getFormats, getConfig, openProject } from "../lib/api";
import { useProjectStore } from "../stores/projectStore";
import { useQueueStore } from "../stores/queueStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const FORMAT_ICONS: Record<string, typeof Globe> = {
  "rpgmaker-mv": Swords,
  "rpgmaker-vxa": Swords,
  renpy: Heart,
  unity: Box,
  "wolf-rpg": Shield,
  sugarcube: Globe,
  "html-game": Code,
  unreal: Box,
};

const FORMAT_COLORS: Record<string, string> = {
  "rpgmaker-mv": "bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300",
  "rpgmaker-vxa": "bg-indigo-100 text-indigo-700 dark:bg-indigo-900/40 dark:text-indigo-300",
  renpy: "bg-pink-100 text-pink-700 dark:bg-pink-900/40 dark:text-pink-300",
  unity: "bg-purple-100 text-purple-700 dark:bg-purple-900/40 dark:text-purple-300",
  "wolf-rpg": "bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-300",
  sugarcube: "bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300",
  "html-game": "bg-cyan-100 text-cyan-700 dark:bg-cyan-900/40 dark:text-cyan-300",
  unreal: "bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300",
};

export default function Welcome() {
  const navigate = useNavigate();
  const setProject = useProjectStore((s) => s.setProject);
  const { data: formats } = useQuery({ queryKey: ["formats"], queryFn: getFormats });
  const { data: config } = useQuery({ queryKey: ["config"], queryFn: getConfig });

  const addToQueue = useQueueStore((s) => s.addItem);
  const setQueueOpen = useQueueStore((s) => s.setPanelOpen);

  // Format picker state
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [selectedFormat, setSelectedFormat] = useState("auto");
  const [opening, setOpening] = useState(false);

  const openWithPath = async (path: string, formatId?: string) => {
    setOpening(true);
    try {
      const result = await openProject(path, formatId);
      setProject({
        path: result.project_path,
        format_id: result.format_id,
        name: result.project_name,
      });
      addLog("info", `Opened: ${result.project_name} (${result.format_name}, ${result.total_strings} strings)`, undefined, "project");
      setPendingPath(null);
      navigate("/editor");
    } catch (err: any) {
      addLog("error", `Failed to open project`, err?.message ?? String(err), "project");
      addToast("error", `Failed to open: ${err}`);
    } finally {
      setOpening(false);
    }
  };

  const showFormatPicker = (path: string) => {
    setPendingPath(path);
    setSelectedFormat("auto");
  };

  const handleConfirmFormat = () => {
    if (!pendingPath) return;
    openWithPath(pendingPath, selectedFormat === "auto" ? undefined : selectedFormat);
  };

  const handleAddToQueue = (path: string) => {
    addToQueue(path);
    setQueueOpen(true);
    addToast("info", "Added to queue");
  };

  const handleOpenFile = async () => {
    let path: string | null = null;
    if (IS_TAURI) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        title: "Select game executable or main file",
        filters: [
          {
            name: "Game files",
            extensions: ["exe", "html", "htm", "rpy", "rpa", "rpgproject", "rvproj2"],
          },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (typeof selected === "string") path = selected;
    } else {
      path = prompt("Enter game executable or file path:");
    }
    if (path) showFormatPicker(path);
  };

  const handleOpenFolder = async () => {
    let path: string | null = null;
    if (IS_TAURI) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        title: "Select game folder",
        directory: true,
      });
      if (typeof selected === "string") path = selected;
    } else {
      path = prompt("Enter game folder path:");
    }
    if (path) showFormatPicker(path);
  };

  const recentProjects = config?.recent_projects ?? [];

  return (
    <div className="flex flex-col min-h-full p-8 max-w-4xl mx-auto">
      {/* Hero */}
      <div className="text-center mb-8">
        <Globe size={48} className="mx-auto mb-3 text-emerald-500" />
        <h1 className="text-3xl font-bold mb-1">Project Locust</h1>
        <p className="text-gray-500">Universal game translation tool</p>
      </div>

      {/* Open buttons */}
      <div className="flex justify-center gap-4 mb-10">
        <button
          onClick={handleOpenFile}
          className="flex items-center gap-2 px-6 py-3 bg-emerald-600 hover:bg-emerald-700 text-white rounded-lg text-sm font-medium transition-colors"
        >
          <File size={18} />
          Open Game File
        </button>
        <button
          onClick={handleOpenFolder}
          className="flex items-center gap-2 px-6 py-3 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded-lg text-sm font-medium transition-colors"
        >
          <FolderOpen size={18} />
          Open Game Folder
        </button>
      </div>

      {/* Format Picker Modal */}
      {pendingPath && (
        <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
          <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-md p-6">
            <div className="flex justify-between items-center mb-4">
              <h2 className="text-lg font-bold">Select Format</h2>
              <button onClick={() => setPendingPath(null)} className="text-gray-400 hover:text-gray-600">
                <X size={20} />
              </button>
            </div>

            <p className="text-sm text-gray-500 mb-1 truncate">{pendingPath}</p>
            <p className="text-xs text-gray-400 mb-4">Choose the game engine format, or let Locust detect it automatically.</p>

            <div className="space-y-1.5 max-h-64 overflow-y-auto mb-4">
              <button
                onClick={() => setSelectedFormat("auto")}
                className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
                  selectedFormat === "auto"
                    ? "border-emerald-500 bg-emerald-50 dark:bg-emerald-900/20"
                    : "border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800"
                }`}
              >
                <div className="p-1.5 rounded bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-300">
                  <Wand2 size={16} />
                </div>
                <div>
                  <div className="text-sm font-medium">Auto-detect</div>
                  <div className="text-xs text-gray-500">Let Locust detect the format automatically</div>
                </div>
              </button>

              {formats?.map((f) => {
                const Icon = FORMAT_ICONS[f.id] ?? Globe;
                const colorClass = FORMAT_COLORS[f.id] ?? "bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
                return (
                  <button
                    key={f.id}
                    onClick={() => setSelectedFormat(f.id)}
                    className={`w-full text-left p-3 rounded-lg border transition-colors flex items-center gap-3 ${
                      selectedFormat === f.id
                        ? "border-emerald-500 bg-emerald-50 dark:bg-emerald-900/20"
                        : "border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800"
                    }`}
                  >
                    <div className={`p-1.5 rounded ${colorClass}`}>
                      <Icon size={16} />
                    </div>
                    <div>
                      <div className="text-sm font-medium">{f.name}</div>
                      <div className="text-xs text-gray-500">{f.extensions.join(", ")}</div>
                    </div>
                  </button>
                );
              })}
            </div>

            <button
              onClick={handleConfirmFormat}
              disabled={opening}
              className="w-full py-2.5 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded-lg font-medium transition-colors"
            >
              {opening ? "Opening..." : "Open Project"}
            </button>
          </div>
        </div>
      )}

      {/* Recent Projects */}
      {recentProjects.length > 0 && (
        <div className="mb-10">
          <h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">
            Recent Projects
          </h2>
          <div className="space-y-2">
            {recentProjects.map((p, i) => {
              const Icon = FORMAT_ICONS[p.format_id] ?? Globe;
              const colorClass = FORMAT_COLORS[p.format_id] ?? "bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
              return (
                <div
                  key={i}
                  className="w-full p-3 rounded-lg border border-gray-200 dark:border-gray-700 hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors flex items-center gap-3"
                >
                  <button
                    onClick={() => { setPendingPath(p.path); setSelectedFormat(p.format_id); }}
                    className="flex items-center gap-3 flex-1 min-w-0 text-left"
                  >
                    <div className={`p-2 rounded-lg ${colorClass}`}>
                      <Icon size={18} />
                    </div>
                    <div className="flex-1 min-w-0">
                      <div className="font-medium truncate">{p.name}</div>
                      <div className="text-xs text-gray-500 truncate">{p.path}</div>
                    </div>
                  </button>
                  <div className="flex items-center gap-2 text-xs text-gray-400 shrink-0">
                    <span className={`px-2 py-0.5 rounded-full text-xs ${colorClass}`}>
                      {p.format_id}
                    </span>
                    {p.last_opened && (
                      <span className="flex items-center gap-1">
                        <Clock size={12} />
                        {new Date(p.last_opened).toLocaleDateString()}
                      </span>
                    )}
                    <button
                      onClick={(e) => { e.stopPropagation(); handleAddToQueue(p.path); }}
                      className="p-1 rounded hover:bg-gray-200 dark:hover:bg-gray-600 text-gray-400 hover:text-emerald-500"
                      title="Add to queue"
                    >
                      <Plus size={14} />
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Supported Formats Grid */}
      {formats && formats.length > 0 && (
        <div>
          <h2 className="text-sm font-semibold text-gray-500 uppercase mb-3">
            Supported Formats
          </h2>
          <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
            {formats.map((f) => {
              const Icon = FORMAT_ICONS[f.id] ?? Globe;
              const colorClass = FORMAT_COLORS[f.id] ?? "bg-gray-100 text-gray-700 dark:bg-gray-800 dark:text-gray-300";
              return (
                <div
                  key={f.id}
                  className="p-3 rounded-lg border border-gray-200 dark:border-gray-700"
                >
                  <div className="flex items-center gap-2 mb-1">
                    <div className={`p-1.5 rounded ${colorClass}`}>
                      <Icon size={14} />
                    </div>
                    <span className="text-sm font-medium">{f.name}</span>
                  </div>
                  {f.description && (
                    <p className="text-xs text-gray-500 line-clamp-2">
                      {f.description}
                    </p>
                  )}
                  <div className="mt-1.5 flex flex-wrap gap-1">
                    {f.extensions.slice(0, 3).map((ext) => (
                      <span
                        key={ext}
                        className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-800 rounded text-xs text-gray-500"
                      >
                        {ext}
                      </span>
                    ))}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Footer stats */}
      <div className="mt-auto pt-8 flex justify-center gap-6 text-xs text-gray-400">
        <span>{formats?.length ?? 0} formats supported</span>
        <span>{recentProjects.length} recent projects</span>
        <span>
          <a href="https://github.com/Miike0303/Locust" className="hover:underline">
            GitHub
          </a>
        </span>
      </div>
    </div>
  );
}
