import type { ValidationSummary } from "./api";
import { buildQaSummary } from "./qaDashboard";
import { readStorageFlag, writeStorageFlag } from "./storage";

export type WorkflowGuideStep = "translate" | "review" | "check" | "write";

export const WORKFLOW_STEP_LABELS: Record<WorkflowGuideStep, string> = {
	translate: "Translate",
	review: "Review",
	check: "Check",
	write: "Write",
};

export type WorkflowCheckState = "not-checked" | "checking" | "issues" | "current";

export interface WorkflowGuideCheck {
	state: WorkflowCheckState;
	openIssues: number;
}

export interface WorkflowCheckContext {
	summary: ValidationSummary | undefined;
	lastCheckedAt: number | null;
	latestEditAt: number | null;
	isChecking: boolean;
}

/** Only a completed session run can establish a current clean result. */
export function resolveWorkflowCheckState({
	summary, lastCheckedAt, latestEditAt, isChecking,
}: WorkflowCheckContext): WorkflowGuideCheck {
	const openIssues = buildQaSummary(summary ?? {}).open;
	let state: WorkflowCheckState;
	if (isChecking) state = "checking";
	else if (latestEditAt !== null && latestEditAt > (lastCheckedAt ?? 0)) state = "not-checked";
	else if (!summary) state = "not-checked";
	else if (openIssues > 0) state = "issues";
	else state = lastCheckedAt === null ? "not-checked" : "current";
	return { state, openIssues };
}

export function resolveWorkflowGuideTarget(step: WorkflowGuideStep) {
	return {
		translate: "translate-dialog",
		review: "/review",
		check: "/qa",
		write: "inject-dialog",
	}[step];
}

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
	checkState?: WorkflowCheckState;
}

/** Derive the next workflow action from current project data, never persisted progress. */
export function resolveWorkflowGuideStep({
	hasProject,
	stats,
	skipReview,
	checkState = "not-checked",
}: WorkflowGuideContext): WorkflowGuideStep | null {
	if (!hasProject || !stats) return null;

	// Pending work remains primary even when some strings are further along.
	if (stats.pending > 0) return "translate";

	const hasCompletedWork =
		stats.translated > 0 || stats.reviewed > 0 || stats.approved > 0;
	if (!skipReview && (stats.translated > 0 || stats.reviewed > 0)) return "review";
	if (hasCompletedWork) return checkState === "current" ? "write" : "check";

	return null;
}

const DISMISSED_KEY = "locust.workflowGuide.dismissed";
const SKIP_REVIEW_KEY = "locust.workflowGuide.skipReview";

export type WorkflowGuideStorage = Pick<
	Storage,
	"getItem" | "setItem" | "removeItem"
>;

export function readWorkflowGuideDismissed(
	storage?: WorkflowGuideStorage | null,
): boolean {
	return readStorageFlag(DISMISSED_KEY, storage);
}

export function saveWorkflowGuideDismissed(
	dismissed: boolean,
	storage?: WorkflowGuideStorage | null,
): void {
	writeStorageFlag(DISMISSED_KEY, dismissed, storage);
}

export function readSkipReviewPreference(
	storage?: WorkflowGuideStorage | null,
): boolean {
	return readStorageFlag(SKIP_REVIEW_KEY, storage);
}

export function saveSkipReviewPreference(
	skipReview: boolean,
	storage?: WorkflowGuideStorage | null,
): void {
	writeStorageFlag(SKIP_REVIEW_KEY, skipReview, storage);
}
