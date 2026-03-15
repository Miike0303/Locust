import { create } from "zustand";
import type { StringFilter } from "../lib/api";

interface EditorStore {
  filter: StringFilter;
  selectedEntryId: string | null;
  jobId: string | null;
  isTranslating: boolean;
  setFilter: (f: Partial<StringFilter>) => void;
  setSelected: (id: string | null) => void;
  setJob: (jobId: string | null) => void;
  setTranslating: (v: boolean) => void;
}

export const useEditorStore = create<EditorStore>((set) => ({
  filter: { limit: 100, offset: 0 },
  selectedEntryId: null,
  jobId: null,
  isTranslating: false,
  setFilter: (f) => set((s) => ({ filter: { ...s.filter, ...f } })),
  setSelected: (id) => set({ selectedEntryId: id }),
  setJob: (jobId) => set({ jobId }),
  setTranslating: (v) => set({ isTranslating: v }),
}));
