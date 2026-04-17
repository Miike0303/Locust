import { useState, useEffect } from "react";
import { X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { getProviders, startTranslation } from "../lib/api";
import { subscribeToJob } from "../lib/ws";
import { useEditorStore } from "../stores/editorStore";
import { useQueueStore } from "../stores/queueStore";
import { useProjectStore } from "../stores/projectStore";
import { addLog } from "../stores/logStore";
import { addToast } from "../stores/toastStore";

interface TranslationModalProps {
  open: boolean;
  onClose: () => void;
  totalPending: number;
  onComplete: () => void;
}

const LANGUAGES: { code: string; name: string }[] = [
  { code: "en", name: "English" },
  { code: "es", name: "Español (Spanish)" },
  { code: "ja", name: "日本語 (Japanese)" },
  { code: "zh-CN", name: "简体中文 (Chinese Simplified)" },
  { code: "zh-TW", name: "繁體中文 (Chinese Traditional)" },
  { code: "ko", name: "한국어 (Korean)" },
  { code: "fr", name: "Français (French)" },
  { code: "de", name: "Deutsch (German)" },
  { code: "it", name: "Italiano (Italian)" },
  { code: "pt", name: "Português (Portuguese)" },
  { code: "pt-BR", name: "Português Brasileiro" },
  { code: "ru", name: "Русский (Russian)" },
  { code: "nl", name: "Nederlands (Dutch)" },
  { code: "pl", name: "Polski (Polish)" },
  { code: "tr", name: "Türkçe (Turkish)" },
  { code: "ar", name: "العربية (Arabic)" },
  { code: "vi", name: "Tiếng Việt (Vietnamese)" },
  { code: "th", name: "ไทย (Thai)" },
  { code: "id", name: "Bahasa Indonesia" },
];

const LANG_STORAGE_KEY = "locust.translation.langs";

export default function TranslationModal({ open, onClose, totalPending, onComplete }: TranslationModalProps) {
  const { data: providers } = useQuery({ queryKey: ["providers"], queryFn: getProviders, enabled: open });
  const { setJob, setTranslating } = useEditorStore();

  const [providerId, setProviderId] = useState("google");
  // Load saved language preferences (fallback: auto-detect source, Spanish target)
  const saved = (() => {
    try { return JSON.parse(localStorage.getItem(LANG_STORAGE_KEY) || "{}"); } catch { return {}; }
  })();
  const [sourceLang, setSourceLang] = useState<string>(saved.source ?? "auto");
  const [targetLang, setTargetLang] = useState<string>(saved.target ?? "es");
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
      setLastTranslated("");
    }
  }, [open]);

  const handleStart = async () => {
    // Persist language selection for next time
    try { localStorage.setItem(LANG_STORAGE_KEY, JSON.stringify({ source: sourceLang, target: targetLang })); } catch {}
    addLog("info", `Starting translation with provider: ${providerId}, source: ${sourceLang}, target: ${targetLang}, batch: ${batchSize}`, undefined, "translation");
    try {
      const params = {
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
      };
      addLog("info", `Calling startTranslation API...`, JSON.stringify(params, null, 2), "translation");
      const result = await startTranslation(params);
      addLog("info", `Got job_id: ${result.job_id}`, undefined, "translation");

      setJob(result.job_id);
      setTranslating(true);
      setStep("progress");
      addLog("info", `Translation started (${providerId}), subscribing to WebSocket...`, undefined, "translation");

      const projectName = useProjectStore.getState().project?.name ?? "Project";

      const unsub = subscribeToJob(result.job_id, {
        onStarted: (e) => {
          setTotal(e.total);
          useQueueStore.getState().setGlobalProgress({
            projectName,
            completed: 0,
            total: e.total,
            costSoFar: 0,
            startedAt: Date.now(),
          });
        },
        onBatchCompleted: (e) => {
          setCompleted(e.completed);
          setCostSoFar(e.cost_so_far);
          useQueueStore.getState().setGlobalProgress({
            projectName,
            completed: e.completed,
            total: e.total,
            costSoFar: e.cost_so_far,
            startedAt: useQueueStore.getState().globalProgress?.startedAt ?? Date.now(),
          });
        },
        onStringTranslated: (e) => setLastTranslated(e.translation),
        onCompleted: (e) => {
          setDone(true);
          setTranslating(false);
          setJob(null);
          useQueueStore.getState().setGlobalProgress(null);
          addLog("info", `Translation complete: ${e.total_translated} strings, $${e.total_cost?.toFixed(4) ?? "0"}`, undefined, "translation");
          addToast("success", `Translation complete: ${e.total_translated} strings`);
        },
        onFailed: (e) => {
          setError(e.error);
          setTranslating(false);
          setJob(null);
          useQueueStore.getState().setGlobalProgress(null);
          addLog("error", `Translation failed`, e.error, "translation");
          addToast("error", `Translation failed: ${e.error}`);
        },
      });

      // Cleanup on unmount
      return unsub;
    } catch (err: any) {
      addLog("error", `Translation start failed: ${err.message ?? err}`, err.stack ?? String(err), "translation");
      addToast("error", `Translation failed to start: ${err.message ?? err}`);
      setError(err.message ?? String(err));
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
                <select value={sourceLang} onChange={(e) => setSourceLang(e.target.value)}
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm">
                  <option value="auto">Auto-detect</option>
                  {LANGUAGES.map(l => <option key={l.code} value={l.code}>{l.name}</option>)}
                </select>
              </div>
              <div>
                <label className="text-sm font-medium">Target</label>
                <select value={targetLang} onChange={(e) => setTargetLang(e.target.value)}
                  className="mt-1 w-full p-2 border rounded dark:bg-gray-800 dark:border-gray-600 text-sm">
                  {LANGUAGES.map(l => <option key={l.code} value={l.code}>{l.name}</option>)}
                </select>
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
