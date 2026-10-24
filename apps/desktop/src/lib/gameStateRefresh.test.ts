import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { registerHooks } from "node:module";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider, QueryObserver, onlineManager } from "@tanstack/react-query";
import { queryClient } from "./queryClient.ts";
import * as projectData from "./queryClient.ts";
import { draftProjectKey } from "../stores/draftStore.ts";

const originalFetch = globalThis.fetch;
const windowDescriptor = Object.getOwnPropertyDescriptor(globalThis, "window");
let api: typeof import("./api.ts");
let fail = false;
let finish: (() => void) | undefined;
let injectCalls = 0;
function completeMutation() {
  assert.ok(finish, "the real API mutation must have started");
  finish();
}
before(async () => {
  Object.defineProperty(globalThis, "window", { configurable: true, value: {
    __TAURI_INTERNALS__: { invoke: async (command: string) => {
      if (command === "get_server_port") return 7842;
      assert.equal(command, "run_inject");
      injectCalls++;
      await new Promise<void>(resolve => { finish = resolve; });
      if (fail) throw new Error("interrupted injection");
      return { mode: "direct", languages_processed: ["es"], languages_failed: [] };
    } },
  } });
  globalThis.fetch = async () => {
    await new Promise<void>(resolve => { finish = resolve; });
    return new Response(fail ? "interrupted restore" : "{}", { status: fail ? 409 : 200 });
  };
  api = await import("./api.ts");
});
after(() => {
  queryClient.clear();
  globalThis.fetch = originalFetch;
  if (windowDescriptor) Object.defineProperty(globalThis, "window", windowDescriptor);
  else Reflect.deleteProperty(globalThis, "window");
});

for (const operation of ["inject", "restore", "recover"] as const) {
  for (const rejects of [false, true]) {
    test(`${operation} settlement refreshes the editor badge (failure=${rejects})`, async () => {
      fail = rejects;
      finish = undefined;
      const key = ["patchStatus", "game", 4]; // same key prefix used by Editor's badge
      queryClient.setQueryData(key, { status: "not_patched" });
      let reads = 0;
      const observer = new QueryObserver(queryClient, {
        queryKey: key, staleTime: Infinity,
        queryFn: async () => { reads++; return { status: "not_patched" }; },
      });
      const unsubscribe = observer.subscribe(() => {});
      try {
        const pending = operation === "inject"
          ? api.inject({ project_path: "game", format_id: "html-game", languages: ["es"], direct: true })
          : operation === "restore" ? api.restoreBackup("backup") : api.recoverInjection("game", "transaction");
        // Attach rejection handling before allowing the simulated backend to settle.
        const settled = pending.then(() => null, error => error);
        for (let i = 0; !finish && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
        assert.ok(finish, "the real API mutation must have started");
        assert.equal(reads, 0, "do not refresh before the game mutation finishes");
        completeMutation();
        const error = await settled;
        assert.equal(error instanceof Error, rejects);
        for (let i = 0; reads === 0 && i < 20; i++) await new Promise(resolve => setTimeout(resolve, 0));
        assert.equal(reads, 1, "the badge query must refetch when the mutation settles");
        if (operation === "inject") assert.ok(injectCalls > 0);
      } finally { unsubscribe(); queryClient.clear(); }
    });
  }
}

test("project invalidation scopes all editor and dashboard data by the complete draft identity", async () => {
  const client = new QueryClient();
  const project = { path: "/game", database_path: "/game.locust.db", format_id: "renpy", name: "Game" };
  const scope = draftProjectKey(project);
  const keys = (key: string) => [
    ["strings", {}, key], ["strings", { offset: 50 }, key], ["string", "row", key],
    ...["stats", "file-stats", "current-project", "translation-runs", "injection-status",
      "validation-issues", "validation-summary", "string-history"].map(family => [family, key]),
  ];
  const own = keys(scope);
  const untouched = [
    ...keys(draftProjectKey({ ...project, database_path: "/other.db" })),
    ...keys(draftProjectKey({ ...project, path: "/other-game" })),
    ...keys(draftProjectKey({ ...project, format_id: "html-game" })),
    ["stats"], ["translation-runs"], ["providers"], ["config"],
    ["string-facets", project.path], ["patchStatus", project.path], ["unrelated", scope],
  ];
  try {
    for (const key of [...own, ...untouched]) client.setQueryData(key, "cached");
    await projectData.invalidateProjectData(client, scope);
    for (const key of own) assert.equal(client.getQueryState(key)?.isInvalidated, true, JSON.stringify(key));
    for (const key of untouched) assert.equal(client.getQueryState(key)?.isInvalidated, false, JSON.stringify(key));
  } finally { client.clear(); }
});

for (const refetchType of ["active", "none"] as const) {
  test(`project invalidation refetchType=${refetchType} respects active and inactive queries`, async () => {
    const client = new QueryClient();
    const scope = '["/game.db","/game","renpy"]';
    const active = ["stats", scope];
    const inactive = ["string", "row", scope];
    client.setQueryData(active, "cached");
    client.setQueryData(inactive, "cached");
    let reads = 0;
    let release = () => {};
    const observer = new QueryObserver(client, { queryKey: active, staleTime: Infinity,
      queryFn: async () => { reads++; await new Promise<void>(resolve => { release = resolve; }); return "fresh"; } });
    const unsubscribe = observer.subscribe(() => {});
    try {
      let settled = false;
      const pending = projectData.invalidateProjectData(client, scope, refetchType).then(() => { settled = true; });
      await Promise.resolve();
      assert.equal(reads, refetchType === "active" ? 1 : 0);
      if (refetchType === "active") assert.equal(settled, false, "caller must be able to await active refetches");
      release(); await pending;
      assert.equal(client.getQueryState(inactive)?.isInvalidated, true);
      assert.equal(client.getQueryData(active), refetchType === "active" ? "fresh" : "cached");
      if (refetchType === "none") assert.equal(client.getQueryState(active)?.isInvalidated, true);
    } finally { release(); unsubscribe(); client.clear(); }
  });
}

test("Editor completion refreshes the open project's rows, stats, detail, validation and history", async () => {
  let refresh: (() => void) | undefined;
  (globalThis as any).__captureEditorRefresh = (callback: () => void) => { refresh = callback; };
  const hooks = registerHooks({
    load(url, context, nextLoad) {
      if (/\/src\/components\/[^/]+\.tsx$/.test(url.replace(/\\/g, "/"))) {
        const capture = url.endsWith("/SearchReplaceModal.tsx")
          ? "globalThis.__captureEditorRefresh(props.onDone);" : "";
        return { format: "module", shortCircuit: true,
          source: `export default function Component(props) { ${capture} return null; }` };
      }
      return nextLoad(url, context);
    },
  });
  const { useProjectStore } = await import("../stores/projectStore.ts");
  const { useEditorStore } = await import("../stores/editorStore.ts");
  const initialProject = useProjectStore.getInitialState().project;
  const initialSelection = useEditorStore.getInitialState().selectedEntryId;
  const previousProject = useProjectStore.getState().project;
  const previousSelection = useEditorStore.getState().selectedEntryId;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  const project = { path: "/game", database_path: "/game.db", format_id: "renpy", name: "Game" };
  const scope = draftProjectKey(project);
  const otherScope = draftProjectKey({ ...project, database_path: "/other.db" });
  const keys = (key: string) => [["strings", useEditorStore.getState().filter, key], ["stats", key],
    ["string", "row", key], ["validation-summary", key], ["string-history", key, "row"], ["translation-runs", key]];
  try {
    useProjectStore.setState({ project });
    useEditorStore.setState({ selectedEntryId: "row" });
    // Zustand's server snapshot uses the original state object.
    useProjectStore.getInitialState().project = project;
    useEditorStore.getInitialState().selectedEntryId = "row";
    for (const key of [...keys(scope), ...keys(otherScope)]) client.setQueryData(key, null);
    const Editor = (await import("../pages/Editor.tsx")).default;
    renderToStaticMarkup(createElement(QueryClientProvider, { client },
      createElement(MemoryRouter, {}, createElement(Editor))));
    assert.ok(refresh, "Editor must pass its refresh callback to completion actions");
    await refresh();
    for (const key of keys(otherScope)) assert.equal(client.getQueryState(key)?.isInvalidated, false, JSON.stringify(key));
    for (const key of keys(scope)) assert.equal(client.getQueryState(key)?.isInvalidated, true, JSON.stringify(key));
  } finally {
    client.clear(); hooks.deregister();
    useProjectStore.getInitialState().project = initialProject;
    useEditorStore.getInitialState().selectedEntryId = initialSelection;
    useProjectStore.setState({ project: previousProject });
    useEditorStore.setState({ selectedEntryId: previousSelection });
    delete (globalThis as any).__captureEditorRefresh;
  }
});


for (const status of ["fetching", "paused"] as const) {
  test(`project cancellation reverts ${status} queries in every scoped family and leaves them stale`, async () => {
    const client = new QueryClient();
    const scope = '["/game.db","/game","renpy"]';
    const keys = [["strings", {}, scope], ["string", "row", scope],
      ...["stats", "file-stats", "current-project", "translation-runs", "injection-status",
        "validation-issues", "validation-summary", "string-history"].map(family => [family, scope])];
    const other = ["validation-summary", "other project"];
    const unsubscribes: (() => void)[] = [];
    const pending: Promise<unknown>[] = [];
    let release = () => {};
    const response = new Promise<string>(resolve => { release = () => resolve("late response"); });
    try {
      onlineManager.setOnline(status !== "paused");
      for (const key of [...keys, other]) {
        client.setQueryData(key, "cached");
        const observer = new QueryObserver(client, { queryKey: key, staleTime: Infinity,
          queryFn: () => response });
        unsubscribes.push(observer.subscribe(() => {}));
        pending.push(observer.refetch());
        assert.equal(client.getQueryState(key)?.fetchStatus, status);
      }
      await projectData.cancelProjectData(client, scope);
      for (const key of keys) {
        assert.equal(client.getQueryData(key), "cached", JSON.stringify(key));
        assert.equal(client.getQueryState(key)?.fetchStatus, "idle", JSON.stringify(key));
        assert.equal(client.getQueryState(key)?.isInvalidated, true, JSON.stringify(key));
      }
      assert.equal(client.getQueryState(other)?.fetchStatus, status);
      assert.equal(client.getQueryState(other)?.isInvalidated, false);
      release();
      await Promise.resolve();
      await Promise.resolve();
      for (const key of keys) assert.equal(client.getQueryData(key), "cached", "late results stay cancelled");
    } finally {
      release();
      unsubscribes.forEach(unsubscribe => unsubscribe());
      client.clear();
      onlineManager.setOnline(true);
      await Promise.all(pending);
    }
  });
}
