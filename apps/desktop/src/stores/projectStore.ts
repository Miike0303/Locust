import { create } from "zustand";
import type { ProjectInfo, ProjectStats } from "../lib/api";

interface ProjectStore {
  project: ProjectInfo | null;
  stats: ProjectStats | null;
  setProject: (p: ProjectInfo) => void;
  setStats: (s: ProjectStats) => void;
  clearProject: () => void;
}

export const useProjectStore = create<ProjectStore>((set) => ({
  project: null,
  stats: null,
  setProject: (project) => set({ project }),
  setStats: (stats) => set({ stats }),
  clearProject: () => set({ project: null, stats: null }),
}));
