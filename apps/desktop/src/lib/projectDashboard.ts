import type { FileStats, ProjectInfo, ProjectStats, StringStatus } from "./api";
import { resolveWorkflowGuideStep } from "./workflowGuide";

const STATUSES: readonly StringStatus[] = ["pending", "translated", "reviewed", "approved", "error"];

export function formatDashboardCost(costUsd: number): string {
  const amount = costUsd > 0 && costUsd < 0.01
    ? costUsd.toFixed(4).replace(/\.?0+$/, "")
    : costUsd.toFixed(2);
  return `$${amount}`;
}

function donePercent(stats: Pick<ProjectStats, "total" | "translated" | "reviewed" | "approved">): number {
  return stats.total > 0
    ? Math.min(100, Math.round((stats.translated + stats.reviewed + stats.approved) / stats.total * 100))
    : 0;
}

function relativePath(filePath: string, gamePath: string): string {
  const path = filePath.replace(/\\/g, "/");
  const root = gamePath.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
  return path.startsWith(root) ? path.slice(root.length) : path;
}

/** Derive presentation only; workflow priorities remain owned by workflowGuide. */
export function buildProjectDashboard({ project, stats, files, skipReview }: {
  project: ProjectInfo;
  stats: ProjectStats;
  files: readonly FileStats[];
  skipReview: boolean;
}) {
  const counts = STATUSES.map(status => stats[status]);
  const sum = counts.reduce((total, count) => total + count, 0);
  let used = 0;
  // Assign the rounding remainder to the final nonempty segment.
  const last = counts.reduce((index, count, i) => count > 0 ? i : index, 0);
  const segments = STATUSES.map((status, index) => {
    const width = sum === 0 ? (index === 0 ? 100 : 0)
      : index === last ? 100 - used : counts[index] / sum * 100;
    used += width;
    return { status, count: counts[index], width };
  });
  const sortedFiles = [...files].sort((a, b) => b.pending - a.pending || b.error - a.error || a.file_path.localeCompare(b.file_path));
  const displayFile = (file: FileStats) => ({ ...file, relativePath: relativePath(file.file_path, project.path), percent: donePercent(file) });
  const warnings = project.extraction_warnings ?? [];
  const step = resolveWorkflowGuideStep({ hasProject: true, stats, skipReview });
  return {
    projectName: project.path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").pop() || project.name,
    percent: donePercent(stats),
    segments,
    topFiles: sortedFiles.slice(0, 10).map(displayFile),
    errorFiles: sortedFiles.filter(file => file.error > 0).map(displayFile),
    warnings: warnings.slice(0, 10),
    moreWarnings: Math.max(0, warnings.length - 10),
    nextStep: step ? { step, path: step === "review" ? "/review" : "/editor" } : null,
  };
}
