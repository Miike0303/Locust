import { useState, useEffect } from "react";
import { AlertTriangle, ChevronDown, ChevronRight } from "lucide-react";
import clsx from "clsx";
import type { StringEntry, StringStatus } from "../lib/api";
import { patchString } from "../lib/api";

const statusButtons: { value: StringStatus; label: string; color: string }[] = [
  { value: "pending", label: "Pending", color: "bg-gray-200 text-gray-700" },
  { value: "reviewed", label: "Reviewed", color: "bg-amber-100 text-amber-700" },
  { value: "approved", label: "Approved", color: "bg-green-100 text-green-700" },
];

interface DetailPanelProps {
  entry: StringEntry;
  onRefetch: () => void;
  onClose: () => void;
}

export default function DetailPanel({ entry, onRefetch, onClose }: DetailPanelProps) {
  const [translation, setTranslation] = useState(entry.translation || "");
  const [showMeta, setShowMeta] = useState(false);

  useEffect(() => {
    setTranslation(entry.translation || "");
  }, [entry.id, entry.translation]);

  const handleSave = async () => {
    if (translation !== (entry.translation || "")) {
      await patchString(entry.id, { translation } as any);
      onRefetch();
    }
  };

  const handleStatusChange = async (status: StringStatus) => {
    await patchString(entry.id, { status } as any);
    onRefetch();
  };

  const charCount = translation.length;
  const limitExceeded = entry.char_limit != null && charCount > entry.char_limit;

  return (
    <aside className="w-[360px] border-l border-gray-200 dark:border-gray-700 overflow-y-auto bg-white dark:bg-gray-900 flex flex-col">
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 flex justify-between items-center">
        <h3 className="text-sm font-semibold">Entry Detail</h3>
        <button onClick={onClose} className="text-gray-400 hover:text-gray-600 text-lg">&times;</button>
      </div>

      <div className="p-4 space-y-4 flex-1">
        {/* Source */}
        <div>
          <label className="text-xs font-semibold text-gray-500 uppercase">Source</label>
          <div className="mt-1 p-2 bg-gray-50 dark:bg-gray-800 rounded font-mono text-sm select-all whitespace-pre-wrap">
            {entry.source}
          </div>
        </div>

        {/* Translation */}
        <div>
          <label className="text-xs font-semibold text-gray-500 uppercase">Translation</label>
          <textarea
            value={translation}
            onChange={(e) => setTranslation(e.target.value)}
            onBlur={handleSave}
            onKeyDown={(e) => { if (e.key === "Enter" && e.ctrlKey) handleSave(); }}
            className="mt-1 w-full p-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 text-sm focus:outline-none focus:ring-2 focus:ring-emerald-500 resize-y min-h-[80px]"
            rows={4}
          />
          <div className={clsx("text-xs mt-1", limitExceeded ? "text-red-500 font-semibold" : "text-gray-400")}>
            {charCount} chars
            {entry.char_limit != null && ` / ${entry.char_limit} limit`}
          </div>
        </div>

        {/* Status */}
        <div>
          <label className="text-xs font-semibold text-gray-500 uppercase">Status</label>
          <div className="flex gap-2 mt-1">
            {statusButtons.map(({ value, label, color }) => (
              <button
                key={value}
                onClick={() => handleStatusChange(value)}
                className={clsx(
                  "px-3 py-1 rounded-full text-xs font-medium transition-colors",
                  entry.status === value ? color : "bg-gray-100 text-gray-500 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
                )}
              >
                {label}
              </button>
            ))}
          </div>
        </div>

        {/* Validation warnings */}
        {limitExceeded && (
          <div className="flex items-center gap-2 p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded text-sm text-red-700 dark:text-red-400">
            <AlertTriangle size={16} />
            Translation exceeds character limit
          </div>
        )}

        {/* Metadata */}
        <div>
          <button
            onClick={() => setShowMeta(!showMeta)}
            className="flex items-center gap-1 text-xs font-semibold text-gray-500 uppercase hover:text-gray-700"
          >
            {showMeta ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            Metadata
          </button>
          {showMeta && (
            <dl className="mt-2 space-y-1 text-xs">
              {entry.context && <><dt className="text-gray-500">Context</dt><dd>{entry.context}</dd></>}
              <dt className="text-gray-500">File</dt><dd className="break-all">{entry.file_path}</dd>
              <dt className="text-gray-500">Entry ID</dt><dd className="font-mono break-all">{entry.id}</dd>
              {entry.tags.length > 0 && (
                <>
                  <dt className="text-gray-500">Tags</dt>
                  <dd className="flex gap-1 flex-wrap">
                    {entry.tags.map((t) => (
                      <span key={t} className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded">{t}</span>
                    ))}
                  </dd>
                </>
              )}
              {entry.provider_used && <><dt className="text-gray-500">Provider</dt><dd>{entry.provider_used}</dd></>}
              <dt className="text-gray-500">Created</dt><dd>{new Date(entry.created_at).toLocaleString()}</dd>
              {entry.translated_at && <><dt className="text-gray-500">Translated</dt><dd>{new Date(entry.translated_at).toLocaleString()}</dd></>}
            </dl>
          )}
        </div>
      </div>
    </aside>
  );
}
