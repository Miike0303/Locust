import { QueryClient } from "@tanstack/react-query";
import type { ProjectInfo } from "./api";
import { draftProjectKey } from "./projectScope";

export const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: 1, staleTime: 30_000 } },
});

/** Shared by single-job editor completion and queue item/run completion. */
export async function refreshTranslationQueries(
  project: ProjectInfo | null,
  client = queryClient,
  refetchType: "active" | "none" = "active",
): Promise<void> {
  if (!project) return;
  const scope = draftProjectKey(project);
  await client.invalidateQueries({
    predicate: ({ queryKey: key }) => {
      switch (key[0]) {
        case "strings":
        case "string":
          return key[2] === scope;
        case "stats":
        case "workflow":
          // The current workflow guide derives from stats; also cover a scoped cache.
          return key[1] === scope;
        case "string-facets":
          return key[1] === project.path;
        case "patchStatus":
          return key[1] === project.path.trim();
        default:
          return false;
      }
    },
    refetchType,
  });
}

/** A failed mutation may still have written files or left recovery evidence. */
export function refreshGameState(): void {
  void queryClient.invalidateQueries({ queryKey: ["patchStatus"] });
}
