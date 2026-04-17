import { useState, useCallback } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Search, Trash2, Database, ChevronLeft, ChevronRight } from "lucide-react";
import { getTranslationMemory, getTranslationMemoryStats, getTranslationMemoryLangPairs, deleteTranslationMemoryEntry, clearTranslationMemory } from "../lib/api";

const PAGE_SIZE = 50;

export default function TranslationMemory() {
  const qc = useQueryClient();
  const [search, setSearch] = useState("");
  const [langPair, setLangPair] = useState<string | undefined>();
  const [offset, setOffset] = useState(0);
  const [searchInput, setSearchInput] = useState("");

  const { data: stats } = useQuery({
    queryKey: ["tm-stats"],
    queryFn: getTranslationMemoryStats,
  });

  const { data: langPairs } = useQuery({
    queryKey: ["tm-lang-pairs"],
    queryFn: getTranslationMemoryLangPairs,
  });

  const { data, refetch } = useQuery({
    queryKey: ["tm-entries", search, langPair, offset],
    queryFn: () => getTranslationMemory({ search: search || undefined, lang_pair: langPair, limit: PAGE_SIZE, offset }),
    staleTime: 10_000,
  });

  const entries = data?.entries ?? [];
  const total = data?.total ?? 0;
  const totalPages = Math.ceil(total / PAGE_SIZE);
  const currentPage = Math.floor(offset / PAGE_SIZE) + 1;

  const handleSearch = useCallback(() => {
    setSearch(searchInput);
    setOffset(0);
  }, [searchInput]);

  const handleDelete = async (hash: string, lp: string) => {
    if (!confirm("Delete this memory entry?")) return;
    await deleteTranslationMemoryEntry(hash, lp);
    refetch();
    qc.invalidateQueries({ queryKey: ["tm-stats"] });
  };

  const handleClearAll = async () => {
    if (!confirm("Clear ALL translation memory entries? This cannot be undone.")) return;
    await clearTranslationMemory();
    refetch();
    qc.invalidateQueries({ queryKey: ["tm-stats"] });
    qc.invalidateQueries({ queryKey: ["tm-lang-pairs"] });
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="px-6 py-4 border-b border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
        <div className="flex items-center justify-between mb-3">
          <div className="flex items-center gap-3">
            <Database size={20} className="text-emerald-600" />
            <h1 className="text-xl font-bold">Translation Memory</h1>
          </div>
          <button
            onClick={handleClearAll}
            className="flex items-center gap-1.5 px-3 py-1.5 bg-red-50 hover:bg-red-100 text-red-600 dark:bg-red-900/30 dark:hover:bg-red-900/50 dark:text-red-400 rounded text-sm font-medium"
          >
            <Trash2 size={14} /> Clear All
          </button>
        </div>

        {/* Stats */}
        <div className="flex gap-6 text-sm text-gray-500 mb-4">
          <span><strong className="text-gray-800 dark:text-gray-200">{stats?.global_entries ?? 0}</strong> global entries</span>
          <span><strong className="text-gray-800 dark:text-gray-200">{stats?.project_entries ?? 0}</strong> project entries</span>
          <span><strong className="text-gray-800 dark:text-gray-200">{langPairs?.length ?? 0}</strong> language pairs</span>
        </div>

        {/* Search & Filter */}
        <div className="flex gap-3">
          <div className="flex-1 relative">
            <Search size={16} className="absolute left-3 top-1/2 -translate-y-1/2 text-gray-400" />
            <input
              value={searchInput}
              onChange={(e) => setSearchInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleSearch()}
              placeholder="Search source or translation text..."
              className="w-full pl-9 pr-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 text-sm focus:outline-none focus:ring-2 focus:ring-emerald-500"
            />
          </div>
          <select
            value={langPair ?? ""}
            onChange={(e) => { setLangPair(e.target.value || undefined); setOffset(0); }}
            className="px-3 py-2 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 text-sm"
          >
            <option value="">All languages</option>
            {langPairs?.map((lp) => (
              <option key={lp} value={lp}>{lp}</option>
            ))}
          </select>
          <button
            onClick={handleSearch}
            className="px-4 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium"
          >
            Search
          </button>
        </div>
      </div>

      {/* Table */}
      <div className="flex-1 overflow-auto">
        <table className="w-full text-sm">
          <thead className="sticky top-0 bg-gray-50 dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700">
            <tr className="text-left text-gray-500 text-xs uppercase">
              <th className="px-4 py-2">Source</th>
              <th className="px-4 py-2">Translation</th>
              <th className="px-4 py-2 w-24">Language</th>
              <th className="px-4 py-2 w-16 text-center">Uses</th>
              <th className="px-4 py-2 w-36">Last Used</th>
              <th className="px-4 py-2 w-12"></th>
            </tr>
          </thead>
          <tbody>
            {entries.length === 0 ? (
              <tr>
                <td colSpan={6} className="px-4 py-8 text-center text-gray-400">
                  {search ? "No matches found." : "Translation memory is empty."}
                </td>
              </tr>
            ) : (
              entries.map((entry, i) => (
                <tr
                  key={`${entry.source_hash}-${entry.lang_pair}-${i}`}
                  className="border-b border-gray-100 dark:border-gray-800 hover:bg-gray-50 dark:hover:bg-gray-800/50"
                >
                  <td className="px-4 py-2 max-w-xs truncate" title={entry.source}>
                    {entry.source}
                  </td>
                  <td className="px-4 py-2 max-w-xs truncate" title={entry.translation}>
                    {entry.translation}
                  </td>
                  <td className="px-4 py-2">
                    <span className="px-2 py-0.5 bg-blue-100 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 rounded text-xs">
                      {entry.lang_pair}
                    </span>
                  </td>
                  <td className="px-4 py-2 text-center text-gray-500">{entry.uses}</td>
                  <td className="px-4 py-2 text-gray-500 text-xs">
                    {new Date(entry.last_used).toLocaleString()}
                  </td>
                  <td className="px-4 py-2">
                    <button
                      onClick={() => handleDelete(entry.source_hash, entry.lang_pair)}
                      className="text-red-400 hover:text-red-600"
                    >
                      <Trash2 size={14} />
                    </button>
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      {/* Pagination */}
      {totalPages > 1 && (
        <div className="flex items-center justify-between px-6 py-3 border-t border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-900">
          <span className="text-sm text-gray-500">
            Showing {offset + 1}-{Math.min(offset + PAGE_SIZE, total)} of {total}
          </span>
          <div className="flex gap-2">
            <button
              onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
              disabled={offset === 0}
              className="flex items-center gap-1 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm disabled:opacity-50"
            >
              <ChevronLeft size={14} /> Prev
            </button>
            <span className="px-3 py-1.5 text-sm text-gray-600 dark:text-gray-400">
              {currentPage} / {totalPages}
            </span>
            <button
              onClick={() => setOffset(offset + PAGE_SIZE)}
              disabled={offset + PAGE_SIZE >= total}
              className="flex items-center gap-1 px-3 py-1.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm disabled:opacity-50"
            >
              Next <ChevronRight size={14} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
