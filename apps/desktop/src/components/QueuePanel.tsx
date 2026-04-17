import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  X, Plus, ChevronUp, ChevronDown, Trash2, Play, Square,
  CheckCircle, AlertCircle, Loader2, Clock, FolderOpen, File,
} from "lucide-react";
import clsx from "clsx";
import { useQueueStore, type QueueItem } from "../stores/queueStore";
import { getProviders, type TranslationStartParams } from "../lib/api";

const IS_TAURI = "__TAURI_INTERNALS__" in window;

const statusIcons: Record<string, typeof Clock> = {
  pending: Clock,
  extracting: Loader2,
  translating: Loader2,
  done: CheckCircle,
  error: AlertCircle,
  cancelled: Square,
};

const statusColors: Record<string, string> = {
  pending: "text-gray-400",
  extracting: "text-blue-400 animate-spin",
  translating: "text-emerald-400 animate-spin",
  done: "text-emerald-500",
  error: "text-red-500",
  cancelled: "text-gray-500",
};

export default function QueuePanel() {
  const { items, isRunning, isPanelOpen, translationParams, setPanelOpen, addItem, removeItem, moveItem, clearCompleted, setParams, startQueue, cancelQueue } = useQueueStore();
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders, enabled: isPanelOpen });

  const [providerId, setProviderId] = useState(translationParams?.provider_id ?? "mock");
  const [sourceLang, setSourceLang] = useState(translationParams?.options.source_lang ?? "ja");
  const [targetLang, setTargetLang] = useState(translationParams?.options.target_lang ?? "en");
  const [batchSize, setBatchSize] = useState(translationParams?.options.batch_size ?? 40);
  const [gameContext, setGameContext] = useState(translationParams?.options.game_context ?? "");

  if (!isPanelOpen) return null;

  const handleAddFile = async () => {
    if (IS_TAURI) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        title: "Select game file(s) to queue",
        multiple: true,
        filters: [
          { name: "Game files", extensions: ["exe", "html", "htm", "rpy", "rpa", "rpgproject", "rvproj2"] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (Array.isArray(selected)) selected.forEach((p) => addItem(p));
      else if (typeof selected === "string") addItem(selected);
    } else {
      const path = prompt("Enter game file path:");
      if (path) addItem(path);
    }
  };

  const handleAddFolder = async () => {
    if (IS_TAURI) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ title: "Select game folder", directory: true });
      if (typeof selected === "string") addItem(selected);
    } else {
      const path = prompt("Enter game folder path:");
      if (path) addItem(path);
    }
  };

  const buildParams = (): TranslationStartParams => ({
    provider_id: providerId,
    options: {
      source_lang: sourceLang,
      target_lang: targetLang,
      batch_size: batchSize,
      max_concurrent: 3,
      cost_limit_usd: null,
      game_context: gameContext || null,
      use_glossary: true,
      use_memory: true,
      skip_approved: true,
    },
  });

  const handleStart = () => {
    const params = buildParams();
    setParams(params);
    startQueue();
  };

  const pendingCount = items.filter((i) => i.status === "pending").length;
  const doneCount = items.filter((i) => i.status === "done").length;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-2xl max-h-[80vh] flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
          <h2 className="font-bold text-lg">Project Queue</h2>
          <div className="flex items-center gap-2">
            {doneCount > 0 && (
              <button onClick={clearCompleted} className="text-xs text-gray-400 hover:text-gray-600">
                Clear completed
              </button>
            )}
            <button onClick={() => setPanelOpen(false)} className="text-gray-400 hover:text-gray-600">
              <X size={20} />
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto">
          {/* Queue list */}
          {items.length === 0 ? (
            <div className="p-8 text-center text-gray-500">
              <p className="mb-2">No projects in queue</p>
              <p className="text-xs">Add game files or folders to translate in batch</p>
            </div>
          ) : (
            <div className="divide-y divide-gray-100 dark:divide-gray-800">
              {items.map((item, idx) => (
                <QueueItemRow
                  key={item.id}
                  item={item}
                  index={idx}
                  total={items.length}
                  onRemove={() => removeItem(item.id)}
                  onMoveUp={() => moveItem(item.id, "up")}
                  onMoveDown={() => moveItem(item.id, "down")}
                  disabled={isRunning}
                />
              ))}
            </div>
          )}

          {/* Add buttons */}
          {!isRunning && (
            <div className="flex gap-2 p-4">
              <button
                onClick={handleAddFile}
                className="flex items-center gap-2 px-3 py-2 text-sm border border-dashed border-gray-300 dark:border-gray-600 rounded-lg hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors"
              >
                <File size={14} />
                Add File
              </button>
              <button
                onClick={handleAddFolder}
                className="flex items-center gap-2 px-3 py-2 text-sm border border-dashed border-gray-300 dark:border-gray-600 rounded-lg hover:bg-gray-50 dark:hover:bg-gray-800 transition-colors"
              >
                <FolderOpen size={14} />
                Add Folder
              </button>
            </div>
          )}

          {/* Translation settings */}
          {!isRunning && items.length > 0 && (
            <div className="p-4 border-t border-gray-200 dark:border-gray-700 space-y-3">
              <h3 className="text-sm font-semibold text-gray-500 uppercase">Translation Settings</h3>
              <div className="grid grid-cols-3 gap-3">
                <div>
                  <label className="text-xs font-medium text-gray-500">Provider</label>
                  <select value={providerId} onChange={(e) => setProviderId(e.target.value)}
                    className="mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-800 dark:border-gray-600">
                    {providers?.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
                  </select>
                </div>
                <div>
                  <label className="text-xs font-medium text-gray-500">Source</label>
                  <input value={sourceLang} onChange={(e) => setSourceLang(e.target.value)}
                    className="mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
                </div>
                <div>
                  <label className="text-xs font-medium text-gray-500">Target</label>
                  <input value={targetLang} onChange={(e) => setTargetLang(e.target.value)}
                    className="mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
                </div>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div>
                  <label className="text-xs font-medium text-gray-500">Batch size</label>
                  <input type="number" value={batchSize} onChange={(e) => setBatchSize(+e.target.value)} min={1} max={100}
                    className="mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
                </div>
                <div>
                  <label className="text-xs font-medium text-gray-500">Game context</label>
                  <input value={gameContext} onChange={(e) => setGameContext(e.target.value)} placeholder="Optional"
                    className="mt-1 w-full p-1.5 border rounded text-sm dark:bg-gray-800 dark:border-gray-600" />
                </div>
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="p-4 border-t border-gray-200 dark:border-gray-700 flex items-center justify-between">
          <span className="text-sm text-gray-500">
            {pendingCount} pending · {doneCount} done
          </span>
          {isRunning ? (
            <button
              onClick={cancelQueue}
              className="flex items-center gap-2 px-4 py-2 bg-red-600 hover:bg-red-700 text-white rounded-lg text-sm font-medium transition-colors"
            >
              <Square size={14} />
              Cancel Queue
            </button>
          ) : (
            <button
              onClick={handleStart}
              disabled={pendingCount === 0}
              className="flex items-center gap-2 px-4 py-2 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 disabled:cursor-not-allowed text-white rounded-lg text-sm font-medium transition-colors"
            >
              <Play size={14} />
              Start Queue ({pendingCount})
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function QueueItemRow({ item, index, total, onRemove, onMoveUp, onMoveDown, disabled }: {
  item: QueueItem; index: number; total: number;
  onRemove: () => void; onMoveUp: () => void; onMoveDown: () => void;
  disabled: boolean;
}) {
  const Icon = statusIcons[item.status] ?? Clock;
  const percent = item.progress.total > 0
    ? Math.round((item.progress.completed / item.progress.total) * 100)
    : 0;

  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <Icon size={16} className={statusColors[item.status]} />
      <div className="flex-1 min-w-0">
        <div className="text-sm font-medium truncate">{item.projectName}</div>
        <div className="text-xs text-gray-500 truncate">{item.projectPath}</div>
        {item.status === "translating" && item.progress.total > 0 && (
          <div className="mt-1 flex items-center gap-2">
            <div className="flex-1 h-1.5 bg-gray-200 dark:bg-gray-700 rounded-full overflow-hidden">
              <div className="h-full bg-emerald-500 rounded-full transition-all" style={{ width: `${percent}%` }} />
            </div>
            <span className="text-xs text-gray-400 tabular-nums">{percent}%</span>
          </div>
        )}
        {item.error && <div className="text-xs text-red-500 mt-0.5 truncate">{item.error}</div>}
      </div>
      {!disabled && item.status === "pending" && (
        <div className="flex items-center gap-1 shrink-0">
          <button onClick={onMoveUp} disabled={index === 0} className="p-1 text-gray-400 hover:text-gray-600 disabled:opacity-30">
            <ChevronUp size={14} />
          </button>
          <button onClick={onMoveDown} disabled={index === total - 1} className="p-1 text-gray-400 hover:text-gray-600 disabled:opacity-30">
            <ChevronDown size={14} />
          </button>
          <button onClick={onRemove} className="p-1 text-gray-400 hover:text-red-500">
            <Trash2 size={14} />
          </button>
        </div>
      )}
    </div>
  );
}
