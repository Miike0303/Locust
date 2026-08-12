import { useState } from "react";
import { X, Replace, AlertCircle } from "lucide-react";
import { batchPatchStrings, getStrings, type StringEntry } from "../lib/api";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";
import { useModalA11y, MODAL_BACKDROP_CLASS, modalPanelClass } from "../lib/modalA11y";

interface SearchReplaceModalProps {
  open: boolean;
  onClose: () => void;
  onDone?: () => void;
}

function countMatches(text: string, find: string, caseSensitive: boolean): number {
  if (!find) return 0;
  if (caseSensitive) {
    let n = 0;
    let i = 0;
    while ((i = text.indexOf(find, i)) !== -1) {
      n++;
      i += find.length;
    }
    return n;
  }
  const lower = text.toLowerCase();
  const f = find.toLowerCase();
  let n = 0;
  let i = 0;
  while ((i = lower.indexOf(f, i)) !== -1) {
    n++;
    i += f.length;
  }
  return n;
}

function replaceAll(
  text: string,
  find: string,
  replace: string,
  caseSensitive: boolean
): string {
  if (!find) return text;
  if (caseSensitive) {
    return text.split(find).join(replace);
  }
  // Case-insensitive replace preserving original match casing is overkill —
  // use the provided replacement string for every hit.
  const re = new RegExp(find.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
  return text.replace(re, replace);
}

export default function SearchReplaceModal({
  open,
  onClose,
  onDone,
}: SearchReplaceModalProps) {
  const [find, setFind] = useState("");
  const [replace, setReplace] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [loading, setLoading] = useState(false);
  const [preview, setPreview] = useState<{
    entries: number;
    occurrences: number;
    samples: { id: string; before: string; after: string }[];
  } | null>(null);
  const { dialogRef, dialogProps, titleProps } = useModalA11y({ open, ownEscape: false });

  if (!open) return null;

  const runPreview = async () => {
    if (!find) {
      addToast("error", "Enter text to find");
      return;
    }
    setLoading(true);
    setPreview(null);
    try {
      const res = await getStrings({ search: find, limit: 50_000, offset: 0 });
      let entriesHit = 0;
      let occurrences = 0;
      const samples: { id: string; before: string; after: string }[] = [];
      for (const e of res.entries) {
        const target = e.translation;
        if (!target) continue;
        const nTr = countMatches(target, find, caseSensitive);
        if (nTr === 0) continue;
        entriesHit++;
        occurrences += nTr;
        if (samples.length < 5) {
          samples.push({
            id: e.id,
            before: target.slice(0, 120),
            after: replaceAll(target, find, replace, caseSensitive).slice(0, 120),
          });
        }
      }
      setPreview({ entries: entriesHit, occurrences, samples });
      if (entriesHit === 0) {
        addToast("info", "No translation matches");
      }
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      addToast("error", `Preview failed: ${msg}`);
    } finally {
      setLoading(false);
    }
  };

  const runReplace = async () => {
    if (!find) {
      addToast("error", "Enter text to find");
      return;
    }
    setLoading(true);
    try {
      const res = await getStrings({ search: find, limit: 50_000, offset: 0 });
      let occurrences = 0;
      const updates: { id: string; translation: string }[] = [];
      for (const e of res.entries as StringEntry[]) {
        if (!e.translation) continue;
        const n = countMatches(e.translation, find, caseSensitive);
        if (n === 0) continue;
        const next = replaceAll(e.translation, find, replace, caseSensitive);
        if (next === e.translation) continue;
        updates.push({ id: e.id, translation: next });
        occurrences += n;
      }
      if (updates.length === 0) {
        addToast("info", "Nothing to replace");
        return;
      }
      const result = await batchPatchStrings(updates, "search-replace");
      addToast(
        result.skipped ? "warning" : "success",
        `Replaced in ${result.applied} string(s) (${occurrences} occurrence(s))` +
          (result.skipped ? ` — ${result.skipped} skipped` : "")
      );
      addLog(
        "info",
        `Search-replace batch: ${result.applied}/${result.requested} applied, ${occurrences} hits`,
        find.length > 40 ? `${find.slice(0, 40)}…` : find,
        "replace"
      );
      onDone?.();
      onClose();
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      addToast("error", `Replace failed: ${msg}`);
      addLog("error", "Search-replace failed", msg, "replace");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className={MODAL_BACKDROP_CLASS}>
      <div ref={dialogRef} {...dialogProps} className={modalPanelClass("max-w-lg p-6")}>
        <div className="flex justify-between items-center mb-4">
          <h2 {...titleProps} className="text-lg font-bold flex items-center gap-2">
            <Replace size={18} /> Search &amp; Replace
          </h2>
          <button onClick={onClose} className="text-gray-400 hover:text-gray-600">
            <X size={20} />
          </button>
        </div>

        <div className="space-y-3">
          <div>
            <label className="text-sm font-medium">Find in translations</label>
            <input
              value={find}
              onChange={(e) => {
                setFind(e.target.value);
                setPreview(null);
              }}
              className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm font-mono"
              placeholder="text to find"
              autoFocus
            />
          </div>
          <div>
            <label className="text-sm font-medium">Replace with</label>
            <input
              value={replace}
              onChange={(e) => {
                setReplace(e.target.value);
                setPreview(null);
              }}
              className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm font-mono"
              placeholder="replacement (can be empty)"
            />
          </div>

          <label className="flex items-center gap-2 text-sm cursor-pointer">
            <input
              type="checkbox"
              checked={caseSensitive}
              onChange={(e) => {
                setCaseSensitive(e.target.checked);
                setPreview(null);
              }}
            />
            Case sensitive
          </label>

          <p className="text-xs text-gray-500 flex items-start gap-1">
            <AlertCircle size={12} className="mt-0.5 shrink-0" />
            Only <strong>translations</strong> are modified (sources stay fixed).
            Scope is the open project; uses server search then exact replace.
          </p>

          {preview && (
            <div className="text-sm border rounded p-3 dark:border-gray-700 bg-gray-50 dark:bg-gray-800/50">
              <p>
                <strong>{preview.entries}</strong> string(s),{" "}
                <strong>{preview.occurrences}</strong> occurrence(s)
              </p>
              {preview.samples.length > 0 && (
                <ul className="mt-2 space-y-2 text-xs font-mono max-h-32 overflow-y-auto">
                  {preview.samples.map((s) => (
                    <li key={s.id} className="border-t dark:border-gray-700 pt-1">
                      <div className="text-gray-500 truncate">{s.id}</div>
                      <div className="text-red-600/80 truncate">− {s.before}</div>
                      <div className="text-emerald-600/80 truncate">+ {s.after}</div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-2">
            <button
              onClick={onClose}
              className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700"
            >
              Cancel
            </button>
            <button
              onClick={() => {
                void runPreview();
              }}
              disabled={loading || !find}
              className="px-3 py-2 text-sm rounded bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 disabled:opacity-50"
            >
              {loading ? "…" : "Preview"}
            </button>
            <button
              onClick={() => {
                void runReplace();
              }}
              disabled={loading || !find}
              className="px-4 py-2 text-sm font-medium rounded bg-emerald-600 hover:bg-emerald-700 disabled:opacity-50 text-white"
            >
              {loading ? "Replacing…" : "Replace all"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
