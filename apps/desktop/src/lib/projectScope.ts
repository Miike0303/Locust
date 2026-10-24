import type { QueryFunctionContext } from "@tanstack/react-query";
import { useProjectStore } from "../stores/projectStore";
import type { ProjectInfo } from "./api";

/** Shared identity for project caches and persisted drafts. */
export function draftProjectKey(project: ProjectInfo): string {
  return JSON.stringify([project.database_path ?? null, project.path, project.format_id]);
}

/** Read at fetch time too: reconnect can run before React commits a render. */
export function isProjectScopedQueryEnabled(projectKey: string): boolean {
  const state = useProjectStore.getState();
  return !state.projectTransitionInProgress && projectKey === state.confirmedOpenProjectKey;
}

/** Capture the render's gate so disabled -> enabled also triggers a refetch. */
export function projectScopedQueryEnabled(projectKey: string, enabled = true): () => boolean {
  const confirmed = enabled && isProjectScopedQueryEnabled(projectKey);
  return () => confirmed && isProjectScopedQueryEnabled(projectKey);
}

/**
 * `enabled` does not stop explicit `refetch()` calls or a paused fetch that
 * resumes on reconnect. The backend answers for whichever project it has open,
 * so check the gate when the request is about to run and keep the cached value
 * instead of fetching another project's data under this key.
 */
export function projectScopedQueryFn<T>(
  projectKey: string,
  fn: (context: QueryFunctionContext) => Promise<T>,
): (context: QueryFunctionContext) => Promise<T> {
  return async (context) => {
    if (!isProjectScopedQueryEnabled(projectKey)) {
      const cached = context.client.getQueryData<T>(context.queryKey);
      if (cached !== undefined) return cached;
      throw new Error("project-scoped query skipped: its project is not the open project");
    }
    return fn(context);
  };
}

export function useProjectScopedQueryEnabled(projectKey: string, enabled = true): () => boolean {
  useProjectStore(state => !state.projectTransitionInProgress && projectKey === state.confirmedOpenProjectKey);
  return projectScopedQueryEnabled(projectKey, enabled);
}
