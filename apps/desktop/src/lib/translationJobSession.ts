/**
 * Module-level translation job session. Survives TranslationModal close so
 * progress, toasts, and BottomBar stay live. Reopen must not double-subscribe.
 */
import { cancelTranslation } from "./api";
import { t } from "./i18n";
import { shouldSubscribeToJob } from "./translationJob";
import { JOB_STREAM_LOST_MESSAGE, subscribeToJob } from "./ws";
import { useEditorStore } from "../stores/editorStore";
import { addLog } from "../stores/logStore";
import { useQueueStore } from "../stores/queueStore";
import { addToast } from "../stores/toastStore";

let unsub: (() => void) | null = null;
let subscribedJobId: string | null = null;
let finished = false;
let cancelRequested = false;
let modalOpen = false;

export function setTranslationModalOpen(open: boolean): void {
  modalOpen = open;
}

export function subscribedTranslationJobId(): string | null {
  return subscribedJobId;
}

function patchSnapshot(
  patch: Partial<NonNullable<ReturnType<typeof useEditorStore.getState>["jobSnapshot"]>>,
): void {
  useEditorStore.getState().patchJobSnapshot(patch);
}

function endJob(): void {
  useEditorStore.getState().setTranslating(false);
  useEditorStore.getState().setJob(null);
  useQueueStore.getState().setGlobalProgress(null);
  subscribedJobId = null;
  unsub?.();
  unsub = null;
}

function discardSnapshotIfModalClosed(): void {
  if (!modalOpen) {
    useEditorStore.getState().setJobSnapshot(null);
  }
}

export function attachTranslationJob(opts: {
  jobId: string;
  projectName: string;
  providerLabel: string;
}): void {
  if (!shouldSubscribeToJob(opts.jobId, subscribedJobId) && unsub) {
    return;
  }
  unsub?.();
  subscribedJobId = opts.jobId;
  finished = false;
  cancelRequested = false;

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
  });

  unsub = subscribeToJob(opts.jobId, {
    onStarted: (e) => {
      patchSnapshot({ total: e.total, completed: 0, costSoFar: 0 });
      useQueueStore.getState().setGlobalProgress({
        projectName: opts.projectName,
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
      });
      useQueueStore.getState().setGlobalProgress({
        projectName: opts.projectName,
        completed: e.completed,
        total: e.total,
        costSoFar: e.cost_so_far,
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
        `Switched to provider ${e.provider_name} (${e.remaining_pending} still pending)`,
        undefined,
        "translation",
      );
      addToast("info", t("translate.toast.switched", { name: e.provider_name }));
    },
    onCompleted: (e) => {
      if (finished) return;
      finished = true;
      patchSnapshot({
        done: true,
        cancelling: false,
        completed: e.total_translated,
      });
      addLog(
        "info",
        `Translation complete: ${e.total_translated} strings, $${e.total_cost?.toFixed(4) ?? "0"}`,
        undefined,
        "translation",
      );
      addToast(
        "success",
        t("translate.toast.complete", { count: e.total_translated }),
      );
      endJob();
      discardSnapshotIfModalClosed();
    },
    onFailed: (e) => {
      if (finished) return;
      finished = true;
      patchSnapshot({ error: e.error, cancelling: false });
      addLog("error", `Translation failed`, e.error, "translation");
      addToast("error", t("translate.toast.failed", { error: e.error }));
      endJob();
      discardSnapshotIfModalClosed();
    },
    onClosed: () => {
      if (finished) return;
      finished = true;
      if (cancelRequested) {
        patchSnapshot({ cancelled: true, cancelling: false });
        addLog("info", "Translation cancelled", undefined, "translation");
        addToast("info", t("translate.toast.cancelled"));
        endJob();
        discardSnapshotIfModalClosed();
        return;
      }
      const message = t(JOB_STREAM_LOST_MESSAGE);
      patchSnapshot({ error: message, cancelling: false });
      addLog("error", "Translation failed", JOB_STREAM_LOST_MESSAGE, "translation");
      addToast("error", t("translate.toast.failed", { error: message }));
      endJob();
      discardSnapshotIfModalClosed();
    },
  });
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
  if (!jobId) return;
  markTranslationCancelRequested();
  try {
    await cancelTranslation(jobId);
    addLog("info", `Cancel requested for job ${jobId}`, undefined, "translation");
    addToast("info", t("translate.toast.cancelling"));
  } catch (err: unknown) {
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
