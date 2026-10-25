import assert from "node:assert/strict";
import { after, test } from "node:test";
import { registerHooks } from "node:module";
import { ApiError } from "./apiError";
import { setLocale, t } from "./i18n";

import { queryClient } from "./queryClient";
import { useProjectStore } from "../stores/projectStore";
import { draftProjectKey } from "./projectScope";
import { qaLastRunKey } from "./qaDashboard";

const h = { response: undefined as unknown, wait: null as Promise<void> | null, setTauri: (_value: boolean) => {}, calls: [] as any[], reject: false };
(globalThis as any).__guardedApi = h;
registerHooks({
  load(url, context, nextLoad) {
    if (url.replace(/\\/g, "/").endsWith("/src/lib/runtime.ts")) {
      return { format: "module", shortCircuit: true, source: `
        export let IS_TAURI=true;export const isTauri=()=>IS_TAURI;
        globalThis.__guardedApi.setTauri=value=>{IS_TAURI=value;};` };
    }
    return nextLoad(url, context);
  },
});
Object.defineProperty(globalThis, "window", { configurable: true, value: {
  __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
    if (command === "get_server_port") return 7842;
    h.calls.push([command, args]);
    if (h.wait) await h.wait;
    if (h.reject) throw "translation changed since it was loaded";
    return h.response ?? { translation: args?.data?.translation };
  } },
} });
globalThis.fetch = async (url, init) => {
  h.calls.push([String(url), init?.method, init?.body ? (String(init.body).startsWith("{") ? JSON.parse(String(init.body)) : String(init.body)) : null]);
  if (h.wait) await h.wait;
  return h.reject
    ? new Response("translation changed since it was loaded", { status: 409 })
    : new Response(JSON.stringify(h.response ?? { translation: "saved" }));
};
const api = await import("./api");
// Resolve base URL using the desktop port seam before switching transports.
await api.getCurrentProject();
for (const tauri of [true, false]) {
  test(`${tauri ? "Tauri" : "HTTP"} preserves text/null/absent guards and legacy status-only requests`, async () => {
    h.setTauri(tauri); h.calls = []; h.reject = false;
    const bodies = [
      { translation: "draft", expected_translation: "V0" },
      { translation: "draft", expected_translation: null },
      { translation: "draft", expected_translation: "" },
      { translation: "legacy" },
      { status: "approved" as const },
    ];
    for (const body of bodies) await api.patchString("row/id", body);
    assert.deepEqual(h.calls, bodies.map(body => tauri
      ? ["patch_string", { id: "row/id", data: body }]
      : ["http://localhost:7842/api/strings/row%2Fid", "PATCH", body]));
  });
  test(`${tauri ? "Tauri" : "HTTP"} gives conflicts a stable key and EN/ES messages`, async () => {
    h.setTauri(tauri); h.reject = true;
    for (const locale of ["en", "es"] as const) {
      setLocale(locale);
      await assert.rejects(api.patchString("row", { translation: "draft", expected_translation: "V0" }), error => {
        assert.ok(error instanceof ApiError);
        assert.equal(error.key, "api.error.translationConflict");
        assert.equal(error.message, t("api.error.translationConflict"));
        return true;
      });
    }
  });
}

const project = { path: "/game", database_path: "/game.db", format_id: "html", name: "Fixture" };
const projectKey = draftProjectKey(project);
const editKey = ["workflow-last-edit", projectKey];
after(() => { queryClient.clear(); useProjectStore.setState({ project: null }); });
for (const tauri of [true, false]) {
  const transport = tauri ? "Tauri" : "HTTP";
  test(`${transport}: successful shared translation saves stale QA in the same millisecond`, async () => {
    h.setTauri(tauri); h.reject = false; h.response = undefined;
    useProjectStore.getState().setProject(project);
    queryClient.clear();
    const now = Date.now;
    Date.now = () => 100;
    try {
      queryClient.setQueryData(qaLastRunKey(projectKey), 100);
      await api.patchString("row", { translation: "Changed in Review", status: "approved" });
      assert.ok(queryClient.getQueryData<number>(editKey)! > 100);
      const first = queryClient.getQueryData<number>(editKey)!;
      await api.patchString("row", { translation: "" });
      assert.equal(queryClient.getQueryData<number>(editKey), first, "rapid writes must not accumulate timestamps ahead of the clock");
      assert.equal(queryClient.getQueryCache().find({ queryKey: editKey })?.gcTime, Infinity);
    } finally { Date.now = now; queryClient.clear(); }
  });
  test(`${transport}: failed writes and status-only approval leave validation current`, async () => {
    h.setTauri(tauri); h.reject = true; h.response = undefined;
    useProjectStore.getState().setProject(project);
    queryClient.clear();
    await assert.rejects(api.patchString("row", { translation: "Unsaved" }));
    assert.equal(queryClient.getQueryData(editKey), undefined);
    h.reject = false;
    await api.patchString("row", { status: "approved" });
    assert.equal(queryClient.getQueryData(editKey), undefined);
  });
  test(`${transport}: Search/Replace marks applied rows only, including partial conflicts`, async () => {
    h.setTauri(tauri); h.reject = false;
    useProjectStore.getState().setProject(project);
    queryClient.clear();
    const updates = [{ id: "row", translation: "Replacement" }];
    h.response = { requested: 1, applied: 0, skipped: 1, conflicts: ["row"] };
    await api.batchPatchStrings(updates);
    assert.equal(queryClient.getQueryData(editKey), undefined);
    h.reject = true;
    await assert.rejects(api.batchPatchStrings(updates));
    assert.equal(queryClient.getQueryData(editKey), undefined);
    h.reject = false;
    h.response = { requested: 2, applied: 1, skipped: 1, conflicts: ["other"] };
    await api.batchPatchStrings(updates);
    assert.ok(queryClient.getQueryData<number>(editKey)! > 0);
    h.response = undefined;
  });
  test(`${transport}: a delayed save marks its original database after switching projects`, async () => {
    h.setTauri(tauri); h.reject = false; h.response = undefined;
    useProjectStore.getState().setProject(project);
    queryClient.clear();
    let finish!: () => void;
    h.wait = new Promise<void>(resolve => { finish = resolve; });
    const pending = api.patchString("row", { translation: "Original project" });
    const other = { ...project, database_path: "/other.db" };
    useProjectStore.getState().setProject(other);
    finish();
    try {
      await pending;
      assert.ok(queryClient.getQueryData<number>(editKey)! > 0);
      assert.equal(queryClient.getQueryData(["workflow-last-edit", draftProjectKey(other)]), undefined);
    } finally { h.wait = null; }
  });
  test(`${transport}: successful imports mark changed rows but empty imports do not`, async () => {
    h.setTauri(tauri); h.reject = false;
    useProjectStore.getState().setProject(project);
    queryClient.clear();
    h.response = { imported: 0, skipped: 1 };
    await api.importTranslations("po", "fixture");
    assert.equal(queryClient.getQueryData(editKey), undefined);
    h.response = { imported: 1, skipped: 0 };
    await api.importTranslations("xliff", "fixture");
    assert.ok(queryClient.getQueryData<number>(editKey)! > 0);
    h.response = undefined;
  });
}
