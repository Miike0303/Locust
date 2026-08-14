/**
 * Queue validation is informational: it must never flip a successful
 * translation to failed. A missing or unreadable issues_found counts as 0.
 */

export type QueueValidationPatch = {
  status: "done";
  validationIssues: number | null;
  validationError: string | null;
};

export function validationIssueCount(res: {
  validation?: { issues_found?: number } | null;
} | null | undefined): number {
  const n = res?.validation?.issues_found;
  return typeof n === "number" && Number.isFinite(n) && n > 0 ? n : 0;
}

export function queueItemPatchAfterValidation(
  outcome:
    | { ok: true; issuesFound: number }
    | { ok: false; error: string },
): QueueValidationPatch {
  if (outcome.ok) {
    return {
      status: "done",
      validationIssues: outcome.issuesFound,
      validationError: null,
    };
  }
  return {
    status: "done",
    validationIssues: null,
    validationError: outcome.error,
  };
}
