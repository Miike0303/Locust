export type WorkflowGuideStep = "translate" | "review" | "inject";

export const WORKFLOW_STEP_LABELS: Record<WorkflowGuideStep, string> = {
	translate: "Translate",
	review: "Review",
	inject: "Inject",
};

export interface WorkflowGuideStats {
  pending: number;
  translated: number;
  reviewed: number;
  approved: number;
}

export interface WorkflowGuideContext {
  hasProject: boolean;
  stats: WorkflowGuideStats | null | undefined;
  skipReview: boolean;
}

/** Derive the next workflow action from current project data, never persisted progress. */
export function resolveWorkflowGuideStep({
  hasProject,
  stats,
  skipReview,
}: WorkflowGuideContext): WorkflowGuideStep | null {
  if (!hasProject || !stats) return null;

  // Pending work remains primary even when some strings are further along.
  if (stats.pending > 0) return "translate";

  const hasCompletedWork =
    stats.translated > 0 || stats.reviewed > 0 || stats.approved > 0;
  if (skipReview && hasCompletedWork) return "inject";

  if (stats.translated > 0 || stats.reviewed > 0) return "review";
  if (stats.approved > 0) return "inject";

  return null;
}

const DISMISSED_KEY = "locust.workflowGuide.dismissed";
const SKIP_REVIEW_KEY = "locust.workflowGuide.skipReview";

export type WorkflowGuideStorage = Pick<
  Storage,
  "getItem" | "setItem" | "removeItem"
>;

function browserStorage(): WorkflowGuideStorage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function readFlag(
  key: string,
  storage?: WorkflowGuideStorage | null
): boolean {
  const target = storage === undefined ? browserStorage() : storage;
  if (!target) return false;
  try {
    return target.getItem(key) === "1";
  } catch {
    return false;
  }
}

function writeFlag(
  key: string,
  enabled: boolean,
  storage?: WorkflowGuideStorage | null
): void {
  const target = storage === undefined ? browserStorage() : storage;
  if (!target) return;
  try {
    if (enabled) target.setItem(key, "1");
    else target.removeItem(key);
  } catch {
    /* Storage can be unavailable in private or restricted browser contexts. */
  }
}

export function readWorkflowGuideDismissed(
  storage?: WorkflowGuideStorage | null
): boolean {
  return readFlag(DISMISSED_KEY, storage);
}

export function saveWorkflowGuideDismissed(
  dismissed: boolean,
  storage?: WorkflowGuideStorage | null
): void {
  writeFlag(DISMISSED_KEY, dismissed, storage);
}

export function readSkipReviewPreference(
  storage?: WorkflowGuideStorage | null
): boolean {
  return readFlag(SKIP_REVIEW_KEY, storage);
}

export function saveSkipReviewPreference(
  skipReview: boolean,
  storage?: WorkflowGuideStorage | null
): void {
  writeFlag(SKIP_REVIEW_KEY, skipReview, storage);
}
