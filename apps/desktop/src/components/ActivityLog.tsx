import { useState } from "react";
import { X, Trash2 } from "lucide-react";
import clsx from "clsx";
import { useLogStore, type LogLevel } from "../stores/logStore";

const levelColors: Record<LogLevel, string> = {
  info: "bg-blue-400",
  warning: "bg-amber-400",
  error: "bg-red-500",
};

const levelBg: Record<LogLevel, string> = {
  info: "",
  warning: "",
  error: "bg-red-900/10",
};

const filters: Array<LogLevel | "all"> = ["all", "info", "warning", "error"];

function timeAgo(ts: number): string {
  const sec = Math.floor((Date.now() - ts) / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  return `${hr}h ago`;
}

export default function ActivityLog() {
  const { entries, filter, isOpen, setOpen, setFilter, clear } = useLogStore();
  const [expandedId, setExpandedId] = useState<string | null>(null);

  if (!isOpen) return null;

  const filtered = filter === "all" ? entries : entries.filter((e) => e.level === filter);

  return (
    <div className="fixed inset-y-0 right-0 w-96 bg-white dark:bg-gray-900 border-l border-gray-200 dark:border-gray-700 z-50 flex flex-col shadow-xl">
      {/* Header */}
      <div className="flex items-center justify-between p-4 border-b border-gray-200 dark:border-gray-700">
        <h2 className="font-bold">Activity Log</h2>
        <div className="flex items-center gap-2">
          <button onClick={clear} className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300" title="Clear log">
            <Trash2 size={16} />
          </button>
          <button onClick={() => setOpen(false)} className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300">
            <X size={18} />
          </button>
        </div>
      </div>

      {/* Filter tabs */}
      <div className="flex gap-1 px-4 py-2 border-b border-gray-200 dark:border-gray-700">
        {filters.map((f) => (
          <button
            key={f}
            onClick={() => setFilter(f)}
            className={clsx(
              "px-3 py-1 rounded-full text-xs font-medium transition-colors capitalize",
              filter === f
                ? "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                : "text-gray-500 hover:bg-gray-100 dark:hover:bg-gray-800"
            )}
          >
            {f}
            {f !== "all" && (
              <span className="ml-1 opacity-60">
                ({entries.filter((e) => e.level === f).length})
              </span>
            )}
          </button>
        ))}
      </div>

      {/* Entries */}
      <div className="flex-1 overflow-y-auto">
        {filtered.length === 0 ? (
          <div className="p-8 text-center text-gray-500 text-sm">No log entries</div>
        ) : (
          filtered.map((entry) => (
            <div
              key={entry.id}
              className={clsx(
                "px-4 py-2.5 border-b border-gray-100 dark:border-gray-800 cursor-pointer hover:bg-gray-50 dark:hover:bg-gray-800/50",
                levelBg[entry.level]
              )}
              onClick={() => setExpandedId(expandedId === entry.id ? null : entry.id)}
            >
              <div className="flex items-start gap-2">
                <div className={clsx("w-2 h-2 rounded-full mt-1.5 shrink-0", levelColors[entry.level])} />
                <div className="flex-1 min-w-0">
                  <div className="text-sm leading-snug">{entry.message}</div>
                  <div className="flex gap-2 mt-0.5 text-xs text-gray-500">
                    <span>{timeAgo(entry.timestamp)}</span>
                    {entry.source && <span className="text-gray-400">· {entry.source}</span>}
                  </div>
                </div>
              </div>
              {expandedId === entry.id && entry.detail && (
                <pre className="mt-2 ml-4 p-2 bg-gray-100 dark:bg-gray-800 rounded text-xs overflow-x-auto whitespace-pre-wrap">
                  {entry.detail}
                </pre>
              )}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
