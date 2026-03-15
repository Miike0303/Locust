import { useState, useEffect, useCallback } from "react";
import { X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getStrings, patchString } from "../lib/api";
import type { StringEntry } from "../lib/api";

interface SearchReplaceProps {
  onClose: () => void;
}

export default function SearchReplace({ onClose }: SearchReplaceProps) {
  const [search, setSearch] = useState("");
  const [replace, setReplace] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [matches, setMatches] = useState<StringEntry[]>([]);
  const [applying, setApplying] = useState(false);

  const { data } = useQuery({
    queryKey: ["strings-all"],
    queryFn: () => getStrings({ limit: 50000 }),
  });

  useEffect(() => {
    if (!search || !data) {
      setMatches([]);
      return;
    }
    const s = caseSensitive ? search : search.toLowerCase();
    const found = data.entries.filter((e) => {
      const t = e.translation || "";
      return caseSensitive ? t.includes(s) : t.toLowerCase().includes(s);
    });
    setMatches(found.slice(0, 10));
  }, [search, data, caseSensitive]);

  const totalMatches = (() => {
    if (!search || !data) return 0;
    const s = caseSensitive ? search : search.toLowerCase();
    return data.entries.filter((e) => {
      const t = e.translation || "";
      return caseSensitive ? t.includes(s) : t.toLowerCase().includes(s);
    }).length;
  })();

  const handleApply = async () => {
    if (!search || !data) return;
    setApplying(true);
    const s = caseSensitive ? search : search.toLowerCase();
    const toUpdate = data.entries.filter((e) => {
      const t = e.translation || "";
      return caseSensitive ? t.includes(s) : t.toLowerCase().includes(s);
    });

    for (const entry of toUpdate) {
      const t = entry.translation || "";
      const newTranslation = caseSensitive
        ? t.replaceAll(search, replace)
        : t.replace(new RegExp(search.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"), replace);
      await patchString(entry.id, { translation: newTranslation } as any);
    }

    setApplying(false);
    onClose();
  };

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-lg p-6">
        <div className="flex justify-between items-center mb-4">
          <h2 className="text-lg font-bold">Search & Replace</h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600"><X size={20} /></button>
        </div>

        <div className="space-y-3">
          <div>
            <label className="text-sm font-medium">Search in translations</label>
            <input value={search} onChange={(e) => setSearch(e.target.value)}
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              placeholder="Search text..." autoFocus />
          </div>
          <div>
            <label className="text-sm font-medium">Replace with</label>
            <input value={replace} onChange={(e) => setReplace(e.target.value)}
              className="mt-1 w-full p-2 border rounded text-sm dark:bg-gray-800 dark:border-gray-600"
              placeholder="Replacement text..." />
          </div>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={caseSensitive} onChange={(e) => setCaseSensitive(e.target.checked)} />
            Case sensitive
          </label>

          {matches.length > 0 && (
            <div className="border border-gray-200 dark:border-gray-700 rounded p-2 max-h-48 overflow-y-auto">
              <p className="text-xs text-gray-500 mb-2">Preview ({totalMatches} matches):</p>
              {matches.map((m) => (
                <div key={m.id} className="text-xs py-1 border-b border-gray-100 dark:border-gray-800 last:border-0">
                  <span className="text-gray-500">{m.id}:</span>{" "}
                  <span>{m.translation}</span>
                </div>
              ))}
              {totalMatches > 10 && <p className="text-xs text-gray-400 mt-1">...and {totalMatches - 10} more</p>}
            </div>
          )}

          <button
            onClick={handleApply}
            disabled={!search || totalMatches === 0 || applying}
            className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white rounded font-medium text-sm"
          >
            {applying ? "Applying..." : `Apply to ${totalMatches} strings`}
          </button>
        </div>
      </div>
    </div>
  );
}
