import { QueryClient, type Query, type QueryFilters } from "@tanstack/react-query";
import type { StringEntry, StringFilter, StringsResponse } from "./api";
import { isProjectScopedQueryEnabled } from "./projectScope";
import { useProjectStore } from "../stores/projectStore";

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

type EditRefetch = { running: boolean; again: boolean; promise: Promise<void> };
const editRefetches = new WeakMap<QueryClient, Map<Query, EditRefetch>>();

/** Join a pending read, then read once more if a write landed after it began. */
function refetchEditedQuery(client: QueryClient, projectKey: string, query: Query): Promise<void> {
  let pending = editRefetches.get(client);
  if (!pending) { pending = new Map(); editRefetches.set(client, pending); }
  const existing = pending.get(query);
  if (existing) {
    if (existing.running) existing.again = true;
    return existing.promise;
  }
  const state: EditRefetch = { running: false, again: false, promise: Promise.resolve() };
  state.promise = Promise.resolve().then(async () => {
    try {
      do {
        state.again = false;
        // A queued trailing read must also respect project switches and unmounts.
        if (!isProjectScopedQueryEnabled(projectKey) || !query.isActive()
          || client.getQueryCache().get(query.queryHash) !== query) return;
        state.running = true;
        const alreadyFetching = query.state.fetchStatus === "fetching";
        await client.refetchQueries({ queryKey: query.queryKey, exact: true, type: "active" }, { cancelRefetch: false });
        state.running = false;
        if (alreadyFetching) state.again = true;
      } while (state.again);
    } finally {
      pending.delete(query);
    }
  });
  pending.set(query, state);
  return state.promise;
}

function matchesEditScope(filter: StringFilter, entry: StringEntry): boolean {
  return (!filter.status || filter.status === entry.status)
    && (!filter.file_path || filter.file_path === entry.file_path);
}

/** Refresh only data a successful single-row edit can change. Bulk writes use invalidateProjectData. */
export async function refreshEditedString(
  client: QueryClient,
  projectKey: string,
  saved: StringEntry,
  previous: StringEntry,
): Promise<void> {
  if (!isProjectScopedQueryEnabled(projectKey)) return;
  const rowKey = ["string", saved.id, projectKey];
  // A pre-save detail read must not overwrite the authoritative PATCH response.
  void client.cancelQueries({ queryKey: rowKey, exact: true });
  client.setQueryData(rowKey, saved);
  const affected = new Set<Query>();
  for (const query of client.getQueryCache().findAll({ queryKey: ["strings"] })) {
    const [_, rawFilter, scope] = query.queryKey;
    if (scope !== projectKey) continue;
    const filter = rawFilter as StringFilter;
    const before = matchesEditScope(filter, previous), after = matchesEditScope(filter, saved);
    if (!before && !after) continue;
    const page = query.state.data as StringsResponse | undefined;
    // Search/tag use SQL LIKE semantics. Let the server determine membership
    // and refill pagination whenever it might change, including inactive pages.
    if (filter.search || filter.tag || before !== after || !page || query.state.isInvalidated) {
      affected.add(query);
    } else if (page.entries.some(entry => entry.id === saved.id)) {
      void client.cancelQueries({ queryKey: query.queryKey, exact: true });
      client.setQueryData<StringsResponse>(query.queryKey, current => current && ({
        ...current, entries: current.entries.map(entry => entry.id === saved.id ? saved : entry),
      }));
    }
  }
  for (const family of ["stats", "file-stats", "string-history"]) {
    const key = family === "string-history" ? [family, projectKey, saved.id] : [family, projectKey];
    for (const query of client.getQueryCache().findAll({ queryKey: key })) affected.add(query);
  }
  // Facets currently contain file paths and tags, so status alone cannot change them.
  if (saved.file_path !== previous.file_path || JSON.stringify(saved.tags) !== JSON.stringify(previous.tags)) {
    const path = useProjectStore.getState().project?.path;
    for (const query of client.getQueryCache().findAll({ queryKey: ["string-facets", path], exact: true })) affected.add(query);
  }
  // QA results are persisted snapshots, unchanged until validation runs again.
  // patchString already records project-scoped translation staleness evidence.
  await client.invalidateQueries({ predicate: query => affected.has(query), refetchType: "none" });
  await Promise.all([...affected].map(query => refetchEditedQuery(client, projectKey, query)));
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
