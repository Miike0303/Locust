import { create } from "zustand";
import { startTranslation, cancelTranslation, validate, type TranslationStartParams } from "../lib/api";
import { completeOpenProject, projectFromOpenResponse } from "../lib/openProjectFlow";
import { invalidateProjectData, queryClient } from "../lib/queryClient";
import { draftProjectKey } from "../lib/projectScope";
import { waitForJob, subscribeToJob, type JobWait } from "../lib/ws";
import { useProjectStore } from "./projectStore";
import { useEditorStore } from "./editorStore";
import { canStartQueue } from "../lib/translationJob";
import { createTranslationJobConnection, hasUnresolvedBatchFailures } from "../lib/translationJobSession";
import { addLog } from "./logStore";
import { addToast } from "./toastStore";
import { t } from "../lib/i18n";
import { ApiError, localizeApiError, parseApiError } from "../lib/apiError";
import { JOB_STREAM_LOST_MESSAGE } from "../lib/ws";
import { MAX_QUEUE_HISTORY, readQueue, writeQueue, type QueueHistoryEntry, type QueueJobOwner } from "../lib/queuePersistence";
import {
  queueItemPatchAfterValidation,
  validationIssueCount,
} from "../lib/queueValidation";

// Unlike a transport failure, a missing server job has no terminal replay to await.
class QueueJobUnavailableError extends Error {}

let activeJobWait: JobWait | null = null;
let activeOpening: AbortController | null = null;

export type QueueItemStatus = "pending" | "extracting" | "translating" | "validating" | "done" | "error" | "cancelled";

export interface QueueItem {
  id: string;
  projectPath: string;
  projectName: string;
  formatId: string | null;
  status: QueueItemStatus;
  progress: { completed: number; total: number; costSoFar: number; costIsComplete?: boolean; startedAt: number | null };
  error: string | null;
  /** Set after a successful validation run. Null if validation did not run. */
  validationIssues: number | null;
  /** Validation RPC failed; translation still succeeded. */
  validationError: string | null;
}

export interface GlobalProgress {
  owner: { kind: "queue"; itemId: string } | { kind: "single"; jobId: string };
  projectName: string;
  completed: number;
  total: number;
  costSoFar: number;
  costIsComplete?: boolean;
  startedAt: number | null;
  queuePosition?: number;
  queueTotal?: number;
}

interface QueueStore {
  items: QueueItem[];
  history: QueueHistoryEntry[];
  isRunning: boolean;
  isPanelOpen: boolean;
  globalProgress: GlobalProgress | null;
  translationParams: TranslationStartParams | null;
  cancelRequested: boolean;
  activeJobId: string | null;
  disconnected: boolean;
  activeJob: QueueJobOwner | null;
  persistenceError: boolean;
  recoveryAvailable: boolean;
  retrySave: () => void;

  addItem: (path: string) => void;
  removeItem: (id: string) => void;
  moveItem: (id: string, direction: "up" | "down") => void;
  clearCompleted: () => void;
  retryItem: (id: string) => void;
  retryFailed: () => void;
  clearHistory: () => void;
  setParams: (p: TranslationStartParams) => void;
  setPanelOpen: (v: boolean) => void;
  setGlobalProgress: (p: GlobalProgress | null) => void;
  startQueue: () => Promise<void>;
  cancelQueue: () => void;
  reconnectJob: () => void;
}

const retryable = (item: QueueItem) => item.status === "error" || item.status === "cancelled";
const resetForRetry = (item: QueueItem): QueueItem => ({
  ...item, status: "pending", error: null, validationIssues: null, validationError: null,
  progress: { completed: 0, total: 0, costSoFar: 0, costIsComplete: false, startedAt: null },
});

const restoredQueue = readQueue();
const restoredOwner = restoredQueue.activeJob ?? null;
const restoredItem = restoredQueue.items.find(i => i.id === restoredOwner?.itemId);

export const useQueueStore = create<QueueStore>((set, get) => ({
  ...restoredQueue,
  activeJob: restoredOwner,
  persistenceError: false,
  recoveryAvailable: restoredQueue.recoveryAvailable === true,
  retrySave: () => saveQueue(),
  isRunning: restoredOwner !== null,
  isPanelOpen: false,
  globalProgress: restoredItem ? { owner: { kind: "queue", itemId: restoredItem.id },
    projectName: restoredItem.projectName, ...restoredItem.progress } : null,
  cancelRequested: false,
  activeJobId: restoredOwner?.jobId ?? null,
  disconnected: false,

  addItem: (path) => {
    const name = path.split(/[\\/]/).filter(Boolean).pop() ?? path;
    const item: QueueItem = {
      id: crypto.randomUUID(),
      projectPath: path,
      projectName: name,
      formatId: null,
      status: "pending",
      progress: { completed: 0, total: 0, costSoFar: 0, startedAt: null },
      error: null,
      validationIssues: null,
      validationError: null,
    };
    set((s) => ({ items: [...s.items, item] }));
  },

  removeItem: (id) => set((s) => ({ items: s.items.filter((i) => i.id !== id) })),

  moveItem: (id, direction) => set((s) => {
    const idx = s.items.findIndex((i) => i.id === id);
    if (idx < 0) return s;
    const swap = direction === "up" ? idx - 1 : idx + 1;
    if (swap < 0 || swap >= s.items.length) return s;
    const items = [...s.items];
    [items[idx], items[swap]] = [items[swap], items[idx]];
    return { items };
  }),

  clearCompleted: () => set((s) => ({ items: s.items.filter((i) => i.status !== "done") })),

  retryItem: (id) => set((s) => {
    if (s.isRunning || !s.items.some(i => i.id === id && retryable(i))) return s;
    return { items: s.items.map(i => i.id === id && retryable(i) ? resetForRetry(i) : i) };
  }),
  retryFailed: () => set((s) => {
    if (s.isRunning || !s.items.some(retryable)) return s;
    return { items: s.items.map(i => retryable(i) ? resetForRetry(i) : i) };
  }),
  clearHistory: () => set({ history: [] }),

  setParams: (translationParams) => set({ translationParams }),
  setPanelOpen: (isPanelOpen) => set({ isPanelOpen }),
  setGlobalProgress: (globalProgress) => set({ globalProgress }),

  reconnectJob: () => { activeJobWait?.reconnect(); },

  cancelQueue: () => {
    set({ cancelRequested: true });
    activeOpening?.abort();
    const jobId = get().activeJobId;
    if (!jobId) return;
    void cancelTranslation(jobId).then(() => {
      // Keep ownership until the terminal replay acknowledges cancellation.
      if (get().activeJobId === jobId) get().reconnectJob();
    }).catch((err: unknown) => {
      const message = err instanceof Error ? err.message : String(err);
      addLog("error", t("activity.queue.cancelFailed"), message, "queue");
      if (get().activeJobId === jobId && jobIsMissing(err)) {
        activeJobWait?.abandon(new QueueJobUnavailableError(t("queue.jobOutcomeUnknown")));
      }
    });
  },

  startQueue: async () => {
    const { items, translationParams, isRunning } = get();
    const singleJobRunning = useEditorStore.getState().isTranslating;
    if (!canStartQueue({ queueRunning: isRunning, singleJobRunning })) {
      if (!isRunning) addToast("info", t("queue.toast.singleJobRunning"));
      return;
    }
    if (!translationParams) {
      addToast("error", t("queue.toast.configureFirst"));
      return;
    }

    set({ isRunning: true, cancelRequested: false, activeJobId: null, disconnected: false });
    const pending = items.filter((i) => i.status === "pending");
    const partialItemIds = new Set<string>();
    addLog("info", t("activity.queue.started", { count: pending.length }), undefined, "queue");

    for (let idx = 0; idx < pending.length; idx++) {
      if (get().cancelRequested) break;

      const item = pending[idx];
      const updateItem = (patch: Partial<QueueItem>) => patchQueueItem(item.id, patch);
      const markCancelled = () => {
        const name = get().items.find((i) => i.id === item.id)?.projectName ?? item.projectName;
        updateItem({ status: "cancelled", error: null });
        addLog("info", t("activity.queue.itemCancelled", { name }), undefined, "queue");
        addToast("info", t("queue.toast.itemCancelled", { name }));
      };

      let jobProjectKey: string | null = null;
      try {
        // Step 1: Open project
        updateItem({ status: "extracting" });
        addLog("info", t("activity.queue.opening", { path: item.projectPath }), undefined, "queue");
        set({
          globalProgress: {
            owner: { kind: "queue", itemId: item.id },
            projectName: item.projectName,
            completed: 0,
            total: 0,
            costSoFar: 0,
            startedAt: null,
            queuePosition: idx + 1,
            queueTotal: pending.length,
          },
        });

        const opening = new AbortController();
        activeOpening = opening;
        const result = await completeOpenProject(item.projectPath, undefined, {
          setProject: useProjectStore.getState().setProject,
          queryClient,
          signal: opening.signal,
        }).finally(() => { activeOpening = null; });
        if (!result) {
          markCancelled();
          if (get().cancelRequested) break;
          continue;
        }
        updateItem({
          projectName: result.project_name,
          formatId: result.format_id,
        });

        if (get().cancelRequested) {
          markCancelled();
          break;
        }

        // Step 2: Start translation
        updateItem({
          status: "translating",
          progress: { completed: 0, total: result.total_strings, costSoFar: 0, startedAt: Date.now() },
        });
        set((s) => ({
          globalProgress: {
            ...s.globalProgress!,
            total: result.total_strings,
            startedAt: Date.now(),
          },
        }));

        const job = await startTranslation(translationParams);
        jobProjectKey = draftProjectKey(projectFromOpenResponse(result));
        set({ activeJobId: job.job_id, activeJob: { jobId: job.job_id, itemId: item.id, projectKey: jobProjectKey } });
        let failedBatches = 0;
        let lastBatchReason = "";
        let translated = 0;
        const updateProgress = (completed: number, total: number, costSoFar: number, costIsComplete: boolean) => {
          updateItem({ progress: { completed, total, costSoFar, costIsComplete, startedAt: get().items.find((i) => i.id === item.id)?.progress.startedAt ?? null } });
          set((s) => ({
            globalProgress: s.globalProgress
              ? { ...s.globalProgress, completed, total, costSoFar, costIsComplete }
              : null,
          }));
        };
        try {
          if (get().cancelRequested) get().cancelQueue();
          // Step 3: Wait for completion
          const waiting = waitForJob(job.job_id, {
            onConnectionChange: disconnected => set({ disconnected }),
            onProgress: updateProgress,
            onBatchFailed: (e) => {
              failedBatches++;
              lastBatchReason = localizeApiError(e.error) || t("activity.translation.failed");
              const summary = lastBatchReason === e.error.trim()
                ? t("activity.translation.batchFailed")
                : t("activity.translation.batchFailedReason", { error: lastBatchReason });
              addLog("warning", summary, e.error, "queue");
            },
            onCompleted: (e) => {
              translated = e.total_translated;
              updateProgress(translated,
                get().items.find((i) => i.id === item.id)?.progress.total ?? result.total_strings,
                e.total_cost, e.cost_is_complete === true);
            },
          });
          activeJobWait = waiting;
          await waiting;
        } finally {
          activeJobWait = null;
          set({ activeJobId: null, activeJob: null, disconnected: false });
        }

        if (get().cancelRequested) {
          markCancelled();
          break;
        }

        const total = get().items.find((i) => i.id === item.id)?.progress.total ?? 0;
        if (hasUnresolvedBatchFailures(failedBatches, translated, total)) {
          // Use the terminal translated count, not interim fallback progress.
          const partial = translated > 0;
          if (partial) partialItemIds.add(item.id);
          const summary = t(partial ? "queue.completedWithErrors" : "queue.translationFailed", {
            name: result.project_name, translated, count: failedBatches,
          });
          // Reuse the existing error row so incomplete jobs stay visible and
          // validation cannot overwrite their translation outcome with "done".
          updateItem({ status: "error", error: `${summary}. ${lastBatchReason} ${t("translate.pendingRetryHint")}` });
          addLog(partial ? "warning" : "error", summary, undefined, "queue");
          addToast(partial ? "warning" : "error", summary);
          continue;
        }

        updateItem({ status: "validating" });
        try {
          const report = await validate();
          const issuesFound = validationIssueCount(report);
          updateItem(queueItemPatchAfterValidation({ ok: true, issuesFound }));
          if (issuesFound > 0) {
            addLog(
              "warning",
              t("activity.queue.completedWithIssues", {
                name: result.project_name,
                count: result.total_strings,
                issues: t("activity.count.validationIssues", { count: issuesFound }),
              }),
              undefined,
              "queue",
            );
            addToast(
              "warning",
              t("queue.toast.itemIssues", { name: result.project_name, count: issuesFound }),
            );
          } else {
            addLog("info", t("activity.queue.completed", { name: result.project_name, count: result.total_strings }), undefined, "queue");
            addToast("success", t("queue.toast.itemDone", { name: result.project_name }));
          }
        } catch (valErr: unknown) {
          const raw = valErr instanceof Error ? valErr.message : String(valErr);
          updateItem(queueItemPatchAfterValidation({ ok: false, error: raw }));
          addLog(
            "warning",
            t("activity.queue.validationFailed", { name: result.project_name }),
            raw,
            "queue",
          );
          addToast("warning", t("queue.toast.validationFailed", { name: result.project_name }));
        }
      } catch (err: any) {
        if (get().cancelRequested && !(err instanceof QueueJobUnavailableError) && !jobIsMissing(err)) {
          markCancelled();
        } else {
          const raw = err.message ?? String(err);
          updateItem({
            status: "error",
            error: jobIsMissing(err) ? t("queue.jobOutcomeUnknown") : raw === JOB_STREAM_LOST_MESSAGE ? t(JOB_STREAM_LOST_MESSAGE) : localizeApiError(raw),
          });
          addLog("error", t("activity.queue.itemFailed", { name: item.projectName }), raw, "queue");
          addToast("error", t("queue.toast.itemFailed", { name: item.projectName }));
        }
      } finally {
        if (jobProjectKey !== null) {
          const currentProject = useProjectStore.getState().project;
          const stillOpen = currentProject && draftProjectKey(currentProject) === jobProjectKey;
          // Failed/cancelled jobs can write rows too. Refresh after validation,
          // awaiting live-project reads before the next item switches the backend.
          await invalidateProjectData(queryClient, jobProjectKey, stillOpen ? "active" : "none");
        }
      }
    }

    set({ isRunning: false, globalProgress: null });
    if (get().cancelRequested) {
      addLog("info", t("activity.queue.cancelled"), undefined, "queue");
      addToast("info", t("queue.toast.cancelled"));
    } else {
      const runIds = new Set(pending.map((i) => i.id));
      let finished = 0;
      let failed = 0;
      let partial = 0;
      let cancelled = 0;
      for (const i of get().items) {
        if (!runIds.has(i.id)) continue;
        if (i.status === "done") finished++;
        else if (i.status === "error") {
          if (partialItemIds.has(i.id)) partial++;
          else failed++;
        } else if (i.status === "cancelled") cancelled++;
      }
      if (partial > 0) {
        const summary = t("queue.completedWithErrorsSummary", { finished, partial, failed, cancelled });
        addLog(failed > 0 ? "error" : "warning", summary, undefined, "queue");
        addToast(failed > 0 ? "error" : "warning", summary);
      } else if (failed === 0 && cancelled > 0) {
        addLog("info", t("activity.queue.finishedWithCancelled", { finished, cancelled }), undefined, "queue");
        addToast("info", t("queue.toast.finishedWithCancelled", { finished, cancelled }));
      } else if (failed === 0) {
        addLog("info", t("activity.queue.finished"), undefined, "queue");
        addToast("success", t("queue.toast.allDone"));
      } else {
        const summary = t("queue.toast.summary", { finished, failed });
        addLog("error", t("activity.queue.summary", {
          finished: t("activity.queue.finishedProjects", { count: finished }),
          failed: t("activity.queue.failedProjects", { count: failed }),
        }), undefined, "queue");
        addToast("error", summary);
      }
    }
  },
}));

function saveQueue(): void {
  const result = writeQueue(useQueueStore.getState());
  useQueueStore.setState({ persistenceError: !result.saved, recoveryAvailable: result.recoveryAvailable });
}

useQueueStore.subscribe((state, previous) => {
  if (state.items !== previous.items || state.translationParams !== previous.translationParams ||
    state.history !== previous.history || state.activeJob !== previous.activeJob) saveQueue();
});

// Defer until circular store/session imports and the persistence subscription are ready.
if (restoredOwner) queueMicrotask(() => { void resumeOwnedJob(restoredOwner); });

function jobIsMissing(error: unknown): boolean {
  const raw = error instanceof ApiError ? error.raw : error instanceof Error ? error.message : String(error);
  const { status, body } = parseApiError(raw);
  return status === 404 || (status === null && body.trim() === "job not found");
}

function patchQueueItem(itemId: string, patch: Partial<QueueItem>): void {
  useQueueStore.setState(s => {
    const current = s.items.find(i => i.id === itemId);
    if (!current) return s;
    const updated = { ...current, ...patch };
    const status = updated.status;
    const terminal = status === "done" || status === "error" || status === "cancelled";
    const entry: QueueHistoryEntry | null = s.isRunning && terminal && current.status !== status ? {
      id: crypto.randomUUID(), projectName: updated.projectName, projectPath: updated.projectPath,
      status, finishedAt: new Date().toISOString(), completed: updated.progress.completed,
      total: updated.progress.total, costSoFar: updated.progress.costSoFar,
      costIsComplete: updated.progress.costIsComplete === true, validationIssues: updated.validationIssues,
    } : null;
    return { items: s.items.map(i => i.id === itemId ? updated : i),
      ...(terminal && s.activeJob?.itemId === itemId ? { activeJob: null, activeJobId: null } : {}),
      history: entry ? [entry, ...s.history].slice(0, MAX_QUEUE_HISTORY) : s.history };
  });
}

/** Reuse W9's replay/reconnect transport with the persisted owner, never the current UI project. */
async function resumeOwnedJob(owner: QueueJobOwner): Promise<void> {
  const item = useQueueStore.getState().items.find(i => i.id === owner.itemId);
  if (!item) return;
  let failedBatches = 0;
  let lastBatchReason = "";
  let translated = 0;
  const updateProgress = (completed: number, total: number, costSoFar: number, costIsComplete: boolean) => {
    const current = useQueueStore.getState().items.find(i => i.id === item.id);
    if (!current) return;
    patchQueueItem(item.id, { progress: { ...current.progress, completed, total, costSoFar, costIsComplete } });
    useQueueStore.setState(s => ({ globalProgress: s.globalProgress
      ? { ...s.globalProgress, completed, total, costSoFar, costIsComplete } : null }));
  };
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  const connection = createTranslationJobConnection({
    subscribe: handlers => subscribeToJob(owner.jobId, handlers, "translate", owner.projectKey),
    onConnectionChange: disconnected => useQueueStore.setState({ disconnected }),
    handlers: {
      onBatchCompleted: e => updateProgress(e.completed, e.total, e.cost_so_far, e.cost_is_complete === true),
      onBatchFailed: e => { failedBatches++; lastBatchReason = localizeApiError(e.error); },
      onCompleted: e => {
        translated = e.total_translated;
        updateProgress(translated, useQueueStore.getState().items.find(i => i.id === item.id)?.progress.total ?? item.progress.total,
          e.total_cost, e.cost_is_complete === true);
        resolve();
      },
      onFailed: e => reject(jobIsMissing(e.error)
        ? new QueueJobUnavailableError(t("queue.jobOutcomeUnknown")) : new Error(e.error)),
    },
  });
  activeJobWait = Object.assign(promise, { reconnect: connection.reconnect,
    abandon: (error: Error) => { connection.stop(); reject(error); } });
  connection.connect();
  try {
    await activeJobWait;
    if (useQueueStore.getState().cancelRequested) patchQueueItem(item.id, { status: "cancelled", error: null });
    else {
      const total = useQueueStore.getState().items.find(i => i.id === item.id)?.progress.total ?? 0;
      if (hasUnresolvedBatchFailures(failedBatches, translated, total)) {
        const summary = t(translated > 0 ? "queue.completedWithErrors" : "queue.translationFailed", {
          name: item.projectName, translated, count: failedBatches,
        });
        patchQueueItem(item.id, { status: "error", error: `${summary}. ${lastBatchReason} ${t("translate.pendingRetryHint")}` });
      } else {
        // Validation uses the backend's currently open project. After reload we
        // only have ownership of this job, so do not validate another project.
        patchQueueItem(item.id, { status: "done", error: null });
      }
    }
  } catch (err) {
    const cancelled = useQueueStore.getState().cancelRequested && !(err instanceof QueueJobUnavailableError) && !jobIsMissing(err);
    const raw = err instanceof Error ? err.message : String(err);
    patchQueueItem(item.id, { status: cancelled ? "cancelled" : "error", error: cancelled ? null
      : err instanceof QueueJobUnavailableError || jobIsMissing(err) ? t("queue.jobOutcomeUnknown") : localizeApiError(raw) });
  } finally {
    connection.stop(); activeJobWait = null;
    useQueueStore.setState({ activeJob: null, activeJobId: null, disconnected: false });
    const project = useProjectStore.getState().project;
    await invalidateProjectData(queryClient, owner.projectKey, project && draftProjectKey(project) === owner.projectKey ? "active" : "none");
    // Pending items require an explicit start after reconciliation, including a missing job.
    useQueueStore.setState({ isRunning: false, globalProgress: null });
  }
}
