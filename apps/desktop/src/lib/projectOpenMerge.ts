/**
 * Project-open merge presentation. Opening always merges; toast only when
 * the game text moved under existing translations or lines disappeared.
 */

import { t, type TranslateFn } from "./i18n";

export type ProjectOpenMergeCounts = {
  added: number;
  updated: number;
  stale_source_reset: number;
  removed: number;
  preserved_translations: number;
};

export type ProjectOpenMergeInput = {
  project_name: string;
  format_name: string;
  total_strings: number;
} & Partial<ProjectOpenMergeCounts>;

export type ProjectOpenMergeNotice = {
  toast: boolean;
  logMessage: string;
  toastMessage: string | null;
};

function asCount(n: unknown): number {
  return typeof n === "number" && Number.isFinite(n) && n > 0 ? Math.trunc(n) : 0;
}

export function shouldToastProjectOpenMerge(
  counts: Pick<ProjectOpenMergeCounts, "stale_source_reset" | "removed">,
): boolean {
  return asCount(counts.stale_source_reset) > 0 || asCount(counts.removed) > 0;
}

/** Editor filter after open when translations were reset to pending for review. */
export const PENDING_AFTER_STALE_FILTER = {
  status: "pending" as const,
  offset: 0,
};

/** Only stale resets need the pending list — removed rows are gone, not pending. */
export function shouldFocusPendingAfterOpen(
  counts: Pick<ProjectOpenMergeCounts, "stale_source_reset">,
): boolean {
  return asCount(counts.stale_source_reset) > 0;
}

export function projectOpenMergeNotice(
  result: ProjectOpenMergeInput,
  translateToast: TranslateFn,
): ProjectOpenMergeNotice {
  const added = asCount(result.added);
  const updated = asCount(result.updated);
  const stale = asCount(result.stale_source_reset);
  const removed = asCount(result.removed);
  const preserved = asCount(result.preserved_translations);

  const logMessage = t("activity.project.openedMerge", {
    name: result.project_name,
    format: result.format_name,
    strings: t("activity.count.strings", { count: result.total_strings }),
    added: t("activity.project.added", { count: added }),
    updated: t("activity.project.updated", { count: updated }),
    stale: t("activity.project.stale", { count: stale }),
    removed: t("activity.project.removed", { count: removed }),
    preserved: t("activity.project.preserved", { count: preserved }),
  });

  if (!shouldToastProjectOpenMerge({ stale_source_reset: stale, removed })) {
    return { toast: false, logMessage, toastMessage: null };
  }

  const parts = [translateToast("welcome.toast.sourceChanged")];
  if (stale > 0) {
    parts.push(translateToast("welcome.toast.staleReset", { count: stale }));
  }
  if (removed > 0) {
    parts.push(translateToast("welcome.toast.removed", { count: removed }));
  }
  return { toast: true, logMessage, toastMessage: parts.join(" ") };
}
