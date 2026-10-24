import { QueryClient, type QueryFilters } from "@tanstack/react-query";

export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 30_000 } },
});

function projectDataQueries(projectKey: string): QueryFilters {
  return {
    predicate: ({ queryKey }) => {
      switch (queryKey[0]) {
        case "strings":
        case "string":
          return queryKey[2] === projectKey;
        case "stats":
        case "file-stats":
        case "current-project":
        case "translation-runs":
        case "injection-status":
        case "validation-issues":
        case "validation-summary":
        case "string-history":
          return queryKey[1] === projectKey;
        default:
          return false;
      }
    },
  };
}

/** Refresh project data without touching another database's caches for the same game. */
export async function invalidateProjectData(
  client: QueryClient,
  projectKey: string,
  refetchType: "active" | "none" = "active",
): Promise<void> {
  await client.invalidateQueries({
    ...projectDataQueries(projectKey),
    refetchType,
  });
}

/** Stop reads before switching the backend's current project. */
export async function cancelProjectData(client: QueryClient, projectKey: string): Promise<void> {
  const filters = projectDataQueries(projectKey);
  // invalidateQueries does not await paused refetches. Cancel their retryers so
  // reconnect cannot read the next project's backend into this project's cache.
  // Revert preserves cached data; invalidate again because revert can restore
  // the state from before invalidation. Defer reads until this project reopens.
  await client.cancelQueries(filters, { revert: true });
  await client.invalidateQueries({ ...filters, refetchType: "none" });
}

/** A failed mutation may still have written files or left recovery evidence. */
export function refreshGameState(): void {
  void queryClient.invalidateQueries({ queryKey: ["patchStatus"] });
}
