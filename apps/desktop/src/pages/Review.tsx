import { useState, useEffect, useCallback } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowRight, Check, SkipForward, X } from "lucide-react";
import { getStrings, patchString } from "../lib/api";
import DiffView from "../components/DiffView";

export default function Review() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [index, setIndex] = useState(0);
  const [showDiff, setShowDiff] = useState(false);
  const [translation, setTranslation] = useState("");
  const [approved, setApproved] = useState(0);

  const { data } = useQuery({
    queryKey: ["review-strings"],
    queryFn: () => getStrings({ status: "translated", limit: 50000 }),
  });

  const entries = data?.entries || [];
  const entry = entries[index];
  const total = entries.length;

  useEffect(() => {
    if (entry) setTranslation(entry.translation || "");
  }, [entry?.id]);

  const advance = useCallback(() => {
    if (index < total - 1) setIndex((i) => i + 1);
  }, [index, total]);

  const handleApprove = useCallback(async () => {
    if (!entry) return;
    if (translation !== (entry.translation || "")) {
      await patchString(entry.id, { translation } as any);
    }
    await patchString(entry.id, { status: "approved" } as any);
    setApproved((a) => a + 1);
    advance();
  }, [entry, translation, advance]);

  const handleSkip = useCallback(() => advance(), [advance]);
  const handlePrev = useCallback(() => {
    if (index > 0) setIndex((i) => i - 1);
  }, [index]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement) return;
      if (e.key === "a" || (e.ctrlKey && e.key === "Enter")) { e.preventDefault(); handleApprove(); }
      if (e.key === "s") handleSkip();
      if (e.key === "p") handlePrev();
      if (e.key === "e") document.querySelector<HTMLTextAreaElement>("#review-textarea")?.focus();
      if (e.key === "Escape") navigate("/editor");
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [handleApprove, handleSkip, handlePrev, navigate]);

  if (total === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full">
        <p className="text-gray-500 text-lg mb-4">No translated strings to review.</p>
        <button onClick={() => navigate("/editor")} className="px-4 py-2 bg-emerald-600 text-white rounded">Back to Editor</button>
      </div>
    );
  }

  const progress = total > 0 ? ((index + 1) / total) * 100 : 0;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex justify-between items-center">
          <span className="text-sm font-medium">
            Reviewing {index + 1} of {total} — {approved} approved
          </span>
          <button onClick={() => navigate("/editor")} className="text-sm text-gray-500 hover:text-gray-700 flex items-center gap-1">
            <X size={14} /> Exit
          </button>
        </div>
        <div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2">
          <div className="bg-emerald-500 h-2 rounded-full transition-all" style={{ width: `${progress}%` }} />
        </div>
      </div>

      {/* Content */}
      {entry && (
        <div className="flex-1 overflow-y-auto p-6 max-w-3xl mx-auto w-full space-y-4">
          <div className="text-xs text-gray-500 flex gap-3">
            <span>{entry.file_path.split(/[/\\]/).pop()}</span>
            {entry.context && <span>Context: {entry.context}</span>}
            {entry.tags.map((t) => (
              <span key={t} className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded">{t}</span>
            ))}
          </div>

          <div>
            <h3 className="text-xs font-semibold text-gray-500 uppercase mb-1">Source</h3>
            <div className="p-3 bg-gray-50 dark:bg-gray-800 rounded font-mono text-sm whitespace-pre-wrap select-all">
              {entry.source}
            </div>
          </div>

          <div>
            <div className="flex justify-between items-center mb-1">
              <h3 className="text-xs font-semibold text-gray-500 uppercase">Translation</h3>
              <button onClick={() => setShowDiff(!showDiff)} className="text-xs text-emerald-600 hover:underline">
                {showDiff ? "Hide Diff" : "Show Diff"}
              </button>
            </div>
            <textarea
              id="review-textarea"
              value={translation}
              onChange={(e) => setTranslation(e.target.value)}
              className="w-full p-3 border border-gray-300 dark:border-gray-600 rounded bg-white dark:bg-gray-800 text-sm focus:outline-none focus:ring-2 focus:ring-emerald-500 resize-y min-h-[100px] font-mono"
              rows={4}
            />
            {entry.char_limit != null && (
              <div className={`text-xs mt-1 ${translation.length > entry.char_limit ? "text-red-500" : "text-gray-400"}`}>
                {translation.length} / {entry.char_limit} chars
              </div>
            )}
          </div>

          {showDiff && entry.translation && (
            <DiffView originalText={entry.source} translatedText={translation} entryId={entry.id} />
          )}
        </div>
      )}

      {/* Bottom bar */}
      <div className="p-4 border-t border-gray-200 dark:border-gray-700 flex justify-center gap-3">
        <button onClick={handlePrev} disabled={index === 0}
          className="flex items-center gap-1.5 px-4 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium disabled:opacity-50">
          <ArrowLeft size={16} /> Previous <kbd className="text-xs text-gray-400 ml-1">P</kbd>
        </button>
        <button onClick={handleSkip}
          className="flex items-center gap-1.5 px-4 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium">
          <SkipForward size={16} /> Skip <kbd className="text-xs text-gray-400 ml-1">S</kbd>
        </button>
        <button onClick={handleApprove}
          className="flex items-center gap-1.5 px-6 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium">
          <Check size={16} /> Approve <kbd className="text-xs text-white/70 ml-1">A</kbd>
        </button>
      </div>
    </div>
  );
}
