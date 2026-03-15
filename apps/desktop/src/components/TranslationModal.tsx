import { useState, useEffect } from "react";
import { X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getProviders, startTranslation } from "../lib/api";
import { subscribeToJob } from "../lib/ws";
import { useEditorStore } from "../stores/editorStore";

interface TranslationModalProps {
  open: boolean;
  onClose: () => void;
  totalPending: number;
  onComplete: () => void;
}

export default function TranslationModal({ open, onClose, totalPending, onComplete }: TranslationModalProps) {
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders, enabled: open });
  const { setJob, setTranslating } = useEditorStore();

  const [providerId, setProviderId] = useState("mock");
  const [sourceLang, setSourceLang] = useState("ja");
  const [targetLang, setTargetLang] = useState("en");
  const [batchSize, setBatchSize] = useState(40);
  const [costLimit, setCostLimit] = useState("");
  const [gameContext, setGameContext] = useState("");
  const [useGlossary, setUseGlossary] = useState(true);
  const [useMemory, setUseMemory] = useState(true);

  // Progress state
  const [step, setStep] = useState<"configure" | "progress">("configure");
  const [completed, setCompleted] = useState(0);
  const [total, setTotal] = useState(0);
  const [costSoFar, setCostSoFar] = useState(0);
  const [lastTranslated, setLastTranslated] = useState("");
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setStep("configure");
      setCompleted(0);
      setTotal(0);
      setCostSoFar(0);
      setDone(false);
      setError(null);
    }
  }, [open]);

  const handleStart = async () => {
    try {
      const result = await startTranslation({
        provider_id: providerId,
        options: {
          source_lang: sourceLang,
          target_lang: targetLang,
          batch_size: batchSize,
          max_concurrent: 3,
          cost_limit_usd: costLimit ? parseFloat(costLimit) : null,
          game_context: gameContext || null,
          use_glossary: useGlossary,
          use_memory: useMemory,
          skip_approved: true,
        },
      });

      setJob(result.job_id);
      setTranslating(true);
      setStep("progress");

      const unsub = subscribeToJob(result.job_id, {
        onStarted: (e) => setTotal(e.total),
        onBatchCompleted: (e) => { setCompleted(e.completed); setCostSoFar(e.cost_so_far); },
        onStringTranslated: (e) => setLastTranslated(e.translation),
        onCompleted: () => { setDone(true); setTranslating(false); setJob(null); },
        onFailed: (e) => { setError(e.error); setTranslating(false); setJob(null); },
      });

      // Cleanup on unmount
      return unsub;
    } catch (err: any) {
      setError(err.message);
    }
  };

  if (!open) return null;

  const progressPercent = total > 0 ? (completed / total) * 100 : 0;

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 rounded-lg shadow-xl w-full max-w-lg p-6">
        <div className="flex justify-between items-center mb-4">
          <h2 className="text-lg font-bold">{step === "configure" ? "Start Translation" : "Translation Progress"}</h2>
          <button onClick={() => { onClose(); if (done) onComplete(); }} className="text-gray-400 hover:text-gray-600"><X size={20} /></button>
        </div>

        {step === "configure" && (
          <div className="space-y-4">
            <div>
              <label className="text-sm font-medium">Provider</label>
              <select value={providerId} onChange={(e) => setProviderId(e.target.value)}
                className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm">
                {providers?.map((p) => <option key={p.id} value={p.id}>{p.name} {p.is_free ? "(free)" : ""}</option>)}
              </select>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="text-sm font-medium">Source</label>
                <input value={sourceLang} onChange={(e) => setSourceLang(e.target.value)}
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm" />
              </div>
              <div>
                <label className="text-sm font-medium">Target</label>
                <input value={targetLang} onChange={(e) => setTargetLang(e.target.value)}
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm" />
              </div>
            </div>
            <div>
              <label className="text-sm font-medium">Game context</label>
              <textarea value={gameContext} onChange={(e) => setGameContext(e.target.value)} rows={2}
                placeholder="Describe genre, tone, setting..."
                className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm" />
            </div>
            <div className="flex gap-4">
              <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={useGlossary} onChange={(e) => setUseGlossary(e.target.checked)} /> Use glossary</label>
              <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={useMemory} onChange={(e) => setUseMemory(e.target.checked)} /> Use memory</label>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <div>
                <label className="text-sm font-medium">Batch size</label>
                <input type="number" value={batchSize} onChange={(e) => setBatchSize(+e.target.value)} min={1} max={100}
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm" />
              </div>
              <div>
                <label className="text-sm font-medium">Cost limit ($)</label>
                <input value={costLimit} onChange={(e) => setCostLimit(e.target.value)} placeholder="No limit"
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm" />
              </div>
            </div>
            <p className="text-sm text-gray-500">{totalPending} pending strings to translate</p>
            <button onClick={handleStart}
              className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium transition-colors">
              Start Translation
            </button>
          </div>
        )}

        {step === "progress" && (
          <div className="space-y-4">
            <div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-3">
              <div className="bg-emerald-500 h-3 rounded-full transition-all" style={{ width: `${progressPercent}%` }} />
            </div>
            <div className="text-center text-sm">
              {done ? "Complete!" : `${completed} / ${total}`}
              {costSoFar > 0 && ` · $${costSoFar.toFixed(4)}`}
            </div>
            {lastTranslated && !done && (
              <div className="text-xs text-gray-500 truncate">Last: {lastTranslated}</div>
            )}
            {error && <div className="p-2 bg-red-50 dark:bg-red-900/20 border border-red-200 rounded text-sm text-red-600">{error}</div>}
            {done && (
              <button onClick={() => { onClose(); onComplete(); }}
                className="w-full py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded font-medium">
                Close
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
