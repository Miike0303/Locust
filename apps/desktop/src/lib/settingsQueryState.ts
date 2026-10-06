import { ApiError } from "./apiError";

/** Classify failed project queries without depending on the current UI language. */
export function settingsQueryState(error: unknown): "no_project" | "failed" {
  return error instanceof ApiError && error.key === "api.error.noProjectOpen"
    ? "no_project"
    : "failed";
}
