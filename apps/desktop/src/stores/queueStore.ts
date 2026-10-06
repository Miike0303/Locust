import { create } from "zustand";
import { openProject, startTranslation, cancelTranslation, validate, type TranslationStartParams } from "../lib/api";
import { waitForJob } from "../lib/ws";
import { useProjectStore } from "./projectStore";
import { useEditorStore } from "./editorStore";
import { canStartQueue } from "../lib/translationJob";
import { addLog } from "./logStore";
import { addToast } from "./toastStore";
import { t } from "../lib/i18n";
import { JOB_STREAM_LOST_MESSAGE } from "../lib/ws";
import {
  queueItemPatchAfterValidation,
  validationIssueCount,
} from "../lib/queueValidation";

let activeJobId: string | null = null;

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
  isRunning: boolean;
  isPanelOpen: boolean;
  globalProgress: GlobalProgress | null;
  translationParams: TranslationStartParams | null;
  cancelRequested: boolean;

  addItem: (path: string) => void;
  removeItem: (id: string) => void;
  moveItem: (id: string, direction: "up" | "down") => void;
  clearCompleted: () => void;
  setParams: (p: TranslationStartParams) => void;
  setPanelOpen: (v: boolean) => void;
  setGlobalProgress: (p: GlobalProgress | null) => void;
  startQueue: () => Promise<void>;
  cancelQueue: () => void;
}

export const useQueueStore = create<QueueStore>((set, get) => ({
  items: [],
  isRunning: false,
  isPanelOpen: false,
  globalProgress: null,
  translationParams: null,
  cancelRequested: false,

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

  setParams: (translationParams) => set({ translationParams }),
  setPanelOpen: (isPanelOpen) => set({ isPanelOpen }),
  setGlobalProgress: (globalProgress) => set({ globalProgress }),

  cancelQueue: () => {
    set({ cancelRequested: true });
    const jobId = activeJobId;
    if (!jobId) return;
    void cancelTranslation(jobId).catch((err: any) => {
      addLog("error", t("activity.queue.cancelFailed"), err.message ?? String(err), "queue");
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

    set({ isRunning: true, cancelRequested: false });
    activeJobId = null;
    const pending = items.filter((i) => i.status === "pending");
    addLog("info", t("activity.queue.started", { count: pending.length }), undefined, "queue");

    for (let idx = 0; idx < pending.length; idx++) {
      if (get().cancelRequested) break;

      const item = pending[idx];
      const updateItem = (patch: Partial<QueueItem>) =>
        set((s) => ({
          items: s.items.map((i) => (i.id === item.id ? { ...i, ...patch } : i)),
        }));
      const markCancelled = () => {
        const name = get().items.find((i) => i.id === item.id)?.projectName ?? item.projectName;
        updateItem({ status: "cancelled", error: null });
        addLog("info", t("activity.queue.itemCancelled", { name }), undefined, "queue");
        addToast("info", t("queue.toast.itemCancelled", { name }));
      };

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

        const result = await openProject(item.projectPath);
        updateItem({
          projectName: result.project_name,
          formatId: result.format_id,
        });

        useProjectStore.getState().setProject({
          path: result.project_path,
          format_id: result.format_id,
          name: result.project_name,
          supported_modes: result.supported_modes,
          database_path: result.database_path,
          extraction_warnings: result.extraction_warnings,
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
        activeJobId = job.job_id;
        try {
          if (get().cancelRequested) get().cancelQueue();
          // Step 3: Wait for completion
          await waitForJob(job.job_id, {
            onProgress: (completed, total, costSoFar, costIsComplete) => {
              updateItem({ progress: { completed, total, costSoFar, costIsComplete, startedAt: get().items.find((i) => i.id === item.id)?.progress.startedAt ?? null } });
              set((s) => ({
                globalProgress: s.globalProgress
                  ? { ...s.globalProgress, completed, total, costSoFar, costIsComplete }
                  : null,
              }));
            },
          });
        } finally {
          activeJobId = null;
        }

        if (get().cancelRequested) {
          markCancelled();
          break;
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
        if (get().cancelRequested) {
          markCancelled();
        } else {
          const raw = err.message ?? String(err);
          updateItem({
            status: "error",
            error: raw === JOB_STREAM_LOST_MESSAGE ? t(JOB_STREAM_LOST_MESSAGE) : raw,
          });
          addLog("error", t("activity.queue.itemFailed", { name: item.projectName }), raw, "queue");
          addToast("error", t("queue.toast.itemFailed", { name: item.projectName }));
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
      for (const i of get().items) {
        if (!runIds.has(i.id)) continue;
        if (i.status === "done") finished++;
        else if (i.status === "error") failed++;
      }
      if (failed === 0) {
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
