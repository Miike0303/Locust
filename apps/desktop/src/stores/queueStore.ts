import { create } from "zustand";
import { openProject, startTranslation, type TranslationStartParams } from "../lib/api";
import { waitForJob } from "../lib/ws";
import { useProjectStore } from "./projectStore";
import { addLog } from "./logStore";
import { addToast } from "./toastStore";

export type QueueItemStatus = "pending" | "extracting" | "translating" | "done" | "error" | "cancelled";

export interface QueueItem {
  id: string;
  projectPath: string;
  projectName: string;
  formatId: string | null;
  status: QueueItemStatus;
  progress: { completed: number; total: number; costSoFar: number; startedAt: number | null };
  error: string | null;
}

export interface GlobalProgress {
  projectName: string;
  completed: number;
  total: number;
  costSoFar: number;
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

  cancelQueue: () => set({ cancelRequested: true }),

  startQueue: async () => {
    const { items, translationParams } = get();
    if (!translationParams) {
      addToast("error", "Configure translation settings first");
      return;
    }

    set({ isRunning: true, cancelRequested: false });
    const pending = items.filter((i) => i.status === "pending");
    addLog("info", `Queue started: ${pending.length} projects`, undefined, "queue");

    for (let idx = 0; idx < pending.length; idx++) {
      if (get().cancelRequested) {
        addLog("warning", "Queue cancelled by user", undefined, "queue");
        addToast("warning", "Queue cancelled");
        break;
      }

      const item = pending[idx];
      const updateItem = (patch: Partial<QueueItem>) =>
        set((s) => ({
          items: s.items.map((i) => (i.id === item.id ? { ...i, ...patch } : i)),
        }));

      try {
        // Step 1: Open project
        updateItem({ status: "extracting" });
        addLog("info", `Opening: ${item.projectPath}`, undefined, "queue");
        set({
          globalProgress: {
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
        });

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

        // Step 3: Wait for completion
        await waitForJob(job.job_id, {
          onProgress: (completed, total, costSoFar) => {
            updateItem({ progress: { completed, total, costSoFar, startedAt: get().items.find((i) => i.id === item.id)?.progress.startedAt ?? null } });
            set((s) => ({
              globalProgress: s.globalProgress
                ? { ...s.globalProgress, completed, total, costSoFar }
                : null,
            }));
          },
        });

        updateItem({ status: "done" });
        addLog("info", `Completed: ${result.project_name} (${result.total_strings} strings)`, undefined, "queue");
        addToast("success", `${result.project_name} translated`);
      } catch (err: any) {
        updateItem({ status: "error", error: err.message });
        addLog("error", `Failed: ${item.projectName}`, err.message, "queue");
        addToast("error", `Queue error: ${item.projectName}`);
      }
    }

    set({ isRunning: false, globalProgress: null });
    if (!get().cancelRequested) {
      addLog("info", "Queue finished", undefined, "queue");
      addToast("success", "All projects in queue completed");
    }
  },
}));
