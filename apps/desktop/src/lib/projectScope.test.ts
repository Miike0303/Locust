import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { QueryClient, QueryObserver, onlineManager } from "@tanstack/react-query";
import { useProjectStore } from "../stores/projectStore.ts";
import { projectScopedQueryFn } from "./projectScope.ts";

const clients: QueryClient[] = [];
function newClient() {
  // gcTime: Infinity schedules no garbage-collection timers that keep the test alive.
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  client.mount(); // subscribe to onlineManager so paused fetches resume on reconnect
  clients.push(client);
  return client;
}

const A = "project-A";
const B = "project-B";

function confirm(key: string | null, transition = false) {
  useProjectStore.setState({ confirmedOpenProjectKey: key, projectTransitionInProgress: transition });
}

afterEach(() => {
  for (const client of clients.splice(0)) { client.clear(); client.unmount(); }
  onlineManager.setOnline(true);
  confirm(null);
});

test("a manual refresh paused offline cannot cache another project's data after reconnect", async () => {
  const client = newClient();
  const key = ["translation-runs", A];
  let backend = "A";
  let calls = 0;
  client.setQueryData(key, "A-runs");
  confirm(A);
  const observer = new QueryObserver(client, {
    queryKey: key,
    queryFn: projectScopedQueryFn(A, async () => { calls += 1; return `${backend}-runs`; }),
    enabled: false,
  });
  const unsubscribe = observer.subscribe(() => {});

  onlineManager.setOnline(false);
  const refresh = observer.refetch(); // Refresh button: bypasses `enabled`.
  // The queue starts opening B; the backend now answers for B.
  confirm(A, true);
  backend = "B";
  onlineManager.setOnline(true); // Reconnect while B's open RPC is still pending.
  await refresh;

  assert.equal(calls, 0, "no request may run for A while the project is in transition");
  assert.equal(client.getQueryData(key), "A-runs");
  unsubscribe();
});

test("a project that is not the confirmed open project keeps its cached value", async () => {
  const client = newClient();
  client.setQueryData(["stats", A], { total: 3 });
  confirm(B);
  let calls = 0;
  const data = await client.fetchQuery({
    queryKey: ["stats", A],
    queryFn: projectScopedQueryFn(A, async () => { calls += 1; return { total: 99 }; }),
    staleTime: 0,
  });
  assert.deepEqual(data, { total: 3 });
  assert.equal(calls, 0);
});

test("without cached data a skipped project query fails instead of fetching", async () => {
  const client = newClient();
  confirm(B);
  await assert.rejects(
    client.fetchQuery({ queryKey: ["stats", A], queryFn: projectScopedQueryFn(A, async () => ({ total: 1 })) }),
    /not the open project/,
  );
});

test("the confirmed open project fetches normally", async () => {
  const client = newClient();
  confirm(A);
  const data = await client.fetchQuery({ queryKey: ["stats", A], queryFn: projectScopedQueryFn(A, async () => ({ total: 7 })) });
  assert.deepEqual(data, { total: 7 });
});
