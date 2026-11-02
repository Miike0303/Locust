import { StatusBadge, statusStyles } from "../lib/statusStyles";
import { useState, useEffect, useCallback, useRef } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, SkipForward, X } from "lucide-react";
import { getFileStats, getSpeakers, getStrings, patchString, type FileStats, type StringEntry } from "../lib/api";
import { speakerOwner } from "../lib/speakers";
import { pathBasename } from "../lib/path";
import { placeholderParts, placeholderCheck } from "../lib/placeholders";
import { buildReviewContextIndex, type ReviewContextIndex } from "../lib/reviewContext";
import DiffView from "../components/DiffView";
import EmptyState from "../components/EmptyState";
import { shouldHandleEscape, shouldRunActionHotkey } from "../lib/hotkeyPolicy";
import { useProjectStore } from "../stores/projectStore";
import { draftProjectKey, isProjectScopedQueryEnabled } from "../lib/projectScope";
import { invalidateProjectData } from "../lib/queryClient";
import { useT } from "../lib/i18n";
import { browserStorage } from "../lib/storage";
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

// Same per-project preference identity as TextFitPreview's saved profile.
const reviewScopeKey = (projectKey: string) => `locust.review.scope.v1:${projectKey}`;

function readReviewScope(projectKey: string): string {
  if (!projectKey) return "";
  try { return browserStorage()?.getItem(reviewScopeKey(projectKey)) ?? ""; }
  catch { return ""; }
}

function rememberReviewScope(projectKey: string, filePath: string): void {
  if (!projectKey) return;
  try { browserStorage()?.setItem(reviewScopeKey(projectKey), filePath); }
  catch { /* File selection still works when storage is unavailable. */ }
}

export function reviewApprovalPatch(
  current: Pick<StringEntry, "translation" | "status">,
  translation: string,
): { translation?: string; status: "approved"; expected_translation: string | null } | null {
  // Saving a translation can reset its status, including on a repeat approval.
  if (translation !== (current.translation ?? "")) {
    return { translation, status: "approved", expected_translation: current.translation };
  }
  return current.status === "approved" ? null : { status: "approved", expected_translation: current.translation };
}

export default function Review() {
  const t = useT();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const project = useProjectStore((s) => s.project);
  const projectPath = project?.path ?? null;
  const projectKey = project ? draftProjectKey(project) : "";
  const databasePath = project?.database_path ?? null;
  const format = project?.format_id ?? "";
  const [savedScope, setSavedScope] = useState(() => ({ projectKey, filePath: readReviewScope(projectKey) }));
  const filePath = savedScope.projectKey === projectKey ? savedScope.filePath : readReviewScope(projectKey);
  const [files, setFiles] = useState<FileStats[] | null>(null);
  const [fileStatsError, setFileStatsError] = useState(false);
  const [fileStatsRevision, setFileStatsRevision] = useState(0);
  const [queue, setQueue] = useState<ReviewQueueState>(createReviewQueue);
  const [byId, setById] = useState<Record<string, StringEntry>>({});
  const [showDiff, setShowDiff] = useState(false);
  const [translation, setTranslation] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const [approving, setApproving] = useState(false);
  const [speakers, setSpeakers] = useState<Record<string, string>>({});
  const [context, setContext] = useState<{ id: string; rows: StringEntry[]; error: boolean } | null>(null);
  const contextPages = useRef(new Map<string, ReturnType<typeof getStrings>>());
  const contextIndexes = useRef(new Map<string, ReviewContextIndex<StringEntry>>());
  const approvingRef = useRef(false);
  const fetchingRef = useRef(false);
  const queueRef = useRef(queue);
  const loadGenRef = useRef(0);

  useEffect(() => {
    let active = true;
    setFiles(null);
    setFileStatsError(false);
    if (projectPath) {
      void getFileStats().then(result => {
        if (!active) return;
        const reviewable = result.filter(file => file.translated + file.reviewed > 0);
        setFiles(reviewable);
        setSavedScope(previous => {
          const selected = previous.projectKey === projectKey ? previous.filePath : readReviewScope(projectKey);
          // A finished, removed or renamed file must not strand a remembered session.
          if (selected && !reviewable.some(file => file.file_path === selected)) {
            rememberReviewScope(projectKey, "");
            return { projectKey, filePath: "" };
          }
          return previous;
        });
      }).catch(() => { if (active) setFileStatsError(true); });
    }
    return () => { active = false; };
  }, [projectKey, fileStatsRevision]);

  useEffect(() => {
    let active = true;
    setSpeakers({});
    setContext(null);
    contextPages.current.clear();
    contextIndexes.current.clear();
    const owner = speakerOwner(project);
    if (owner) {
      void getSpeakers(owner).then(result => {
        if (active) setSpeakers(Object.fromEntries(result.groups.flatMap(group =>
          group.entry_ids.map(id => [id, group.name]))));
      }).catch(() => { /* Unknown speakers remain unlabelled. */ });
    }
    return () => { active = false; };
  }, [projectPath, databasePath, format]);

  const resetSession = useCallback(() => {
    loadGenRef.current += 1;
    queueRef.current = createReviewQueue();
    setQueue(queueRef.current);
    setById({});
    setShowDiff(false);
    setTranslation("");
    setLoadError(null);
    setInitialLoading(true);
    fetchingRef.current = false;
    setContext(null);
    contextPages.current.clear();
    contextIndexes.current.clear();
  }, []);

  useEffect(() => {
    resetSession();
    // Unmounting also invalidates requests that outlive this Review page.
    return () => { loadGenRef.current += 1; };
  }, [projectKey, filePath]);

  const changeScope = (nextFilePath: string) => {
    if (approvingRef.current || nextFilePath === filePath) return;
    // Invalidate synchronously, before React renders or an old request resolves.
    resetSession();
    setSavedScope({ projectKey, filePath: nextFilePath });
    rememberReviewScope(projectKey, nextFilePath);
  };

  const loadMore = useCallback(async () => {
    if (!projectPath || fetchingRef.current) return;
    const state = queueRef.current;
    if (!shouldLoadMore(state)) return;
    const plan = nextFetchPlan(state);
    if (plan.type === "none") return;

    const gen = loadGenRef.current;
    const scope = filePath ? { file_path: filePath } : {};
    fetchingRef.current = true;
    let failed = false;
    try {
      if (plan.type === "bootstrap") {
        const [translated, reviewed] = await Promise.all([
          getStrings({ ...scope, status: "translated", limit: REVIEW_PAGE_SIZE, offset: 0 }),
          getStrings({ ...scope, status: "reviewed", limit: REVIEW_PAGE_SIZE, offset: 0 }),
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
          ...scope,
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
  }, [projectPath, projectKey, filePath]);

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
    if (!entry || !projectPath) return;
    let active = true;
    const current = entry;
    const gen = loadGenRef.current;
    const indexes = contextIndexes.current;
    const cached = indexes.get(current.file_path);
    if (cached) {
      // Map insertion order tracks the four most recently used files.
      indexes.delete(current.file_path);
      indexes.set(current.file_path, cached);
      setContext({ id: current.id, rows: cached.lookup(current), error: false });
      return;
    }
    const pages = contextPages.current;
    const load = async () => {
      const rows: StringEntry[] = [];
      for (let offset = 0; active && gen === loadGenRef.current; offset += REVIEW_PAGE_SIZE) {
        const key = JSON.stringify([current.file_path, offset]);
        let page = pages.get(key);
        if (!page) {
          page = getStrings({ file_path: current.file_path, offset, limit: REVIEW_PAGE_SIZE });
          pages.set(key, page);
          void page.catch(() => { pages.delete(key); });
        }
        const result = await page;
        if (!active || gen !== loadGenRef.current) return;
        // Lexical API pages can split numeric neighbors; collect the entire file.
        rows.push(...result.entries);
        const exhausted = !result.entries.length || offset + result.entries.length >= result.total;
        if (exhausted) {
          const index = buildReviewContextIndex(rows);
          indexes.set(current.file_path, index);
          if (indexes.size > 4) {
            const oldest = indexes.keys().next().value;
            if (oldest !== undefined) indexes.delete(oldest);
          }
          setContext({ id: current.id, rows: index.lookup(current), error: false });
          return;
        }
      }
    };
    void load().catch(() => {
      if (active && gen === loadGenRef.current) setContext({ id: current.id, rows: [current], error: true });
    });
    return () => { active = false; };
  }, [entry?.id, entry?.file_path, projectPath, databasePath, format]);

  useEffect(() => {
    if (entry) setTranslation(entry.translation || "");
  }, [entry?.id]);

  const renderedGen = loadGenRef.current;
  const handleApprove = useCallback(async () => {
    if (renderedGen !== loadGenRef.current) return;
    if (approvingRef.current || !projectKey || !isProjectScopedQueryEnabled(projectKey)) return;
    const state = queueRef.current;
    const id = state.items[state.index]?.id;
    const current = id ? byId[id] : undefined;
    if (!current || state.complete) return;
    approvingRef.current = true;
    setApproving(true);
    const gen = loadGenRef.current;
    try {
      const patch = reviewApprovalPatch(current, translation);
      if (patch) {
        const saved = await patchString(current.id, patch);
        // Defer reads until Editor remounts; never refetch a different open database.
        void invalidateProjectData(qc, projectKey, "none");
        if (gen !== loadGenRef.current || !isProjectScopedQueryEnabled(projectKey)) return;
        setById((prev) => ({ ...prev, [current.id]: saved }));
      } else {
        void invalidateProjectData(qc, projectKey, "none");
      }
      setLoadError(null);
      const next = approveCurrent(queueRef.current, current.id);
      queueRef.current = next;
      setQueue(next);
    } catch (err: unknown) {
      if (gen === loadGenRef.current) {
        setLoadError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      approvingRef.current = false;
      setApproving(false);
    }
  }, [byId, translation, qc, projectKey, renderedGen]);

  const handleSkip = useCallback(() => {
    if (approvingRef.current) return;
    const next = skipCurrent(queueRef.current);
    queueRef.current = next;
    setQueue(next);
  }, []);

  const handlePrev = useCallback(() => {
    if (approvingRef.current) return;
    const next = prevCurrent(queueRef.current);
    queueRef.current = next;
    setQueue(next);
  }, []);

  const handleExit = useCallback(() => {
    if (approvingRef.current) return;
    navigate("/editor");
  }, [navigate]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (approvingRef.current) return;
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

  const scopePicker = (
    <div className="flex flex-wrap items-center gap-2 text-body">
      <label htmlFor="review-file-scope" className="font-medium">{t("review.scope.label")}</label>
      <select
        id="review-file-scope"
        value={filePath}
        disabled={approving}
        onChange={event => changeScope(event.target.value)}
        className="min-w-0 max-w-full rounded border border-border bg-surface px-2 py-1 text-text disabled:opacity-50"
      >
        <option value="">{t("review.scope.all")}</option>
        {filePath && files === null && <option value={filePath}>{filePath}</option>}
        {files?.map(file => (
          <option key={file.file_path} value={file.file_path}>
            {file.file_path} ({file.translated + file.reviewed})
          </option>
        ))}
      </select>
      {fileStatsError && <>
        <span role="alert" className="text-danger">{t("review.scope.loadError")}</span>
        <button onClick={() => setFileStatsRevision(value => value + 1)} className="text-accent-fg hover:underline">
          {t("common.retry")}
        </button>
      </>}
    </div>
  );

  if (!project) {
    return (
      <div className="flex h-full flex-col bg-surface text-text">
        <header className="border-b border-border px-6 py-4">
          <h1 className="text-page font-bold">{t("nav.review")}</h1>
        </header>
        <EmptyState
          title={t("editor.empty.title")}
          description={t("editor.empty.description")}
          actionLabel={t("common.goHome")}
          actionTo="/"
        />
      </div>
    );
  }

  if (initialLoading) {
    return (
      <div className="flex h-full flex-col bg-surface text-text">
        <div className="border-b border-border p-4">{scopePicker}</div>
        <div className="flex flex-1 items-center justify-center text-text-muted">{t("review.loading")}</div>
      </div>
    );
  }

  if (loadError && !queue.bootstrapped) {
    return (
      <div className="flex h-full flex-col bg-surface text-text">
        <div className="border-b border-border p-4">{scopePicker}</div>
        <div className="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
          <h1 className="text-page font-semibold text-danger">{t("review.loadError")}</h1>
          <p className="text-body text-text-muted">
            {loadError || t("common.tryAgain")}
          </p>
          <button
            onClick={() => {
              resetSession();
              void loadMore();
            }}
            className="rounded bg-accent px-4 py-2 font-medium text-white hover:bg-accent-hover"
          >
            {t("common.retry")}
          </button>
        </div>
      </div>
    );
  }

  const showComplete = queue.complete && progress.total > 0;
  const showNothing = queue.bootstrapped && progress.total === 0;

  if (showComplete || showNothing) {
    return (
      <div className="flex h-full flex-col bg-surface text-text">
        <div className="border-b border-border p-4">{scopePicker}</div>
        <div className="flex flex-1 flex-col items-center justify-center px-6 text-center">
          <h1 className="text-page font-semibold text-text">
            {showComplete ? t("review.complete") : t("review.nothing")}
          </h1>
          <p className="mt-2 mb-4 text-text-muted">
            {showComplete
              ? t("review.completeHint")
              : t("review.nothingHint")}
          </p>
          <button
            onClick={() => navigate("/editor")}
            className="rounded bg-accent px-4 py-2 font-medium text-white hover:bg-accent-hover"
          >
            {t("review.returnToEditor")}
          </button>
        </div>
      </div>
    );
  }

  const bar = progress.total > 0 ? (progress.current / progress.total) * 100 : 0;
  const sourceParts = placeholderParts(entry?.source ?? "", format);
  const sourceTokens = sourceParts.filter(part => part.token).map(part => part.text);
  const check = placeholderCheck(sourceTokens, placeholderParts(translation, format).filter(part => part.token).map(part => part.text));
  const placeholdersValid = !check.missing.length && !check.extra.length;
  const contextReady = context?.id === entryId;

  return (
    <div className="flex flex-col h-full bg-surface text-text">
      {/* Header */}
      <div className="p-4 border-b border-border space-y-2">
        <div className="flex justify-between items-center">
          <span className="text-body font-medium inline-flex items-center gap-3">
            {t("review.progress", {
              current: progress.current,
              total: progress.total,
              approved: progress.approved,
            })}
            {entry && <StatusBadge status={entry.status} />}
          </span>
          <button onClick={handleExit} disabled={approving} className="text-body text-text-muted hover:text-text flex items-center gap-1 disabled:opacity-50">
            <X size={14} /> {t("common.exit")}
          </button>
        </div>
        {scopePicker}
        <div className="w-full bg-border rounded-full h-2">
          <div className={`${statusStyles.approved.bar} h-2 rounded-full transition-all`} style={{ width: `${bar}%` }} />
        </div>
      </div>

      {loadError && (
        <p role="alert" className="px-6 pt-4 text-body text-danger">{loadError}</p>
      )}

      {/* Content */}
      {entry && (
        <div className="flex-1 min-h-0 overflow-y-auto p-6">
          <div className="max-w-5xl mx-auto w-full grid grid-cols-1 min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(0,1fr)] items-start gap-6">
            <section className="min-w-0 rounded-lg border border-border p-5 space-y-5">
              <div className="text-caption text-text-muted flex flex-wrap items-center gap-3">
                {speakers[entry.id] && <span data-speaker={speakers[entry.id]} className="rounded bg-accent-muted px-2 py-1 font-semibold text-accent-fg">{speakers[entry.id]}</span>}
                <span title={entry.file_path} className="min-w-0 flex-1 truncate">{pathBasename(entry.file_path)}</span>
                {entry.tags.map((tag) => (
                  <span key={tag} className="px-1.5 py-0.5 bg-surface-muted rounded">{tag}</span>
                ))}
              </div>
              {entry.context && <p className="text-caption text-text-muted break-words">{t("review.context", { context: entry.context })}</p>}

              <div>
                <h3 className="text-caption font-semibold text-text-muted uppercase mb-1">{t("review.source")}</h3>
                <div className="p-3 rounded text-body leading-relaxed whitespace-pre-wrap break-words select-all">
                  {sourceParts.map((part, index) => part.token
                    ? <span key={index} data-placeholder={part.text} className="rounded bg-surface-muted px-1 text-caption font-mono">{part.text}</span>
                    : part.text)}
                </div>
              </div>

              <div>
                <div className="flex justify-between items-center mb-1">
                  <h3 className="text-caption font-semibold text-text-muted uppercase">{t("review.translation")}</h3>
                  <button onClick={() => setShowDiff(!showDiff)} className="text-caption text-accent-fg hover:underline">
                    {showDiff ? t("review.hideDiff") : t("review.showDiff")}
                  </button>
                </div>
                <textarea
                  id="review-textarea"
                  value={translation}
                  disabled={approving}
                  onChange={(e) => setTranslation(e.target.value)}
                  aria-label={t("review.translation")}
                  className="w-full p-3 border border-border rounded bg-surface text-body leading-relaxed focus:outline-none focus:ring-2 focus:ring-accent-fg resize-y min-h-[100px]"
                  rows={4}
                />
                <p role="status" className={`mt-2 text-caption break-words ${placeholdersValid ? "text-success" : "text-warning"}`}>
                  {placeholdersValid
                    ? t("review.placeholdersPresent", { present: check.present, total: sourceTokens.length })
                    : t("review.placeholdersWarning", { details: [
                      check.missing.length ? t("review.placeholdersMissing", { tokens: check.missing.join(", ") }) : "",
                      check.extra.length ? t("review.placeholdersExtra", { tokens: check.extra.join(", ") }) : "",
                    ].filter(Boolean).join("; ") })}
                </p>
                {entry.char_limit != null && (
                  <div className={`text-caption mt-1 ${translation.length > entry.char_limit ? "text-danger" : "text-text-muted"}`}>
                    {t("review.chars", { count: translation.length, limit: entry.char_limit })}
                  </div>
                )}
              </div>

              {showDiff && entry.translation && (
                <DiffView originalText={entry.source} translatedText={translation} entryId={entry.id} />
              )}
            </section>
            <aside aria-label={t("review.contextTitle")} className="min-w-0 rounded-lg border border-border p-4">
              <h2 className="mb-3 text-body font-semibold">{t("review.contextTitle")}</h2>
              {!contextReady && <p className="mb-3 text-caption text-text-muted">{t("review.contextLoading")}</p>}
              {contextReady && context?.error && <p className="mb-3 text-caption text-warning">{t("review.contextLoadError")}</p>}
              <ol className="space-y-2">
                {(contextReady ? context!.rows : [entry]).map(row => {
                  const current = row.id === entry.id;
                  const latest = byId[row.id] ?? row;
                  return <li key={row.id} data-entry-id={row.id} aria-current={current || undefined}
                    className={`border-l-2 rounded-r px-3 py-2 text-body ${current ? "border-accent bg-accent-muted" : "border-transparent"}`}>
                    {speakers[row.id] && <strong className="block text-caption text-text-muted">{speakers[row.id]}</strong>}
                    <p className="line-clamp-2 break-words leading-relaxed">{latest.source}</p>
                    <p className="mt-1 line-clamp-2 break-words text-caption text-text-muted">{current ? translation : latest.translation}</p>
                  </li>;
                })}
              </ol>
            </aside>
          </div>
        </div>
      )}

      {/* Bottom bar */}
      <div className="p-4 border-t border-border flex justify-center gap-3">
        <button onClick={handlePrev} disabled={approving || queue.index === 0}
          className="flex items-center gap-1.5 px-4 py-2 bg-surface-muted hover:bg-surface rounded text-body font-medium disabled:opacity-50">
          <ArrowLeft size={16} /> {t("review.previous")} <kbd className="text-caption text-text-muted ml-1">P</kbd>
        </button>
        <button onClick={handleSkip} disabled={approving}
          className="flex items-center gap-1.5 px-4 py-2 bg-surface-muted hover:bg-surface rounded text-body font-medium disabled:opacity-50">
          <SkipForward size={16} /> {t("review.skip")} <kbd className="text-caption text-text-muted ml-1">S</kbd>
        </button>
        <button onClick={() => { void handleApprove(); }} disabled={approving}
          className="flex items-center gap-1.5 px-6 py-2 bg-accent hover:bg-accent-hover text-white rounded text-body font-medium disabled:opacity-50">
          <Check size={16} /> {t("review.approve")} <kbd className="text-caption text-white/70 ml-1">A</kbd>
        </button>
      </div>
    </div>
  );
}
