import { useState, useEffect } from "react";
import { Search, X } from "lucide-react";
import clsx from "clsx";
import { useEditorStore } from "../stores/editorStore";
import type { StringStatus } from "../lib/api";

const STATUSES: { label: string; value: StringStatus | undefined }[] = [
  { label: "All", value: undefined },
  { label: "Pending", value: "pending" },
  { label: "Translated", value: "translated" },
  { label: "Reviewed", value: "reviewed" },
  { label: "Approved", value: "approved" },
  { label: "Error", value: "error" },
];

const statusColors: Record<string, string> = {
  pending: "bg-gray-200 text-gray-700 dark:bg-gray-700 dark:text-gray-300",
  translated: "bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300",
  reviewed: "bg-amber-100 text-amber-700 dark:bg-amber-900 dark:text-amber-300",
  approved: "bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300",
  error: "bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300",
};

interface FilterBarProps {
  total: number;
  showing: number;
}

export default function FilterBar({ total, showing }: FilterBarProps) {
  const { filter, setFilter } = useEditorStore();
  const [searchInput, setSearchInput] = useState(filter.search || "");

  useEffect(() => {
    const timer = setTimeout(() => {
      setFilter({ search: searchInput || undefined, offset: 0 });
    }, 300);
    return () => clearTimeout(timer);
  }, [searchInput, setFilter]);

  const hasFilters = filter.status || filter.search || filter.file_path || filter.tag;

  return (
    <div className="flex items-center gap-3 p-3 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
      <div className="flex gap-1">
        {STATUSES.map(({ label, value }) => (
          <button
            key={label}
            onClick={() => setFilter({ status: value, offset: 0 })}
            className={clsx(
              "px-3 py-1 rounded-full text-xs font-medium transition-colors",
              filter.status === value
                ? value
                  ? statusColors[value]
                  : "bg-emerald-100 text-emerald-700 dark:bg-emerald-900 dark:text-emerald-300"
                : "bg-gray-100 text-gray-600 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-400 dark:hover:bg-gray-700"
            )}
          >
            {label}
          </button>
        ))}
      </div>

      <div className="flex-1 relative max-w-md">
        <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
        <input
          type="text"
          value={searchInput}
          onChange={(e) => setSearchInput(e.target.value)}
          placeholder="Search strings..."
          className="w-full pl-9 pr-3 py-1.5 text-sm border border-gray-300 dark:border-gray-600 rounded-md bg-white dark:bg-gray-800 focus:outline-none focus:ring-2 focus:ring-emerald-500"
        />
      </div>

      {hasFilters && (
        <button
          onClick={() => {
            setFilter({ status: undefined, search: undefined, file_path: undefined, tag: undefined, offset: 0 });
            setSearchInput("");
          }}
          className="flex items-center gap-1 px-2 py-1 text-xs text-gray-500 hover:text-gray-700"
        >
          <X size={14} /> Clear
        </button>
      )}

      <span className="text-xs text-gray-500 ml-auto">
        Showing {showing} of {total}
      </span>
    </div>
  );
}
