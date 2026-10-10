import { create } from "zustand";
import type { StringFilter } from "../lib/api";
import { stepWorklistIndex } from "../lib/validationWorklist";

export interface TranslationJobSnapshot {
  completed: number;
  total: number;
  costSoFar: number;
  costIsComplete?: boolean;
  batchFailures?: { count: number; lastReason: string };
  lastTranslated: string;
  activeProviderLabel: string;
  error: string | null;
  done: boolean;
  cancelled: boolean;
  cancelling: boolean;
}

interface EditorStore {
  filter: StringFilter;
  selectedEntryId: string | null;
  jobId: string | null;
  isTranslating: boolean;
  jobSnapshot: TranslationJobSnapshot | null;
  /** Unique entry ids from the last Validate "Review in Editor" action. */
  validationWorklist: string[] | null;
  validationWorklistIndex: number;
  setFilter: (f: Partial<StringFilter>) => void;
  setSelected: (id: string | null) => void;
  setJob: (jobId: string | null) => void;
  setTranslating: (v: boolean) => void;
  setJobSnapshot: (snapshot: TranslationJobSnapshot | null) => void;
  patchJobSnapshot: (patch: Partial<TranslationJobSnapshot>) => void;
  startValidationWorklist: (ids: string[]) => void;
  clearValidationWorklist: () => void;
  stepValidationWorklist: (delta: number) => void;
}

export const useEditorStore = create<EditorStore>((set) => ({
  filter: { limit: 100, offset: 0 },
  selectedEntryId: null,
  jobId: null,
  isTranslating: false,
  jobSnapshot: null,
  validationWorklist: null,
  validationWorklistIndex: 0,
  setFilter: (f) => set((s) => ({ filter: { ...s.filter, ...f } })),
  setSelected: (id) => set({ selectedEntryId: id }),
  setJob: (jobId) => set({ jobId }),
  setTranslating: (v) => set({ isTranslating: v }),
  setJobSnapshot: (jobSnapshot) => set({ jobSnapshot }),
  patchJobSnapshot: (patch) =>
    set((s) =>
      s.jobSnapshot ? { jobSnapshot: { ...s.jobSnapshot, ...patch } } : s,
    ),
  startValidationWorklist: (ids) => {
    if (ids.length === 0) {
      set({ validationWorklist: null, validationWorklistIndex: 0 });
      return;
    }
    set({
      validationWorklist: ids,
      validationWorklistIndex: 0,
      selectedEntryId: ids[0],
    });
  },
  clearValidationWorklist: () =>
    set({ validationWorklist: null, validationWorklistIndex: 0 }),
  stepValidationWorklist: (delta) =>
    set((s) => {
      const list = s.validationWorklist;
      if (!list || list.length === 0) return s;
      const next = stepWorklistIndex(s.validationWorklistIndex, list.length, delta);
      return {
        validationWorklistIndex: next,
        selectedEntryId: list[next],
      };
    }),
}));
