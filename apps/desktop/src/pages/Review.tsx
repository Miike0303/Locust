import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, SkipForward, X } from "lucide-react";
import { getStrings, patchString, type StringEntry } from "../lib/api";
import DiffView from "../components/DiffView";
import EmptyState from "../components/EmptyState";
import { shouldHandleEscape, shouldRunActionHotkey } from "../lib/hotkeyPolicy";
import { useProjectStore } from "../stores/projectStore";
import { useT } from "../lib/i18n";
import {
  applyBootstrap,
  applyFetchedPage,
  approveCurrent,
  createReviewQueue,
  mergeEntriesById,
  nextFetchPlan,
  pageMatchesPlan,
  prevCurrent,
  REVIEW_PAGE_SIZE,
  reviewProgress,
  shouldLoadMore,
  skipCurrent,
  type ReviewQueueState,
} from "../lib/reviewQueue";

export default function Review() {
  const t = useT();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const project = useProjectStore((s) => s.project);
  const [queue, setQueue] = useState<ReviewQueueState>(createReviewQueue);
  const [byId, setById] = useState<Record<string, StringEntry>>({});
  const [showDiff, setShowDiff] = useState(false);
  const [translation, setTranslation] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const fetchingRef = useRef(false);
  const queueRef = useRef(queue);
  const loadGenRef = useRef(0);

  const projectPath = project?.path ?? null;

  useEffect(() => {
    loadGenRef.current += 1;
    queueRef.current = createReviewQueue();
    setQueue(queueRef.current);
    setById({});
    setShowDiff(false);
    setTranslation("");
    setLoadError(null);
    setInitialLoading(true);
    fetchingRef.current = false;
  }, [projectPath]);

  const loadMore = useCallback(async () => {
    if (!projectPath || fetchingRef.current) return;
    const state = queueRef.current;
    if (!shouldLoadMore(state)) return;
    const plan = nextFetchPlan(state);
    if (plan.type === "none") return;

    const gen = loadGenRef.current;
    fetchingRef.current = true;
    let failed = false;
    try {
      if (plan.type === "bootstrap") {
        const [translated, reviewed] = await Promise.all([
          getStrings({ status: "translated", limit: REVIEW_PAGE_SIZE, offset: 0 }),
          getStrings({ status: "reviewed", limit: REVIEW_PAGE_SIZE, offset: 0 }),
        ]);
        if (gen !== loadGenRef.current) return;
        const next = applyBootstrap(
          queueRef.current,
          { entries: translated.entries, total: translated.total },
          { entries: reviewed.entries, total: reviewed.total },
        );
        queueRef.current = next;
        setQueue(next);
        const incoming = next.translatedExhausted
          ? [...translated.entries, ...reviewed.entries]
          : translated.entries;
        setById((prev) => mergeEntriesById(prev, incoming));
        setLoadError(null);
        setInitialLoading(false);
      } else {
        const res = await getStrings({
          status: plan.type,
          limit: plan.limit,
          offset: plan.offset,
        });
        if (gen !== loadGenRef.current) return;
        const latest = queueRef.current;
        if (pageMatchesPlan(latest, plan.type, plan.offset)) {
          const next = applyFetchedPage(
            latest,
            plan.type,
            { entries: res.entries, total: res.total },
            plan.offset,
          );
          queueRef.current = next;
          setQueue(next);
          setById((prev) => mergeEntriesById(prev, res.entries));
        }
      }
    } catch (err: unknown) {
      if (gen !== loadGenRef.current) return;
      failed = true;
      const message = err instanceof Error ? err.message : String(err);
      setLoadError(message);
      setInitialLoading(false);
    } finally {
      if (gen === loadGenRef.current) fetchingRef.current = false;
    }
    if (
      gen === loadGenRef.current &&
      !failed &&
      shouldLoadMore(queueRef.current) &&
      nextFetchPlan(queueRef.current).type !== "none"
    ) {
      void loadMore();
    }
  }, [projectPath]);

  useEffect(() => {
    void loadMore();
  }, [
    loadMore,
    queue.bootstrapped,
    queue.index,
    queue.items.length,
    queue.pendingAdvance,
    queue.translatedExhausted,
    queue.reviewedExhausted,
    queue.complete,
  ]);

  const entryId = queue.items[queue.index]?.id;
  const entry = entryId ? byId[entryId] : undefined;
  const progress = reviewProgress(queue);

  useEffect(() => {
    if (entry) setTranslation(entry.translation || "");
  }, [entry?.id]);

  const handleApprove = useCallback(async () => {
    const state = queueRef.current;
    const id = state.items[state.index]?.id;
    const current = id ? byId[id] : undefined;
    if (!current || state.complete) return;
    if (!state.approvedIds.includes(current.id)) {
      if (translation !== (current.translation || "")) {
        await patchString(current.id, { translation } as any);
      }
      await patchString(current.id, { status: "approved" } as any);
      setById((prev) => ({
        ...prev,
        [current.id]: { ...current, translation, status: "approved" },
      }));
    }
    const next = approveCurrent(queueRef.current);
    queueRef.current = next;
    setQueue(next);
    void qc.invalidateQueries({ queryKey: ["stats"], refetchType: "none" });
  }, [byId, translation, qc]);

  const handleSkip = useCallback(() => {
    const next = skipCurrent(queueRef.current);
    queueRef.current = next;
    setQueue(next);
  }, []);

  const handlePrev = useCallback(() => {
    const next = prevCurrent(queueRef.current);
    queueRef.current = next;
    setQueue(next);
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        if (shouldHandleEscape({ overlayOpen: false, target: e.target as HTMLElement | null })) {
          navigate("/editor");
        }
        return;
      }
      if (!shouldRunActionHotkey({
        overlayOpen: !!document.querySelector("[data-hotkey-overlay]"),
        target: e.target as HTMLElement | null,
      })) return;
      if (e.key === "a" || (e.ctrlKey && e.key === "Enter")) { e.preventDefault(); void handleApprove(); }
      if (e.key === "s") handleSkip();
      if (e.key === "p") handlePrev();
      if (e.key === "e") document.querySelector<HTMLTextAreaElement>("#review-textarea")?.focus();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [handleApprove, handleSkip, handlePrev, navigate]);

  if (!project) {
    return (
      <EmptyState
        title={t("editor.empty.title")}
        description={t("editor.empty.description")}
        actionLabel={t("editor.empty.action")}
        onAction={() => navigate("/")}
      />
    );
  }

  if (initialLoading) {
    return (
      <div className="flex h-full items-center justify-center text-gray-500">
        {t("review.loading")}
      </div>
    );
  }

  if (loadError && !queue.bootstrapped) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 px-6 text-center">
        <h1 className="text-xl font-semibold text-red-600">{t("review.loadError")}</h1>
        <p className="text-sm text-gray-500">
          {loadError || t("common.tryAgain")}
        </p>
        <button
          onClick={() => {
            setLoadError(null);
            setInitialLoading(true);
            fetchingRef.current = false;
            const fresh = createReviewQueue();
            queueRef.current = fresh;
            setQueue(fresh);
            void loadMore();
          }}
          className="rounded bg-emerald-600 px-4 py-2 font-medium text-white hover:bg-emerald-700"
        >
          {t("common.retry")}
        </button>
      </div>
    );
  }

  const showComplete = queue.complete && progress.total > 0;
  const showNothing = queue.bootstrapped && progress.total === 0;

  if (showComplete || showNothing) {
    return (
      <div className="flex h-full flex-col items-center justify-center px-6 text-center">
        <h1 className="text-xl font-semibold">
          {showComplete ? t("review.complete") : t("review.nothing")}
        </h1>
        <p className="mt-2 mb-4 text-gray-500">
          {showComplete
            ? t("review.completeHint")
            : t("review.nothingHint")}
        </p>
        <button
          onClick={() => navigate("/editor")}
          className="rounded bg-emerald-600 px-4 py-2 font-medium text-white hover:bg-emerald-700"
        >
          {t("review.returnToEditor")}
        </button>
      </div>
    );
  }

  const bar = progress.total > 0 ? (progress.current / progress.total) * 100 : 0;

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="p-4 border-b border-gray-200 dark:border-gray-700 space-y-2">
        <div className="flex justify-between items-center">
          <span className="text-sm font-medium">
            {t("review.progress", {
              current: progress.current,
              total: progress.total,
              approved: progress.approved,
            })}
          </span>
          <button onClick={() => navigate("/editor")} className="text-sm text-gray-500 hover:text-gray-700 flex items-center gap-1">
            <X size={14} /> {t("common.exit")}
          </button>
        </div>
        <div className="w-full bg-gray-200 dark:bg-gray-700 rounded-full h-2">
          <div className="bg-emerald-500 h-2 rounded-full transition-all" style={{ width: `${bar}%` }} />
        </div>
      </div>

      {/* Content */}
      {entry && (
        <div className="flex-1 overflow-y-auto p-6 max-w-3xl mx-auto w-full space-y-4">
          <div className="text-xs text-gray-500 flex gap-3">
            <span>{entry.file_path.split(/[/\\]/).pop()}</span>
            {entry.context && <span>{t("review.context", { context: entry.context })}</span>}
            {entry.tags.map((tag) => (
              <span key={tag} className="px-1.5 py-0.5 bg-gray-100 dark:bg-gray-700 rounded">{tag}</span>
            ))}
          </div>

          <div>
            <h3 className="text-xs font-semibold text-gray-500 uppercase mb-1">{t("review.source")}</h3>
            <div className="p-3 bg-gray-50 dark:bg-gray-800 rounded font-mono text-sm whitespace-pre-wrap select-all">
              {entry.source}
            </div>
          </div>

          <div>
            <div className="flex justify-between items-center mb-1">
              <h3 className="text-xs font-semibold text-gray-500 uppercase">{t("review.translation")}</h3>
              <button onClick={() => setShowDiff(!showDiff)} className="text-xs text-emerald-600 hover:underline">
                {showDiff ? t("review.hideDiff") : t("review.showDiff")}
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
                {t("review.chars", { count: translation.length, limit: entry.char_limit })}
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
        <button onClick={handlePrev} disabled={queue.index === 0}
          className="flex items-center gap-1.5 px-4 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium disabled:opacity-50">
          <ArrowLeft size={16} /> {t("review.previous")} <kbd className="text-xs text-gray-400 ml-1">P</kbd>
        </button>
        <button onClick={handleSkip}
          className="flex items-center gap-1.5 px-4 py-2 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 rounded text-sm font-medium">
          <SkipForward size={16} /> {t("review.skip")} <kbd className="text-xs text-gray-400 ml-1">S</kbd>
        </button>
        <button onClick={() => { void handleApprove(); }}
          className="flex items-center gap-1.5 px-6 py-2 bg-emerald-600 hover:bg-emerald-700 text-white rounded text-sm font-medium">
          <Check size={16} /> {t("review.approve")} <kbd className="text-xs text-white/70 ml-1">A</kbd>
        </button>
      </div>
    </div>
  );
}
