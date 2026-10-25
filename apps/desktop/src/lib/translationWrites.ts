import { queryClient } from "./queryClient";
import { qaLastRunKey } from "./qaDashboard";
import { draftProjectKey } from "./projectScope";
import { useProjectStore } from "../stores/projectStore";

export const workflowLastEditKey = (projectKey: string) => ["workflow-last-edit", projectKey] as const;

/** Capture ownership before a write starts, never when its response arrives. */
export function translationWriteProjectKey(): string {
  const project = useProjectStore.getState().project;
  return project ? draftProjectKey(project) : "";
}

/** Session-only evidence that a successful write made validation stale. */
export function recordTranslationWrite(projectKey: string): void {
  if (!projectKey) return;
  const key = workflowLastEditKey(projectKey);
  // Keep evidence while Editor is unmounted, including saves from Review.
  queryClient.setQueryDefaults(key, { gcTime: Infinity, staleTime: Infinity });
  const checked = queryClient.getQueryData<number | null>(qaLastRunKey(projectKey)) ?? 0;
  // Break ties with the check without advancing the timestamp for every row.
  queryClient.setQueryData<number>(key, previous =>
    Math.max(Date.now(), checked + 1, previous ?? 0));
}

export async function trackTranslationWrite<T>(
  write: () => Promise<T>,
  changed: (result: T) => boolean,
): Promise<T> {
  const projectKey = translationWriteProjectKey();
  const result = await write();
  if (changed(result)) recordTranslationWrite(projectKey);
  return result;
}
