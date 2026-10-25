/**
 * Module-level translation job session. Survives TranslationModal close so
 * progress, toasts, and BottomBar stay live. Reopen must not double-subscribe.
 */
import { cancelTranslation } from "./api";
import { localizeApiError } from "./apiError";
import { t } from "./i18n";
import {
  formatObservedCost,
} from "./translationCost";
import { shouldSubscribeToJob } from "./translationJob";
import { translationWriteProjectKey } from "./translationWrites";
import { subscribeToJob, type JobHandlers } from "./ws";
import { useEditorStore } from "../stores/editorStore";
import { addLog } from "../stores/logStore";
import { useQueueStore, type GlobalProgress } from "../stores/queueStore";
import { addToast } from "../stores/toastStore";

let unsub: (() => void) | null = null;
let subscribedJob: { jobId: string; projectKey: string } | null = null;
let finished = false;
let cancelRequested = false;
let modalOpen = false;
let retryTimer: ReturnType<typeof setTimeout> | null = null;
let retrySubscription: (() => void) | null = null;
let subscriptionVersion = 0;
const RECONNECT_DELAYS = [1000, 2000, 4000];

export function hasUnresolvedBatchFailures(failedBatches: number, translated: number, total: number): boolean {
  // Fallback providers can recover failed batches; an unknown total cannot prove recovery.
  return failedBatches > 0 && !(total > 0 && translated >= total);
}

export function setTranslationModalOpen(open: boolean): void {
  modalOpen = open;
}

export function subscribedTranslationJobId(): string | null {
  return subscribedJob?.jobId ?? null;
}

function patchSnapshot(
  patch: Partial<NonNullable<ReturnType<typeof useEditorStore.getState>["jobSnapshot"]>>,
): void {
  useEditorStore.getState().patchJobSnapshot(patch);
}

function stopSubscription(): void {
  subscriptionVersion += 1;
  if (retryTimer !== null) clearTimeout(retryTimer);
  retryTimer = null;
  const close = unsub;
  unsub = null;
  close?.();
}

function endJob(): void {
  stopSubscription();
  retrySubscription = null;
  patchSnapshot({ disconnected: false });
  useEditorStore.getState().setTranslating(false);
  useEditorStore.getState().setJob(null);
  const queue = useQueueStore.getState();
  if (queue.globalProgress?.owner.kind === "single" && queue.globalProgress.owner.jobId === subscribedJob?.jobId) {
    queue.setGlobalProgress(null);
  }
  subscribedJob = null;
}

function discardSnapshotIfModalClosed(): void {
  if (!modalOpen) {
    useEditorStore.getState().setJobSnapshot(null);
  }
}

function finishCancelledJob(): void {
  patchSnapshot({ cancelled: true, cancelling: false, error: null });
  addLog("info", t("activity.translation.cancelled"), undefined, "translation");
  addToast("info", t("translate.toast.cancelled"));
  endJob();
  discardSnapshotIfModalClosed();
}

export function attachTranslationJob(opts: {
  jobId: string;
  projectName: string;
  providerLabel: string;
}): void {
  if (!shouldSubscribeToJob(opts.jobId, subscribedJob?.jobId ?? null)) {
    return;
  }
  stopSubscription();
  // Ownership belongs to the job session, not to whichever project is open on reconnect.
  const jobSession = { jobId: opts.jobId, projectKey: translationWriteProjectKey() };
  subscribedJob = jobSession;
  finished = false;
  cancelRequested = false;
  let batchFailures = { count: 0, lastReason: "" };
  let retryAttempt = 0;
  // The server replays the entire history. Keep occurrence counts across
  // subscriptions: replay cannot repeat progress, logs or batch failures, but
  // distinct failures with identical payloads still count separately. String
  // events only update the preview; server batch/terminal totals own progress.
  const received = new Map<string, number>();

  const editor = useEditorStore.getState();
  editor.setJob(opts.jobId);
  editor.setTranslating(true);
  editor.setJobSnapshot({
    completed: 0,
    total: 0,
    costSoFar: 0,
    lastTranslated: "",
    activeProviderLabel: opts.providerLabel,
    error: null,
    done: false,
    cancelled: false,
    cancelling: false,
    disconnected: false,
  });

  const publishProgress = (progress: Omit<GlobalProgress, "owner" | "projectName">) => {
    const queue = useQueueStore.getState();
    const owner = queue.globalProgress?.owner;
    if (finished || subscribedJob !== jobSession || queue.isRunning) return;
    if (owner && (owner.kind !== "single" || owner.jobId !== opts.jobId)) return;
    queue.setGlobalProgress({
      ...progress,
      owner: { kind: "single", jobId: opts.jobId },
      projectName: opts.projectName,
    });
  };

  const handlers: JobHandlers = {
    onStarted: (e) => {
      patchSnapshot({ total: e.total, completed: 0, costSoFar: 0 });
      publishProgress({
        completed: 0,
        total: e.total,
        costSoFar: 0,
        startedAt: Date.now(),
      });
    },
    onBatchCompleted: (e) => {
      patchSnapshot({
        completed: e.completed,
        total: e.total,
        costSoFar: e.cost_so_far,
        costIsComplete: e.cost_is_complete === true,
      });
      publishProgress({
        completed: e.completed,
        total: e.total,
        costSoFar: e.cost_so_far,
        costIsComplete: e.cost_is_complete === true,
        startedAt:
          useQueueStore.getState().globalProgress?.startedAt ?? Date.now(),
      });
    },
    onStringTranslated: (e) => {
      patchSnapshot({ lastTranslated: e.translation });
    },
    onProviderSwitched: (e) => {
      patchSnapshot({ activeProviderLabel: e.provider_name });
      addLog(
        "info",
        t("activity.translation.providerSwitched", { name: e.provider_name, count: e.remaining_pending }),
        undefined,
        "translation",
      );
      addToast("info", t("translate.toast.switched", { name: e.provider_name }));
    },
    onCompleted: (e) => {
      if (finished) return;
      finished = true;
      // The final pending count is authoritative across provider fallbacks.
      // Older servers omit it, so retain the progress-total inference for them.
      const pending = (e.remaining ?? 0) > 0;
      const pendingGuidance = pending ? ` ${t("translate.pendingRetryHint")}` : "";
      const unresolvedFailures = e.remaining !== undefined
        ? batchFailures.count > 0 && pending
        : hasUnresolvedBatchFailures(
          batchFailures.count, e.total_translated, useEditorStore.getState().jobSnapshot?.total ?? 0,
        );
      const totalFailure = unresolvedFailures && e.total_translated === 0;
      patchSnapshot({
        done: !totalFailure,
        error: totalFailure ? batchFailures.lastReason : null,
        batchFailures: unresolvedFailures ? batchFailures : undefined,
        cancelling: false,
        completed: e.total_translated,
        costSoFar: e.total_cost,
        costIsComplete: e.cost_is_complete === true,
      });
      if (unresolvedFailures) {
        const summary = t("translate.completedWithErrorsSummary", {
          translated: e.total_translated, count: batchFailures.count,
        }) + pendingGuidance;
        // Each original diagnostic already has its own batch log entry.
        addLog(totalFailure ? "error" : "warning", totalFailure ? t("activity.translation.failed") + pendingGuidance : summary, undefined, "translation");
        addToast(totalFailure ? "error" : "warning", totalFailure
          ? t("translate.toast.failed", { error: batchFailures.lastReason }) + pendingGuidance
          : summary);
      } else {
        addLog(
          pending ? "warning" : "info",
          t("activity.translation.completed", { count: e.total_translated, cost: formatObservedCost(e.total_cost, e.cost_is_complete, t) }) + pendingGuidance,
          undefined,
          "translation",
        );
        const cost = e.total_cost ?? 0;
        addToast(
          pending ? "warning" : "success",
          t("translate.toast.completeObservedCost", {
            count: e.total_translated,
            cost: formatObservedCost(cost, e.cost_is_complete, t),
          }) + pendingGuidance,
        );
      }
      endJob();
      discardSnapshotIfModalClosed();
    },
    onBatchFailed: (e) => {
      if (finished) return;
      const reason = localizeApiError(e.error) || t("activity.translation.failed");
      batchFailures = { count: batchFailures.count + 1, lastReason: reason };
      patchSnapshot({ batchFailures });
      // Unknown diagnostics stay in detail, without repeating them in the summary.
      const summary = reason === e.error.trim()
        ? t("activity.translation.batchFailed")
        : t("activity.translation.batchFailedReason", { error: reason });
      addLog("warning", summary, e.error, "translation");
    },
    onFailed: (e) => {
      if (finished) return;
      finished = true;
      // The server replays cancellation as this stable terminal event, also
      // when another client requested it or the socket reconnected afterward.
      if (e.error === "cancelled") {
        finishCancelledJob();
        return;
      }
      const reason = localizeApiError(e.error);
      patchSnapshot({ error: reason, cancelling: false });
      addLog("error", t("activity.translation.failed"), e.error, "translation");
      addToast("error", t("translate.toast.failed", { error: reason }));
      endJob();
      discardSnapshotIfModalClosed();
    },
  };

  const connect = () => {
    stopSubscription();
    const version = subscriptionVersion;
    const current = () => !finished && subscribedJob === jobSession && version === subscriptionVersion;
    let closed = false;
    const replayed = new Map<string, number>();
    const receive = <E,>(handler: (event: E) => void, terminal = false) => (event: E) => {
      if (!current() || (closed && !terminal)) return;
      patchSnapshot({ disconnected: false });
      const key = JSON.stringify(event);
      const count = (replayed.get(key) ?? 0) + 1;
      replayed.set(key, count);
      if (count <= (received.get(key) ?? 0)) return;
      received.set(key, count);
      // Only new events replenish the budget. Repeated opens/replays followed
      // by a close must not create an unbounded reconnect loop.
      retryAttempt = 0;
      handler(event);
    };
    unsub = subscribeToJob(opts.jobId, {
      onOpen: () => {
        if (current() && !closed) patchSnapshot({ disconnected: false });
      },
      onStarted: receive(handlers.onStarted!),
      onBatchCompleted: receive(handlers.onBatchCompleted!),
      onStringTranslated: receive(handlers.onStringTranslated!),
      onProviderSwitched: receive(handlers.onProviderSwitched!),
      onBatchFailed: receive(handlers.onBatchFailed!),
      onCompleted: receive(handlers.onCompleted!, true),
      onFailed: receive(handlers.onFailed!, true),
      onClosed: () => {
        if (!current() || closed) return;
        closed = true;
        // Preserve the existing close acknowledgement for requested cancels.
        if (cancelRequested) {
          finished = true;
          finishCancelledJob();
          return;
        }
        // A terminal result already in flight remains authoritative until a
        // new subscription takes over. Ignore further closes and progress.
        const close = unsub;
        unsub = null;
        close?.();
        patchSnapshot({ disconnected: true });
        const delay = RECONNECT_DELAYS[retryAttempt++];
        if (delay !== undefined) retryTimer = setTimeout(connect, delay);
      },
    }, "translate", jobSession.projectKey);
  };
  retrySubscription = () => {
    retryAttempt = 0;
    connect();
  };
  connect();
}

export function reconnectTranslationJob(): void {
  if (finished || unsub || !useEditorStore.getState().jobSnapshot?.disconnected) return;
  retrySubscription?.();
}

export function markTranslationCancelRequested(): void {
  cancelRequested = true;
  patchSnapshot({ cancelling: true });
}

export function clearTranslationCancelRequested(): void {
  cancelRequested = false;
  patchSnapshot({ cancelling: false });
}

export async function requestTranslationCancel(): Promise<void> {
  const jobId = useEditorStore.getState().jobId;
  if (!jobId || jobId !== subscribedJob?.jobId || finished || cancelRequested) return;
  markTranslationCancelRequested();
  try {
    await cancelTranslation(jobId);
    if (finished || subscribedJob?.jobId !== jobId) return;
    addLog("info", t("activity.translation.cancelRequested", { jobId }), undefined, "translation");
    addToast("info", t("translate.toast.cancelling"));
    // Cancellation remains available after retries are exhausted; subscribe
    // again to receive the server's terminal cancellation replay.
    reconnectTranslationJob();
  } catch (err: unknown) {
    if (finished || subscribedJob?.jobId !== jobId) return;
    clearTranslationCancelRequested();
    const message = err instanceof Error ? err.message : String(err);
    addToast("error", t("translate.toast.cancelFailed", { error: message }));
  }
}

export function clearTranslationSnapshotIfIdle(): void {
  const { isTranslating } = useEditorStore.getState();
  if (!isTranslating) {
    useEditorStore.getState().setJobSnapshot(null);
  }
}
