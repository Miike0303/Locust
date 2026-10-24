import { create } from "zustand";
import type { ProjectInfo, ProjectStats } from "../lib/api";
import { draftProjectKey } from "../lib/projectScope";

interface ProjectStore {
  confirmedOpenProjectKey: string | null;
  projectTransitionInProgress: boolean;
  beginProjectTransition: () => void;
  project: ProjectInfo | null;
  stats: ProjectStats | null;
  setProject: (p: ProjectInfo) => void;
  setStats: (s: ProjectStats) => void;
  clearProject: () => void;
}

export const useProjectStore = create<ProjectStore>((set) => ({
  confirmedOpenProjectKey: null,
  projectTransitionInProgress: false,
  beginProjectTransition: () => set({ projectTransitionInProgress: true }),
  project: null,
  stats: null,
  setProject: (project) => set({ project, confirmedOpenProjectKey: draftProjectKey(project), projectTransitionInProgress: false }),
  setStats: (stats) => set({ stats }),
  clearProject: () => set({ project: null, stats: null, confirmedOpenProjectKey: null, projectTransitionInProgress: false }),
}));
