import { create } from "zustand";
import type { StringFilter } from "../lib/api";

export interface TranslationJobSnapshot {
  completed: number;
  total: number;
  costSoFar: number;
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
  setFilter: (f: Partial<StringFilter>) => void;
  setSelected: (id: string | null) => void;
  setJob: (jobId: string | null) => void;
  setTranslating: (v: boolean) => void;
  setJobSnapshot: (snapshot: TranslationJobSnapshot | null) => void;
  patchJobSnapshot: (patch: Partial<TranslationJobSnapshot>) => void;
}

export const useEditorStore = create<EditorStore>((set) => ({
  filter: { limit: 100, offset: 0 },
  selectedEntryId: null,
  jobId: null,
  isTranslating: false,
  jobSnapshot: null,
  setFilter: (f) => set((s) => ({ filter: { ...s.filter, ...f } })),
  setSelected: (id) => set({ selectedEntryId: id }),
  setJob: (jobId) => set({ jobId }),
  setTranslating: (v) => set({ isTranslating: v }),
  setJobSnapshot: (jobSnapshot) => set({ jobSnapshot }),
  patchJobSnapshot: (patch) =>
    set((s) =>
      s.jobSnapshot ? { jobSnapshot: { ...s.jobSnapshot, ...patch } } : s,
    ),
}));
